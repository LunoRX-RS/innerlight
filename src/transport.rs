//! HTTP transport abstraction: a reqwest-backed default and an offline fixture transport for tests.

use crate::Error;
use serde_json::Value;
use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Default per-request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);
/// Default maximum response body size.
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// A JSON POST. `Debug` hides header values and the body (they can hold cookies and tokens).
pub struct Request {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Request(..)")
    }
}

/// A raw HTTP answer. Non-2xx statuses are returned as data; the client maps them to errors.
pub struct Response {
    pub status: u16,
    /// `Retry-After` in whole seconds, when present.
    pub retry_after: Option<Duration>,
    pub body: Vec<u8>,
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response").field("status", &self.status).finish_non_exhaustive()
    }
}

/// Sends one request. Implementations must be cancel-safe (the client wraps calls in a timeout).
pub trait Transport: Send + Sync + 'static {
    fn send(&self, request: Request) -> BoxFuture<'_, Result<Response, Error>>;

    /// Anonymous GET of a public static file (the player script and its loader). Never carries a cookie or any
    /// account header. The URL has been validated by [`Client::get_static`](crate::Client::get_static). The default
    /// refuses, so existing implementations keep compiling.
    fn get(&self, _url: String) -> BoxFuture<'_, Result<Response, Error>> {
        Box::pin(async { Err(Error::Refused) })
    }
}

/// Default transport: reqwest with rustls, gzip, no redirects (so cookies never follow a redirect), HTTPS only.
pub struct ReqwestTransport {
    http: reqwest::Client,
    max_bytes: usize,
}

impl ReqwestTransport {
    pub fn new(timeout: Duration, max_response_bytes: usize) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .https_only(true)
            .gzip(true)
            .build()
            .map_err(|_| Error::Network)?;
        Ok(Self { http, max_bytes: max_response_bytes })
    }
}

impl Transport for ReqwestTransport {
    fn send(&self, request: Request) -> BoxFuture<'_, Result<Response, Error>> {
        Box::pin(async move {
            let mut builder = self.http.post(&request.url).body(request.body);
            for (k, v) in &request.headers {
                builder = builder.header(k, v);
            }
            let mut resp = builder.send().await.map_err(|_| Error::Network)?;
            let status = resp.status().as_u16();
            let retry_after = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            if resp.content_length().is_some_and(|n| n > self.max_bytes as u64) {
                return Err(Error::TooLarge);
            }
            let mut body = Vec::new();
            while let Some(chunk) = resp.chunk().await.map_err(|_| Error::Network)? {
                if body.len() + chunk.len() > self.max_bytes {
                    return Err(Error::TooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(Response { status, retry_after, body })
        })
    }

    fn get(&self, url: String) -> BoxFuture<'_, Result<Response, Error>> {
        Box::pin(async move { self.fetch(&url).await })
    }
}

impl ReqwestTransport {
    async fn fetch(&self, url: &str) -> Result<Response, Error> {
        let mut resp = self
            .http
            .get(url)
            .header("User-Agent", crate::client::versions::USER_AGENT)
            .send()
            .await
            .map_err(|_| Error::Network)?;
        let status = resp.status().as_u16();
        if resp.content_length().is_some_and(|n| n > self.max_bytes as u64) {
            return Err(Error::TooLarge);
        }
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|_| Error::Network)? {
            if body.len() + chunk.len() > self.max_bytes {
                return Err(Error::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Response { status, retry_after: None, body })
    }
}

/// One request as seen by a [`FixtureTransport`].
#[derive(Clone)]
pub struct Recorded {
    /// `www.youtube.com` or `music.youtube.com`.
    pub host: String,
    /// Endpoint path after `/youtubei/v1/`, e.g. `browse`.
    pub endpoint: String,
    pub headers: Vec<(String, String)>,
    /// The full request body, including the added `context`.
    pub body: Value,
}

impl fmt::Debug for Recorded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Recorded").field("host", &self.host).field("endpoint", &self.endpoint).finish_non_exhaustive()
    }
}

impl Recorded {
    /// Header value by case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

struct Rule {
    endpoint: String,
    /// When set, the rule only matches requests whose serialized body contains this text.
    contains: Option<String>,
    queue: VecDeque<(u16, Vec<u8>)>,
}

/// Offline transport for tests: returns canned answers by endpoint and records every request.
///
/// Rules are checked in the order added; the first whose endpoint (and optional body text) matches wins.
/// Each rule answers its responses in order and repeats the last one. A request that matches no rule
/// **panics** with the endpoint name, so a missing fixture fails loudly in tests.
#[derive(Default)]
pub struct FixtureTransport {
    rules: Mutex<Vec<Rule>>,
    log: Mutex<Vec<Recorded>>,
}

impl FixtureTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Answer `endpoint` with 200 and this JSON (appended to the endpoint's rule if it has one without a body filter).
    pub fn with(self, endpoint: &str, json: Value) -> Self {
        self.rule(endpoint, None, 200, serde_json::to_vec(&json).expect("json serializes"))
    }

    /// Like [`with`](Self::with), but only for requests whose body text contains `needle`
    /// (e.g. a continuation token). Put these before the catch-all `with` for the same endpoint.
    pub fn with_match(self, endpoint: &str, needle: &str, json: Value) -> Self {
        self.rule(endpoint, Some(needle), 200, serde_json::to_vec(&json).expect("json serializes"))
    }

    /// Answer `endpoint` with a raw status and body (for error and malformed-JSON tests).
    pub fn with_raw(self, endpoint: &str, status: u16, body: &[u8]) -> Self {
        self.rule(endpoint, None, status, body.to_vec())
    }

    /// Answer `endpoint` with 200 and the contents of a JSON file such as `tests/fixtures/x.json`.
    pub fn with_file(self, endpoint: &str, path: impl AsRef<std::path::Path>) -> Self {
        let body = std::fs::read(path.as_ref()).expect("fixture file readable");
        self.rule(endpoint, None, 200, body)
    }

    fn rule(self, endpoint: &str, contains: Option<&str>, status: u16, body: Vec<u8>) -> Self {
        {
            let mut rules = self.rules.lock().unwrap();
            match rules.iter_mut().find(|r| r.endpoint == endpoint && r.contains.as_deref() == contains) {
                Some(r) => r.queue.push_back((status, body)),
                None => rules.push(Rule {
                    endpoint: endpoint.to_owned(),
                    contains: contains.map(str::to_owned),
                    queue: VecDeque::from([(status, body)]),
                }),
            }
        }
        self
    }

    /// Every request seen so far, oldest first.
    pub fn requests(&self) -> Vec<Recorded> {
        self.log.lock().unwrap().clone()
    }
}

impl Transport for FixtureTransport {
    fn send(&self, request: Request) -> BoxFuture<'_, Result<Response, Error>> {
        Box::pin(async move {
            let rest = request.url.strip_prefix("https://").unwrap_or(&request.url);
            let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
            let endpoint = path.strip_prefix("youtubei/v1/").unwrap_or(path);
            let endpoint = endpoint.split('?').next().unwrap_or("").to_owned();
            let body_text = String::from_utf8_lossy(&request.body).into_owned();
            self.log.lock().unwrap().push(Recorded {
                host: host.to_owned(),
                endpoint: endpoint.clone(),
                headers: request.headers.clone(),
                body: serde_json::from_slice(&request.body).unwrap_or(Value::Null),
            });
            let mut rules = self.rules.lock().unwrap();
            let rule = rules
                .iter_mut()
                .find(|r| r.endpoint == endpoint && r.contains.as_deref().is_none_or(|n| body_text.contains(n)))
                .unwrap_or_else(|| panic!("FixtureTransport: no fixture for endpoint `{endpoint}`"));
            let (status, body) = if rule.queue.len() > 1 {
                rule.queue.pop_front().expect("non-empty")
            } else {
                rule.queue.front().cloned().expect("non-empty")
            };
            Ok(Response { status, retry_after: None, body })
        })
    }

    /// GET: the endpoint is the path without the leading `/` and without the query, e.g. `iframe_api` or
    /// `s/player/abcd1234/player_ias.vflset/en_US/base.js`. Register answers with [`with_raw`](Self::with_raw).
    fn get(&self, url: String) -> BoxFuture<'_, Result<Response, Error>> {
        Box::pin(async move {
            let rest = url.strip_prefix("https://").unwrap_or(&url);
            let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
            let endpoint = path.split(['?', '#']).next().unwrap_or("").to_owned();
            self.log.lock().unwrap().push(Recorded {
                host: host.to_owned(),
                endpoint: endpoint.clone(),
                headers: Vec::new(),
                body: Value::Null,
            });
            let mut rules = self.rules.lock().unwrap();
            let rule = rules
                .iter_mut()
                .find(|r| r.endpoint == endpoint)
                .unwrap_or_else(|| panic!("FixtureTransport: no fixture for GET `{endpoint}`"));
            let (status, body) = if rule.queue.len() > 1 {
                rule.queue.pop_front().expect("non-empty")
            } else {
                rule.queue.front().cloned().expect("non-empty")
            };
            Ok(Response { status, retry_after: None, body })
        })
    }
}
