//! The InnerTube client: builds the request context, validates the endpoint, signs when asked, maps failures.

use crate::Error;
use crate::session::Session;
use crate::transport::{DEFAULT_MAX_RESPONSE_BYTES, DEFAULT_TIMEOUT, Request, ReqwestTransport, Transport};
use serde_json::{Value, json};
use std::fmt;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Client identity strings. Update here when the site's web client version moves on; nothing else hard-codes them.
pub mod versions {
    pub const WEB_NAME: &str = "WEB";
    pub const WEB_VERSION: &str = "2.20251020.01.00";
    pub const WEB_ID: &str = "1";
    pub const WEB_MUSIC_NAME: &str = "WEB_REMIX";
    pub const WEB_MUSIC_VERSION: &str = "1.20251020.03.00";
    pub const WEB_MUSIC_ID: &str = "67";
    /// One fixed desktop browser User-Agent, sent on every request. It is never rotated.
    pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
}

/// Which site front end the request pretends to come from: the standard web clients, nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClientKind {
    /// www.youtube.com
    Web,
    /// music.youtube.com
    WebMusic,
}

impl ClientKind {
    /// `https://www.youtube.com` or `https://music.youtube.com`.
    pub fn origin(self) -> &'static str {
        match self {
            ClientKind::Web => "https://www.youtube.com",
            ClientKind::WebMusic => "https://music.youtube.com",
        }
    }

    fn name(self) -> &'static str {
        match self {
            ClientKind::Web => versions::WEB_NAME,
            ClientKind::WebMusic => versions::WEB_MUSIC_NAME,
        }
    }

    fn version(self) -> &'static str {
        match self {
            ClientKind::Web => versions::WEB_VERSION,
            ClientKind::WebMusic => versions::WEB_MUSIC_VERSION,
        }
    }

    fn id(self) -> &'static str {
        match self {
            ClientKind::Web => versions::WEB_ID,
            ClientKind::WebMusic => versions::WEB_MUSIC_ID,
        }
    }

    /// The `context` object added to every request body.
    pub fn context(self, hl: &str, gl: &str, visitor_data: Option<&str>) -> Value {
        let mut client = json!({
            "clientName": self.name(),
            "clientVersion": self.version(),
            "hl": hl,
            "gl": gl,
        });
        if let Some(v) = visitor_data {
            client["visitorData"] = json!(v);
        }
        json!({ "client": client, "user": { "lockedSafetyMode": false } })
    }
}

/// Accepts `[a-z_/]+` only: no empty string, no `..`, no leading, trailing or doubled `/`.
pub fn validate_endpoint(endpoint: &str) -> Result<(), Error> {
    let ok = !endpoint.is_empty()
        && endpoint.bytes().all(|b| b.is_ascii_lowercase() || b == b'_' || b == b'/')
        && !endpoint.contains("..")
        && !endpoint.contains("//")
        && !endpoint.starts_with('/')
        && !endpoint.ends_with('/');
    ok.then_some(()).ok_or(Error::InvalidRequest)
}

struct State {
    session: Session,
    visitor_data: Option<String>,
}

struct Inner {
    transport: Arc<dyn Transport>,
    hl: String,
    gl: String,
    timeout: Duration,
    max_bytes: usize,
    state: RwLock<State>,
}

/// Cheap to clone (shared state). `Debug` prints `Client(..)`.
#[derive(Clone)]
pub struct Client(Arc<Inner>);

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Client(..)")
    }
}

pub struct ClientBuilder {
    hl: String,
    gl: String,
    timeout: Duration,
    max_bytes: usize,
    transport: Option<Arc<dyn Transport>>,
}

impl ClientBuilder {
    /// UI language, e.g. `en`. Default `en`.
    pub fn language(mut self, hl: &str) -> Self {
        self.hl = hl.to_owned();
        self
    }

    /// Region, e.g. `US`. Default `US`.
    pub fn country(mut self, gl: &str) -> Self {
        self.gl = gl.to_owned();
        self
    }

    /// Per-request timeout (default 20 s). Enforced by the client for every transport.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Maximum response body size (default 8 MiB). Enforced by the client for every transport.
    pub fn max_response_bytes(mut self, max: usize) -> Self {
        self.max_bytes = max;
        self
    }

    /// Replace the default reqwest transport, e.g. with a `FixtureTransport` in tests.
    pub fn transport(mut self, transport: Arc<dyn Transport>) -> Self {
        self.transport = Some(transport);
        self
    }

    pub fn build(self) -> Result<Client, Error> {
        let valid = |s: &str, min: usize, max: usize| {
            (min..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphabetic() || b == b'-')
        };
        if !valid(&self.hl, 2, 8) || !valid(&self.gl, 2, 2) {
            return Err(Error::InvalidRequest);
        }
        let transport = match self.transport {
            Some(t) => t,
            None => Arc::new(ReqwestTransport::new(self.timeout, self.max_bytes)?),
        };
        Ok(Client(Arc::new(Inner {
            transport,
            hl: self.hl,
            gl: self.gl,
            timeout: self.timeout,
            max_bytes: self.max_bytes,
            state: RwLock::new(State { session: Session::new(), visitor_data: None }),
        })))
    }
}

impl Client {
    pub fn builder() -> ClientBuilder {
        ClientBuilder {
            hl: "en".into(),
            gl: "US".into(),
            timeout: DEFAULT_TIMEOUT,
            max_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            transport: None,
        }
    }

    fn state<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let mut guard = self.0.state.write().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }

    /// Stores the Google cookie string used by [`post_authed`](Self::post_authed).
    pub fn set_cookie(&self, cookie: &str) -> Result<(), Error> {
        self.state(|s| s.session.set_cookie(cookie))
    }

    /// Forgets the cookie.
    pub fn clear_session(&self) {
        self.state(|s| s.session.clear());
    }

    /// True when a signable cookie is stored.
    pub fn is_signed_in(&self) -> bool {
        self.state(|s| s.session.can_sign())
    }

    /// Visitor data sent with every request; learned from `responseContext.visitorData` when not set.
    pub fn visitor_data(&self) -> Option<String> {
        self.state(|s| s.visitor_data.clone())
    }

    pub fn set_visitor_data(&self, visitor_data: Option<String>) {
        self.state(|s| s.visitor_data = visitor_data);
    }

    /// Anonymous POST to `/youtubei/v1/<endpoint>`. `body` must be a JSON object; `context` is added to it.
    /// The cookie is never sent.
    pub async fn post(&self, kind: ClientKind, endpoint: &str, body: Value) -> Result<Value, Error> {
        self.send(kind, endpoint, body, false).await
    }

    /// Like [`post`](Self::post) but signed with the stored cookie (`SAPISIDHASH`).
    /// Returns [`Error::Unauthenticated`] without sending anything when no signable cookie is stored.
    pub async fn post_authed(&self, kind: ClientKind, endpoint: &str, body: Value) -> Result<Value, Error> {
        self.send(kind, endpoint, body, true).await
    }

    async fn send(&self, kind: ClientKind, endpoint: &str, mut body: Value, authed: bool) -> Result<Value, Error> {
        validate_endpoint(endpoint)?;
        let obj = body.as_object_mut().ok_or(Error::InvalidRequest)?;
        let (visitor, auth) = self.state(|s| {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            (s.visitor_data.clone(), authed.then(|| s.session.auth_headers(kind, now)))
        });
        let auth = match auth {
            Some(Some(h)) => h,
            Some(None) => return Err(Error::Unauthenticated),
            None => Vec::new(),
        };
        obj.insert("context".into(), kind.context(&self.0.hl, &self.0.gl, visitor.as_deref()));

        let mut headers = vec![
            ("Content-Type".to_owned(), "application/json".to_owned()),
            ("User-Agent".to_owned(), versions::USER_AGENT.to_owned()),
            ("Accept-Language".to_owned(), self.0.hl.clone()),
            ("X-Youtube-Client-Name".to_owned(), kind.id().to_owned()),
            ("X-Youtube-Client-Version".to_owned(), kind.version().to_owned()),
            ("Origin".to_owned(), kind.origin().to_owned()),
        ];
        if let Some(v) = &visitor {
            headers.push(("X-Goog-Visitor-Id".to_owned(), v.clone()));
        }
        for (k, v) in auth {
            match headers.iter_mut().find(|(hk, _)| *hk == k) {
                Some(slot) => slot.1 = v,
                None => headers.push((k, v)),
            }
        }

        let request = Request {
            url: format!("{}/youtubei/v1/{endpoint}?prettyPrint=false", kind.origin()),
            headers,
            body: serde_json::to_vec(&body).map_err(|_| Error::InvalidRequest)?,
        };
        let response = tokio::time::timeout(self.0.timeout, self.0.transport.send(request))
            .await
            .map_err(|_| Error::Network)??;
        if !(200..300).contains(&response.status) {
            return Err(Error::from_status(response.status, response.retry_after));
        }
        if response.body.len() > self.0.max_bytes {
            return Err(Error::TooLarge);
        }
        let value: Value = serde_json::from_slice(&response.body).map_err(|_| Error::Parse)?;
        if visitor.is_none()
            && let Some(v) = value.pointer("/responseContext/visitorData").and_then(Value::as_str)
        {
            self.state(|s| {
                s.visitor_data.get_or_insert_with(|| v.to_owned());
            });
        }
        Ok(value)
    }
}
