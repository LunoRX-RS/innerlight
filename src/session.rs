//! Cookie session and the `SAPISIDHASH` request signature. Authenticated requests are opt-in per call.

use crate::{ClientKind, Error};
use sha1::{Digest, Sha1};
use std::fmt;

/// A Google cookie string (`name=value; name=value`). Never logged: `Debug` prints `Session(..)`.
#[derive(Default, Clone)]
pub struct Session {
    cookie: Option<String>,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Session(..)")
    }
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores the cookie string. Rejects empty strings and control characters (header injection).
    pub fn set_cookie(&mut self, cookie: &str) -> Result<(), Error> {
        let cookie = cookie.trim();
        if cookie.is_empty() || cookie.bytes().any(|b| b.is_ascii_control()) {
            return Err(Error::InvalidRequest);
        }
        self.cookie = Some(cookie.to_owned());
        Ok(())
    }

    pub fn clear(&mut self) {
        self.cookie = None;
    }

    /// True when a cookie containing a `SAPISID` value is stored, i.e. requests can be signed.
    pub fn can_sign(&self) -> bool {
        self.sapisid().is_some()
    }

    /// The `SAPISID` cookie value (falls back to `__Secure-3PAPISID`, which carries the same value).
    fn sapisid(&self) -> Option<&str> {
        let cookie = self.cookie.as_deref()?;
        let find = |name: &str| {
            cookie.split(';').find_map(|pair| {
                let (k, v) = pair.split_once('=')?;
                (k.trim() == name && !v.trim().is_empty()).then(|| v.trim())
            })
        };
        find("SAPISID").or_else(|| find("__Secure-3PAPISID"))
    }

    /// The `Authorization` header value `SAPISIDHASH {ts}_{sha1("{ts} {SAPISID} {origin}")}` for `kind`'s origin.
    pub fn sapisidhash(&self, kind: ClientKind, timestamp: u64) -> Option<String> {
        let sapisid = self.sapisid()?;
        let digest = Sha1::digest(format!("{timestamp} {sapisid} {}", kind.origin()).as_bytes());
        let mut out = format!("SAPISIDHASH {timestamp}_");
        for b in digest {
            out.push_str(&format!("{b:02x}"));
        }
        Some(out)
    }

    /// Headers for a signed request: `Cookie`, `Authorization`, `Origin`, `X-Origin`, `X-Goog-AuthUser`.
    /// `None` when no signable cookie is stored.
    pub fn auth_headers(&self, kind: ClientKind, timestamp: u64) -> Option<Vec<(String, String)>> {
        let auth = self.sapisidhash(kind, timestamp)?;
        let origin = kind.origin().to_owned();
        Some(vec![
            ("Cookie".into(), self.cookie.clone()?),
            ("Authorization".into(), auth),
            ("Origin".into(), origin.clone()),
            ("X-Origin".into(), origin),
            ("X-Goog-AuthUser".into(), "0".into()),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(c: &str) -> Session {
        let mut s = Session::new();
        s.set_cookie(c).unwrap();
        s
    }

    // Vectors computed independently with `echo -n "<ts> <sapisid> <origin>" | shasum -a 1`.
    #[test]
    fn sapisidhash_known_vectors() {
        let s = session("SID=x; SAPISID=abc123; HSID=y");
        assert_eq!(
            s.sapisidhash(ClientKind::Web, 1700000000).unwrap(),
            "SAPISIDHASH 1700000000_9e5071f149fc514366f78b22d1a169786d40ed32"
        );
        assert_eq!(
            s.sapisidhash(ClientKind::WebMusic, 1700000000).unwrap(),
            "SAPISIDHASH 1700000000_597a8411c1c01f9c14cea84b6ec377b76d93be6e"
        );
    }

    #[test]
    fn falls_back_to_secure_3papisid_and_needs_exact_name() {
        let s = session("__Secure-3PAPISID=abc123");
        assert!(
            s.sapisidhash(ClientKind::Web, 1700000000).unwrap().ends_with("9e5071f149fc514366f78b22d1a169786d40ed32")
        );
        assert!(!session("XSAPISID=abc123; SID=1").can_sign());
        assert!(!session("SAPISID=; SID=1").can_sign());
        assert!(!Session::new().can_sign());
    }

    #[test]
    fn headers_and_clear() {
        let mut s = session("SAPISID=abc123");
        let h = s.auth_headers(ClientKind::WebMusic, 1).unwrap();
        let get = |n: &str| h.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
        assert_eq!(get("Origin"), Some("https://music.youtube.com"));
        assert_eq!(get("X-Goog-AuthUser"), Some("0"));
        assert_eq!(get("Cookie"), Some("SAPISID=abc123"));
        s.clear();
        assert!(s.auth_headers(ClientKind::Web, 1).is_none());
    }

    #[test]
    fn rejects_bad_cookie_and_hides_debug() {
        let mut s = Session::new();
        assert_eq!(s.set_cookie("a=b\r\nX: y"), Err(Error::InvalidRequest));
        assert_eq!(s.set_cookie("  "), Err(Error::InvalidRequest));
        assert!(!s.can_sign());
        let s = session("SAPISID=topsecret");
        assert_eq!(format!("{s:?}"), "Session(..)");
        assert_eq!(format!("{s:#?}"), "Session(..)");
    }
}
