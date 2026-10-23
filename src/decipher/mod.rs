//! Signature descrambling and `n` throttle-parameter rewriting: the real [`Decipher`] for the `player` module.
//!
//! This is the signature-cipher / throttle-parameter transform only. It makes no request to the `player` endpoint,
//! does not touch the bot check, and uses no user-agent or client-identity tricks. Network use: one anonymous GET for
//! the public loader (to learn the current player version) and one for the public player script, both through
//! [`Client::get_static`](crate::Client::get_static).
//!
//! How it works (details and "what breaks when the script changes" in `docs/DECIPHER.md`):
//! 1. [`PlayerScript`] holds one script version (downloaded by [`PlayerScript::fetch_current`] or given to
//!    [`PlayerScript::from_source`] for offline use) and extracts `signatureTimestamp`.
//! 2. [`ScriptDecipher::new`] finds the two transforms by structure, loads only the definitions they depend on into a
//!    sandboxed pure-Rust JS engine, and verifies by behaviour that each really is the transform it was taken for.
//! 3. [`ScriptCache`] keeps the result per version and refetches when asked to (`invalidate`, after a `DecipherFailed`).
//!
//! Heavy work (lexing a ~3 MB script, loading the engine) is blocking CPU work of roughly a second: call
//! [`ScriptDecipher::new`] from a blocking context if you are on an async executor with other latency-sensitive tasks.

mod error;
mod index;
mod lexer;
mod locate;
mod plan;
pub mod sandbox;

pub use error::{DecipherError, Which};
pub use sandbox::{Sandbox, SandboxLimits};

use crate::Client;
use crate::player::{Decipher, PlayerError, check_stream_url};
use index::Index;
use plan::Resolved;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Largest player script accepted.
pub const MAX_SCRIPT_BYTES: usize = 4 * 1024 * 1024;
/// The public loader that names the current player version.
pub const LOADER_URL: &str = "https://www.youtube.com/iframe_api";
const MAX_SIGNATURE_LEN: usize = 4096;
const MAX_N_LEN: usize = 512;

fn valid_version(v: &str) -> bool {
    (4..=32).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `https://www.youtube.com/s/player/<version>/player_ias.vflset/en_US/base.js`, or `None` for a malformed version.
pub fn script_url(version: &str) -> Option<String> {
    valid_version(version)
        .then(|| format!("https://www.youtube.com/s/player/{version}/player_ias.vflset/en_US/base.js"))
}

/// True for `https://(www.youtube.com|s.ytimg.com)/s/player/<version>/<path>.js` and nothing else.
pub fn is_script_url(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url) else { return false };
    let mut parts = u.path().trim_start_matches('/').split('/');
    let shape = parts.next() == Some("s") && parts.next() == Some("player") && parts.next().is_some_and(valid_version);
    u.scheme() == "https"
        && u.host_str().is_some_and(|h| h == "www.youtube.com" || h == "s.ytimg.com")
        && u.username().is_empty()
        && u.port().is_none()
        && u.query().is_none()
        && u.fragment().is_none()
        && shape
        && u.path().ends_with(".js")
}

/// The player version named by the loader text (it contains `.../s/player/<version>/www-widgetapi...`, with the
/// slashes escaped or not).
pub fn loader_version(loader: &str) -> Option<String> {
    let text = loader.replace("\\/", "/");
    let mut from = 0;
    while let Some(p) = text[from..].find("/s/player/") {
        let start = from + p + "/s/player/".len();
        let end = text[start..].find('/').map_or(text.len(), |e| start + e);
        let v = &text[start..end];
        if valid_version(v) {
            return Some(v.to_owned());
        }
        from = start;
    }
    None
}

/// One version of the player script. Cheap to clone (the text is shared).
#[derive(Clone)]
pub struct PlayerScript {
    version: String,
    source: Arc<str>,
}

impl std::fmt::Debug for PlayerScript {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlayerScript").field("version", &self.version).field("bytes", &self.source.len()).finish()
    }
}

impl PlayerScript {
    /// Offline constructor. `BadScript` for an empty or over-long (> [`MAX_SCRIPT_BYTES`]) script or a malformed version.
    pub fn from_source(version: String, js: String) -> Result<PlayerScript, DecipherError> {
        if !valid_version(&version) || js.trim().is_empty() || js.len() > MAX_SCRIPT_BYTES {
            return Err(DecipherError::BadScript);
        }
        Ok(PlayerScript { version, source: Arc::from(js) })
    }

    /// Learns the current version from the public loader.
    pub async fn current_version(client: &Client) -> Result<String, DecipherError> {
        let body = client.get_static(LOADER_URL).await?;
        let text = String::from_utf8(body).map_err(|_| DecipherError::BadScript)?;
        loader_version(&text).ok_or(DecipherError::ScriptShapeChanged { which: Which::Version })
    }

    /// Downloads one known version.
    pub async fn fetch_version(client: &Client, version: &str) -> Result<PlayerScript, DecipherError> {
        let url = script_url(version).ok_or(DecipherError::BadScript)?;
        if !is_script_url(&url) {
            return Err(DecipherError::BadScript);
        }
        let body = client.get_static(&url).await?;
        if body.len() > MAX_SCRIPT_BYTES {
            return Err(DecipherError::BadScript);
        }
        let js = String::from_utf8(body).map_err(|_| DecipherError::BadScript)?;
        PlayerScript::from_source(version.to_owned(), js)
    }

    /// Finds the current script URL through the loader and downloads it (two anonymous GETs).
    pub async fn fetch_current(client: &Client) -> Result<PlayerScript, DecipherError> {
        let version = Self::current_version(client).await?;
        Self::fetch_version(client, &version).await
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn len(&self) -> usize {
        self.source.len()
    }

    pub fn is_empty(&self) -> bool {
        self.source.is_empty()
    }

    /// The `signatureTimestamp` the caller must put into `ResolveOptions::signature_timestamp`. Looks for
    /// `signatureTimestamp` followed by `:` or `=` and a number; falls back to a bare `sts:<5 digits>`.
    pub fn signature_timestamp(&self) -> Result<u32, DecipherError> {
        let s: &str = &self.source;
        let number_after = |at: usize, min: u32, max: u32| -> Option<u32> {
            let rest = s.get(at..)?.trim_start_matches(['"', '\'', ' ']);
            let rest = rest.strip_prefix([':', '='])?.trim_start_matches(['"', '\'', ' ']);
            let digits: String = rest.chars().take_while(char::is_ascii_digit).take(9).collect();
            digits.parse::<u32>().ok().filter(|n| (min..=max).contains(n))
        };
        const KEY: &str = "signatureTimestamp";
        let first = s.match_indices(KEY).find_map(|(i, _)| number_after(i + KEY.len(), 1000, 99_999_999));
        let fallback = || {
            s.match_indices("sts").find_map(|(i, _)| {
                let boundary = i == 0 || !s.as_bytes()[i - 1].is_ascii_alphanumeric();
                (boundary && s[i + 3..].starts_with([':', '='])).then(|| number_after(i + 3, 10_000, 99_999)).flatten()
            })
        };
        first.or_else(fallback).ok_or(DecipherError::ScriptShapeChanged { which: Which::SignatureTimestamp })
    }
}

/// Percent-encodes everything but unreserved characters.
fn pct(s: &str) -> String {
    s.bytes().fold(String::with_capacity(s.len()), |mut out, b| {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
        out
    })
}

/// The real [`Decipher`]: signature descrambler and `n` transform of one player script version. `Send + Sync`;
/// each transform runs on its own sandbox thread and calls to the same transform are serialised.
pub struct ScriptDecipher {
    script: PlayerScript,
    sig: Resolved,
    throttle: Resolved,
}

impl std::fmt::Debug for ScriptDecipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptDecipher").field("version", &self.script.version).finish_non_exhaustive()
    }
}

impl ScriptDecipher {
    /// Locates and verifies both transforms. `ScriptShapeChanged { which }` when a transform cannot be found or does
    /// not behave like one. Blocking CPU work.
    pub fn new(script: &PlayerScript) -> Result<ScriptDecipher, DecipherError> {
        Self::with_limits(script, SandboxLimits::default())
    }

    pub fn with_limits(script: &PlayerScript, limits: SandboxLimits) -> Result<ScriptDecipher, DecipherError> {
        let idx = Index::new(&script.source);
        let sig = plan::resolve(&idx, Which::Signature, &locate::signature_candidates(&idx), &[], limits)?;
        let (cands, brute) = locate::throttle_candidates(&idx);
        let throttle = plan::resolve(&idx, Which::Throttle, &cands, &brute, limits)?;
        Ok(ScriptDecipher { script: script.clone(), sig, throttle })
    }

    pub fn version(&self) -> &str {
        self.script.version()
    }

    pub fn signature_timestamp(&self) -> Result<u32, DecipherError> {
        self.script.signature_timestamp()
    }

    /// Which heuristics found the transforms: `(signature, throttle)`. For logs and tests.
    pub fn found_by(&self) -> (&'static str, &'static str) {
        (self.sig.how, self.throttle.how)
    }

    /// Descrambles the `s` value of a `signatureCipher`. Output: non-empty, no longer than the input, and made only of
    /// the input's characters (a real descrambler reverses, drops and swaps characters, nothing else).
    pub fn signature(&self, s: &str) -> Result<String, DecipherError> {
        if s.is_empty() || s.len() > MAX_SIGNATURE_LEN || s.chars().any(char::is_control) {
            return Err(DecipherError::BadInput);
        }
        let out = self.sig.sandbox.call(&self.sig.template, s)?;
        if out.is_empty() || out.chars().count() > s.chars().count() || !out.chars().all(|c| s.contains(c)) {
            return Err(DecipherError::BadOutput);
        }
        Ok(out)
    }

    /// Transforms one `n` value. Never returns the input, empty output or the script's failure fallback.
    pub fn throttle_value(&self, n: &str) -> Result<String, DecipherError> {
        if n.is_empty()
            || n.len() > MAX_N_LEN
            || !n.bytes().all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\'' && b != b'\\')
        {
            return Err(DecipherError::BadInput);
        }
        let out = self.throttle.sandbox.call(&self.throttle.template, n)?;
        plan::check_n_output(n, &out)?;
        Ok(out)
    }

    /// Rewrites the `n` query parameter of `url`; a URL without `n` is returned unchanged. The result passes
    /// [`check_stream_url`].
    pub fn throttle_url(&self, url: &str) -> Result<String, DecipherError> {
        let mut u = reqwest::Url::parse(url).map_err(|_| DecipherError::BadInput)?;
        let query = u.query().unwrap_or("").to_owned();
        let Some(pos) = query.split('&').position(|seg| seg.starts_with("n=")) else {
            // Nothing to rewrite: hand the URL back as it came, once it is known to be acceptable.
            return check_stream_url(url).map(|_| url.to_owned()).map_err(|_| DecipherError::UnsafeUrl);
        };
        let value = u.query_pairs().find(|(k, _)| k == "n").map(|(_, v)| v.into_owned()).unwrap_or_default();
        let fixed = self.throttle_value(&value)?;
        let rebuilt: Vec<String> = query
            .split('&')
            .enumerate()
            .map(|(i, seg)| if i == pos { format!("n={}", pct(&fixed)) } else { seg.to_owned() })
            .collect();
        u.set_query(Some(&rebuilt.join("&")));
        check_stream_url(u.as_str()).map_err(|_| DecipherError::UnsafeUrl)
    }
}

impl Decipher for ScriptDecipher {
    fn decipher(&self, s: &str) -> Result<String, PlayerError> {
        self.signature(s).map_err(PlayerError::from)
    }

    fn throttle_fix(&self, url: &str) -> Result<String, PlayerError> {
        self.throttle_url(url).map_err(PlayerError::from)
    }
}

struct Cached {
    decipher: Arc<ScriptDecipher>,
    checked: Instant,
}

/// Keeps the [`ScriptDecipher`] of the current player version in memory. Typical use:
///
/// ```ignore
/// let cache = ScriptCache::new();
/// let d = cache.get(&client).await?;                       // fetch + analyse on first use, then cached
/// let ts = d.signature_timestamp()?;                       // for ResolveOptions
/// match player.with_decipher(d).resolve(id, opts).await {
///     Err(PlayerError::DecipherFailed) => cache.invalidate(),   // the next get() refetches the script
///     other => ...
/// }
/// ```
///
/// While the cached entry is younger than the check interval no request is made; after that one small GET of the
/// loader confirms the version and the script is only downloaded again when the version changed. Two tasks calling
/// `get` at the same moment may both fetch; the last result wins (harmless).
pub struct ScriptCache {
    state: Mutex<Option<Cached>>,
    recheck: Duration,
    limits: SandboxLimits,
}

impl Default for ScriptCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptCache {
    /// Rechecks the version after one hour.
    pub fn new() -> Self {
        Self { state: Mutex::new(None), recheck: Duration::from_secs(3600), limits: SandboxLimits::default() }
    }

    pub fn with_recheck_interval(mut self, interval: Duration) -> Self {
        self.recheck = interval;
        self
    }

    pub fn with_limits(mut self, limits: SandboxLimits) -> Self {
        self.limits = limits;
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Cached>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Forgets the cached script; the next [`get`](Self::get) downloads and analyses again. Call it after a
    /// `PlayerError::DecipherFailed`.
    pub fn invalidate(&self) {
        *self.lock() = None;
    }

    /// The cached version, if any.
    pub fn cached_version(&self) -> Option<String> {
        self.lock().as_ref().map(|c| c.decipher.version().to_owned())
    }

    pub async fn get(&self, client: &Client) -> Result<Arc<ScriptDecipher>, DecipherError> {
        if let Some(c) = self.lock().as_ref().filter(|c| c.checked.elapsed() < self.recheck) {
            return Ok(Arc::clone(&c.decipher));
        }
        let version = PlayerScript::current_version(client).await?;
        if let Some(c) = self.lock().as_mut().filter(|c| c.decipher.version() == version) {
            c.checked = Instant::now();
            return Ok(Arc::clone(&c.decipher));
        }
        let script = PlayerScript::fetch_version(client, &version).await?;
        let decipher = Arc::new(ScriptDecipher::with_limits(&script, self.limits)?);
        *self.lock() = Some(Cached { decipher: Arc::clone(&decipher), checked: Instant::now() });
        Ok(decipher)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_and_urls() {
        let loader = "var scriptUrl = 'https:\\/\\/www.youtube.com\\/s\\/player\\/cbcfbf2f\\/www-widgetapi.vflset\\/www-widgetapi.js';";
        assert_eq!(loader_version(loader).as_deref(), Some("cbcfbf2f"));
        assert_eq!(loader_version("x https://www.youtube.com/s/player/ab12CD_-/w.js").as_deref(), Some("ab12CD_-"));
        assert_eq!(loader_version("no version here"), None);
        assert_eq!(loader_version("/s/player/../x"), None);
        let u = script_url("cbcfbf2f").unwrap();
        assert!(is_script_url(&u));
        assert!(script_url("../etc").is_none() && script_url("ab").is_none());
        for bad in [
            "http://www.youtube.com/s/player/cbcfbf2f/base.js",
            "https://evil.com/s/player/cbcfbf2f/base.js",
            "https://www.youtube.com.evil.com/s/player/cbcfbf2f/base.js",
            "https://www.youtube.com:444/s/player/cbcfbf2f/base.js",
            "https://u@www.youtube.com/s/player/cbcfbf2f/base.js",
            "https://www.youtube.com/s/player/cbcfbf2f/base.js?x=1",
            "https://www.youtube.com/s/other/cbcfbf2f/base.js",
            "https://www.youtube.com/s/player/cbcfbf2f/base.css",
            "https://www.youtube.com/s/player/../base.js",
            "",
        ] {
            assert!(!is_script_url(bad), "{bad}");
        }
    }

    #[test]
    fn timestamp_heuristics() {
        let mk = |js: &str| PlayerScript::from_source("abcd1234".into(), js.into()).unwrap();
        assert_eq!(mk("a={signatureTimestamp:20746,b:1}").signature_timestamp(), Ok(20746));
        assert_eq!(mk("a={\"signatureTimestamp\" : 19000}").signature_timestamp(), Ok(19000));
        assert_eq!(mk("var x=signatureTimestamp=\"20001\"").signature_timestamp(), Ok(20001));
        assert_eq!(mk("a,sts:19999,b").signature_timestamp(), Ok(19999));
        assert!(mk("a=ests:19999").signature_timestamp().is_err());
        assert_eq!(
            mk("signatureTimestamp:5").signature_timestamp().unwrap_err(),
            DecipherError::ScriptShapeChanged { which: Which::SignatureTimestamp }
        );
        assert!(PlayerScript::from_source("abcd1234".into(), "  ".into()).is_err());
        assert!(PlayerScript::from_source("bad version".into(), "x".into()).is_err());
        assert!(PlayerScript::from_source("abcd1234".into(), "x".repeat(MAX_SCRIPT_BYTES + 1)).is_err());
    }
}
