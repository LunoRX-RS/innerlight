//! HTTP transport abstraction: a reqwest-backed default and an offline fixture transport for tests.

use crate::Error;
use serde_json::Value;
use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Default per-request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);
/// Default maximum response body size.
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Month abbreviations in calendar order, three letters, lower case.
const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// `Retry-After` in the two forms the header allows: whole seconds, or an IMF-fixdate
/// (`Wed, 21 Oct 2015 07:28:00 GMT`). A date in the past, and any negative or unparseable value, is `None`
/// so the caller falls back to its own backoff. `now` is only used to turn a date into a delay.
pub fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        return value.parse::<u64>().ok().map(Duration::from_secs);
    }
    // `[<wday>,] <dd> <mon> <yyyy> <hh>:<mm>:<ss> <gmt>`; the weekday is redundant and dropped.
    let mut words = value.split_whitespace().peekable();
    if words.peek().is_some_and(|t| t.ends_with(',')) {
        let _ = words.next();
    }
    let (day, month, year, time, zone) = (words.next()?, words.next()?, words.next()?, words.next()?, words.next()?);
    if words.next().is_some()
        || (!zone.eq_ignore_ascii_case("gmt") && !zone.eq_ignore_ascii_case("utc") && zone != "-0000")
    {
        return None;
    }
    let day: i64 = day.parse().ok()?;
    let year: i64 = year.parse().ok()?;
    let month = month.get(..3)?.to_ascii_lowercase();
    let m = MONTHS.iter().position(|x| *x == month)? as i64 + 1;
    let mut nums = time.split(':');
    let (hh, mm, ss) =
        (nums.next()?.parse::<i64>().ok()?, nums.next()?.parse::<i64>().ok()?, nums.next()?.parse::<i64>().ok()?);
    if nums.next().is_some() || !(1..=31).contains(&day) || !(1970..=9999).contains(&year) {
        return None;
    }
    if !(0..=23).contains(&hh) || !(0..=59).contains(&mm) || !(0..=60).contains(&ss) {
        return None;
    }
    let when = days_from_civil(year, m, day) * 86_400 + hh * 3600 + mm * 60 + ss;
    let now = now.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    (when > now).then(|| Duration::from_secs((when - now) as u64))
}

/// A JSON POST. `Debug` hides header values and the body (they can hold cookies and tokens).
/// `Clone` is for the retry loop, which sends the same request again after a wait; nothing in it borrows.
#[derive(Clone)]
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
    user_agent: String,
}

impl ReqwestTransport {
    pub fn new(timeout: Duration, max_response_bytes: usize) -> Result<Self, Error> {
        Self::build(timeout, max_response_bytes, true)
    }

    fn build(timeout: Duration, max_response_bytes: usize, https_only: bool) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .https_only(https_only)
            .gzip(true)
            .build()
            .map_err(|_| Error::Network)?;
        Ok(Self { http, max_bytes: max_response_bytes, user_agent: crate::client::versions::USER_AGENT.to_owned() })
    }

    /// The fixed `User-Agent` sent on `get`. The caller (`ClientBuilder::build`) has validated it.
    pub(crate) fn with_user_agent(mut self, user_agent: &str) -> Self {
        self.user_agent = user_agent.to_owned();
        self
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
                .and_then(|v| parse_retry_after(v, SystemTime::now()));
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
        let mut resp =
            self.http.get(url).header("User-Agent", &self.user_agent).send().await.map_err(|_| Error::Network)?;
        let status = resp.status().as_u16();
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| parse_retry_after(v, SystemTime::now()));
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
    queue: VecDeque<(u16, Option<Duration>, Vec<u8>)>,
}

impl Rule {
    /// The next queued answer; the last one repeats when the queue is one deep.
    fn next(&mut self) -> (u16, Option<Duration>, Vec<u8>) {
        if self.queue.len() > 1 {
            self.queue.pop_front().expect("non-empty")
        } else {
            self.queue.front().cloned().expect("non-empty")
        }
    }
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
        self.rule(endpoint, None, 200, None, serde_json::to_vec(&json).expect("json serializes"))
    }

    /// Like [`with`](Self::with), but only for requests whose body text contains `needle`
    /// (e.g. a continuation token). Put these before the catch-all `with` for the same endpoint.
    pub fn with_match(self, endpoint: &str, needle: &str, json: Value) -> Self {
        self.rule(endpoint, Some(needle), 200, None, serde_json::to_vec(&json).expect("json serializes"))
    }

    /// Answer `endpoint` with a raw status and body (for error and malformed-JSON tests).
    pub fn with_raw(self, endpoint: &str, status: u16, body: &[u8]) -> Self {
        self.rule(endpoint, None, status, None, body.to_vec())
    }

    /// Like [`with_raw`](Self::with_raw), but the answer carries a `Retry-After` delay. Use it to test that a
    /// rate-limited call waits exactly that long.
    pub fn with_retry_after(self, endpoint: &str, status: u16, retry_after: Duration, body: &[u8]) -> Self {
        self.rule(endpoint, None, status, Some(retry_after), body.to_vec())
    }

    /// Answer `endpoint` with 200 and the contents of a JSON file such as `tests/fixtures/x.json`.
    pub fn with_file(self, endpoint: &str, path: impl AsRef<std::path::Path>) -> Self {
        let body = std::fs::read(path.as_ref()).expect("fixture file readable");
        self.rule(endpoint, None, 200, None, body)
    }

    fn rule(
        self,
        endpoint: &str,
        contains: Option<&str>,
        status: u16,
        retry_after: Option<Duration>,
        body: Vec<u8>,
    ) -> Self {
        {
            let mut rules = self.rules.lock().unwrap();
            match rules.iter_mut().find(|r| r.endpoint == endpoint && r.contains.as_deref() == contains) {
                Some(r) => r.queue.push_back((status, retry_after, body)),
                None => rules.push(Rule {
                    endpoint: endpoint.to_owned(),
                    contains: contains.map(str::to_owned),
                    queue: VecDeque::from([(status, retry_after, body)]),
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
            let (status, retry_after, body) = rule.next();
            Ok(Response { status, retry_after, body })
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
            let (status, retry_after, body) = rule.next();
            Ok(Response { status, retry_after, body })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Wed, 21 Oct 2015 07:28:00 GMT`, the example date from the HTTP spec.
    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_445_412_480)
    }

    #[test]
    fn retry_after_delta_seconds() {
        assert_eq!(parse_retry_after("120", now()), Some(Duration::from_secs(120)));
        assert_eq!(parse_retry_after(" 0 ", now()), Some(Duration::ZERO));
        assert_eq!(parse_retry_after("", now()), None);
        assert_eq!(parse_retry_after("-5", now()), None);
        assert_eq!(parse_retry_after("1d", now()), None);
        assert_eq!(parse_retry_after("Wed, 21 Oct 2015", now()), None);
    }

    #[test]
    fn retry_after_imf_fixdate() {
        let at = |s: &str| parse_retry_after(s, now());
        assert_eq!(at("Wed, 21 Oct 2015 07:28:30 GMT"), Some(Duration::from_secs(30)));
        assert_eq!(at("Wed, 21 Oct 2015 07:29:00 utc"), Some(Duration::from_secs(60)));
        assert_eq!(at("21 Oct 2015 07:28:30 -0000"), Some(Duration::from_secs(30))); // weekday omitted
        assert_eq!(at("Wed, 21 Oct 2015 07:28:00 GMT"), None); // now, not later
        assert_eq!(at("Wed, 21 Oct 2015 07:27:00 GMT"), None); // in the past
    }

    #[test]
    fn retry_after_rejects_malformed_dates() {
        let at = |s: &str| parse_retry_after(s, now());
        for bad in [
            "Wed, 21 Xyz 2015 07:28:30 GMT",
            "Wed, 32 Oct 2015 07:28:30 GMT",
            "Wed, 00 Oct 2015 07:28:30 GMT",
            "Wed, 21 Oct 1969 07:28:30 GMT",
            "Wed, 21 Oct 2015 24:00:00 GMT",
            "Wed, 21 Oct 2015 07:60:00 GMT",
            "Wed, 21 Oct 2015 07:28:30 PDT",
            "Wed, 21 Oct 2015 07:28:30 GMT extra",
            "Wed, 21 Oct 2015 07:28 GMT",
        ] {
            assert_eq!(at(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn fixdate_crosses_month_and_leap_day() {
        let at = |s: &str| parse_retry_after(s, UNIX_EPOCH + Duration::from_secs(1_648_512_000)); // 2022-03-29 00:00
        assert_eq!(at("Tue, 29 Mar 2022 00:00:30 GMT"), Some(Duration::from_secs(30)));
        assert_eq!(at("Wed, 30 Mar 2022 00:00:00 GMT"), Some(Duration::from_secs(86_400)));
        assert_eq!(at("Fri, 01 Apr 2022 00:00:00 GMT"), Some(Duration::from_secs(3 * 86_400)));
        // 2024 was a leap year: Feb 29 exists and Mar 1 follows it.
        let leap = |s: &str| parse_retry_after(s, UNIX_EPOCH + Duration::from_secs(1_708_992_000)); // 2024-02-27
        assert_eq!(leap("Wed, 28 Feb 2024 00:00:00 GMT"), Some(Duration::from_secs(86_400)));
        assert_eq!(leap("Thu, 29 Feb 2024 00:00:00 GMT"), Some(Duration::from_secs(2 * 86_400)));
        assert_eq!(leap("Fri, 01 Mar 2024 00:00:00 GMT"), Some(Duration::from_secs(3 * 86_400)));
    }

    /// Plain-http loopback only so the test needs no TLS server; production `new` is always https-only.
    #[tokio::test]
    async fn a_redirect_is_returned_not_followed() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let target_url = format!("http://{}/stolen", target.local_addr().unwrap());
        let first = TcpListener::bind("127.0.0.1:0").unwrap();
        let first_url = format!("http://{}/", first.local_addr().unwrap());
        let served = Arc::new(AtomicUsize::new(0));
        let counter = served.clone();
        std::thread::spawn(move || {
            let (mut conn, _) = first.accept().unwrap();
            let mut buf = [0u8; 2048];
            let _ = conn.read(&mut buf);
            counter.fetch_add(1, Ordering::SeqCst);
            let reply = format!("HTTP/1.1 302 Found\r\nLocation: {target_url}\r\nContent-Length: 0\r\n\r\n");
            conn.write_all(reply.as_bytes()).unwrap();
        });
        let t = ReqwestTransport::build(Duration::from_secs(5), 1 << 16, false).unwrap();
        let resp = t.get(first_url).await.unwrap();
        assert_eq!(resp.status, 302, "the redirect itself is the answer");
        assert_eq!(served.load(Ordering::SeqCst), 1);
        assert!(target.accept().is_err(), "nothing connected to the redirect target");
    }
}
