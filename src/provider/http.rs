//! The one way a [`Provider`](super::Provider) talks to the network: an anonymous bounded GET with the same
//! politeness limits the YouTube client obeys.

use crate::Error;
use crate::policy::{Attempt, Clock, Gate, Policy};
use crate::transport::Transport;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// Longest URL accepted, the same ceiling as a static asset fetch.
const MAX_URL_LEN: usize = 2048;

/// Rejects anything a provider should never be asked to fetch: not https, a userinfo or port hiding where the
/// request really goes, a fragment, a non-hostname, or an over-long URL.
///
/// There is deliberately no host allowlist: a provider is chosen by the application, and this crate's YouTube
/// allowlist (`Client::get_static`) is YouTube-only on purpose.
pub fn check_url(url: &str) -> Result<(), Error> {
    let u = reqwest::Url::parse(url).map_err(|_| Error::InvalidRequest)?;
    let ok = url.len() <= MAX_URL_LEN
        && u.scheme() == "https"
        && u.host_str().is_some_and(|h| h.contains('.') && !h.ends_with('.'))
        && u.username().is_empty()
        && u.password().is_none()
        && u.port().is_none()
        && u.fragment().is_none();
    ok.then_some(()).ok_or(Error::InvalidRequest)
}

/// Percent-encode a whole query term, so a user string can neither close a parameter nor become a path segment.
pub fn encode(q: &str) -> String {
    let mut out = String::with_capacity(q.len());
    for b in q.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A sized, timed, retried GET. Cloneable handle: cheap, and the gate is shared.
#[derive(Clone)]
pub struct Fetcher {
    transport: Arc<dyn Transport>,
    gate: Arc<Gate>,
    timeout: Duration,
    max_bytes: usize,
}

impl std::fmt::Debug for Fetcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Fetcher(..)")
    }
}

impl Fetcher {
    /// `policy` is validated first, so a nonsensical limit fails here rather than on the first request.
    ///
    /// The transport is the provider's own: it never sees a [`Client`](crate::Client) cookie, and traffic to a
    /// third-party host is not mixed into the YouTube client's budget.
    pub fn new(
        transport: Arc<dyn Transport>,
        policy: Policy,
        clock: Arc<dyn Clock>,
        timeout: Duration,
        max_bytes: usize,
    ) -> Result<Self, Error> {
        policy.validate()?;
        if max_bytes == 0 || timeout.is_zero() {
            return Err(Error::InvalidRequest);
        }
        Ok(Self { transport, gate: Arc::new(Gate::new(policy, clock)), timeout, max_bytes })
    }

    /// The limits this fetcher obeys.
    pub fn policy(&self) -> Policy {
        self.gate.policy()
    }

    /// Fetch one URL as bytes. Non-2xx answers become the typed [`Error`] the status says, a
    /// challenge body is named rather than retried, and only transient failures are resent.
    pub async fn get(&self, url: &str) -> Result<Vec<u8>, Error> {
        check_url(url)?;
        let attempt = Attempt::Get { transport: self.transport.clone(), url: url.to_owned() };
        let response = self.gate.dispatch(attempt, self.timeout).await?;
        if response.body.len() > self.max_bytes {
            return Err(Error::TooLarge);
        }
        Ok(response.body)
    }

    /// [`get`](Self::get) plus a JSON parse. An answer that is not an object is [`Error::Parse`].
    pub async fn json(&self, url: &str) -> Result<Value, Error> {
        let body = self.get(url).await?;
        serde_json::from_slice::<Value>(&body).map_err(|_| Error::Parse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FakeClock;
    use crate::transport::FixtureTransport;

    fn fetcher(fx: FixtureTransport, max_bytes: usize) -> Fetcher {
        Fetcher::new(Arc::new(fx), Policy::no_retry(), Arc::new(FakeClock::new()), Duration::from_secs(5), max_bytes)
            .unwrap()
    }

    #[tokio::test]
    async fn only_plain_https_gets_fetched() {
        let fx = FixtureTransport::new().with_raw("a", 200, b"{}");
        for bad in [
            "http://example.com/a",
            "https://user:pw@example.com/a",
            "https://example.com:8443/a",
            "https://example.com/a#frag",
            "https://localhost/a",
            "file:///etc/passwd",
            "not a url",
        ] {
            assert_eq!(check_url(bad), Err(Error::InvalidRequest), "{bad}");
        }
        assert_eq!(check_url("https://example.com/"), Ok(()));
        assert_eq!(check_url("https://example.com/a"), Ok(()));
        assert_eq!(check_url("https://example.com/a?x=1&y=2"), Ok(()));
        // a rejected URL is reported as a bad request, and nothing is sent
        let f = fetcher(fx, 1024);
        assert_eq!(f.get("http://example.com/a").await.unwrap_err(), Error::InvalidRequest);
    }

    #[tokio::test]
    async fn a_body_over_the_cap_is_too_large() {
        let fx = || FixtureTransport::new().with_raw("a", 200, &[b'x'; 40]);
        assert_eq!(fetcher(fx(), 32).get("https://example.com/a").await.unwrap_err(), Error::TooLarge);
        assert_eq!(fetcher(fx(), 64).get("https://example.com/a").await.unwrap().len(), 40);
    }

    #[test]
    fn encoding_leaves_nothing_to_escape() {
        assert_eq!(encode("hello world-1_2.3~4"), "hello+world-1_2.3~4");
        assert_eq!(encode("a&b=c/d?e"), "a%26b%3Dc%2Fd%3Fe");
        assert_eq!(encode("\" OR x:*"), "%22+OR+x%3A%2A");
        assert_eq!(encode("🎵"), "%F0%9F%8E%B5");
        assert_eq!(encode(""), "");
    }

    #[test]
    fn a_fetcher_refuses_a_policy_that_could_spin() {
        let fx: Arc<dyn Transport> = Arc::new(FixtureTransport::new());
        assert!(
            Fetcher::new(fx.clone(), Policy::no_retry(), Arc::new(FakeClock::new()), Duration::from_secs(1), 0)
                .is_err()
        );
        assert!(
            Fetcher::new(
                fx,
                Policy { requests_per_second: 0, ..Policy::polite() },
                Arc::new(FakeClock::new()),
                Duration::from_secs(1),
                10
            )
            .is_err()
        );
    }
}
