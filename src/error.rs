//! The one error type every public function returns.

use std::time::Duration;
use thiserror::Error;

/// Every failure a caller can see. Messages are fixed text: no URL, cookie or response body ever appears in one.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    #[error("invalid id")]
    InvalidId,
    /// The endpoint string or request body handed to the client was malformed (a bug in the caller).
    #[error("invalid request")]
    InvalidRequest,
    /// Connect failure or TLS failure. A deadline that elapsed is [`Error::Timeout`], a 5xx answer
    /// [`Error::ServerError`].
    #[error("network unavailable")]
    Network,
    /// HTTP 401, or an authed call without a usable cookie session.
    #[error("sign-in required")]
    Unauthenticated,
    /// HTTP 403, 404 or any other unexpected non-success status.
    #[error("the service refused the request")]
    Refused,
    /// HTTP 429. `retry_after` is the `Retry-After` header in whole seconds, when the service sent one.
    #[error("rate limited")]
    RateLimited { retry_after: Option<Duration> },
    /// The service demanded proof the caller is not a bot: the "Sign in to confirm you're not a bot" or
    /// "unusual traffic from your network" challenge.
    ///
    /// Reported as the reason the call stopped. This crate never retries a challenge, never switches to another
    /// client identity and never forges a proof-of-origin token to get past it; sign in with your own cookie.
    #[error("the service asked to verify you are not a bot")]
    BotCheck,
    /// The service says this caller, project or account is over its quota. Backing off for a few seconds does
    /// not clear it, so this is never retried.
    #[error("quota exceeded")]
    QuotaExceeded,
    /// HTTP 5xx: the service had an internal error.
    #[error("the service had an internal error")]
    ServerError,
    /// The per-request deadline elapsed before the transport answered.
    #[error("request timed out")]
    Timeout,
    /// The response body was not valid JSON or lacked the expected shape.
    #[error("unexpected response")]
    Parse,
    /// The response body exceeded the configured size limit.
    #[error("response too large")]
    TooLarge,
}

/// Bytes of a refusal body searched for challenge markers. The scan is a bounded prefix: a hostile body of any
/// size costs a fixed amount of work.
const MARKER_WINDOW: usize = 64 * 1024;

/// Case-insensitive substring search over ASCII bytes.
fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.len() >= needle.len() && hay.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}

impl Error {
    /// Maps a non-2xx HTTP status to an error. Call only for statuses outside `200..300`.
    pub fn from_status(status: u16, retry_after: Option<Duration>) -> Error {
        match status {
            401 => Error::Unauthenticated,
            429 => Error::RateLimited { retry_after },
            500..=599 => Error::ServerError,
            _ => Error::Refused,
        }
    }

    /// [`from_status`](Self::from_status), but a 400-403 or 429 body that carries one of the service's own
    /// challenge or quota phrases reports that instead of a bare refusal. Pure reporting: a detected challenge
    /// is never retried and no workaround is attempted.
    pub fn classify(status: u16, retry_after: Option<Duration>, body: &[u8]) -> Error {
        if matches!(status, 400..=403 | 429)
            && let Some(e) = Self::detect_markers(&body[..body.len().min(MARKER_WINDOW)])
        {
            return e;
        }
        Self::from_status(status, retry_after)
    }

    /// The challenge or quota phrase in a refusal body, if it holds one. Never fails and never panics.
    pub fn detect_markers(body: &[u8]) -> Option<Error> {
        for marker in SELF_CHECK_MARKERS {
            if contains(body, marker) {
                return Some(Error::BotCheck);
            }
        }
        for marker in QUOTA_MARKERS {
            if contains(body, marker) {
                return Some(Error::QuotaExceeded);
            }
        }
        None
    }

    /// True when the same request may be tried again after a wait: a rate limit, a 5xx, an expired deadline or a
    /// connect failure. A challenge, a quota, a refusal, a missing sign-in and a bad answer are all `false`, so a
    /// caller never hammers something a retry cannot fix.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Error::RateLimited { .. } | Error::ServerError | Error::Timeout | Error::Network)
    }

    /// The delay the service asked for, when it sent one. Only [`RateLimited`](Self::RateLimited) carries it.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Error::RateLimited { retry_after } => *retry_after,
            _ => None,
        }
    }
}

/// Phrases of the bot / unusual-traffic challenge, as the service spells them in refusal bodies.
const SELF_CHECK_MARKERS: &[&[u8]] =
    &[b"confirm you're not a bot", b"not a bot", b"unusual traffic", b"automated queries"];

/// Phrases of a quota refusal. Narrow on purpose: it only refines a status that already failed.
const QUOTA_MARKERS: &[&[u8]] = &[b"exceeded your current quota", b"quota exceeded", b"rate limit exceeded"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_mapping() {
        assert_eq!(Error::from_status(401, None), Error::Unauthenticated);
        assert_eq!(Error::from_status(403, None), Error::Refused);
        assert_eq!(Error::from_status(404, None), Error::Refused);
        assert_eq!(Error::from_status(302, None), Error::Refused);
        assert_eq!(Error::from_status(500, None), Error::ServerError);
        assert_eq!(Error::from_status(503, None), Error::ServerError);
        let d = Some(Duration::from_secs(7));
        assert_eq!(Error::from_status(429, d), Error::RateLimited { retry_after: d });
    }

    #[test]
    fn messages_are_fixed() {
        assert_eq!(Error::RateLimited { retry_after: Some(Duration::from_secs(1)) }.to_string(), "rate limited");
        assert_eq!(Error::TooLarge.to_string(), "response too large");
        assert_eq!(Error::BotCheck.to_string(), "the service asked to verify you are not a bot");
        assert_eq!(Error::QuotaExceeded.to_string(), "quota exceeded");
        assert_eq!(Error::Timeout.to_string(), "request timed out");
        assert_eq!(Error::ServerError.to_string(), "the service had an internal error");
    }

    #[test]
    fn classify_reads_the_challenge_out_of_a_refusal_body() {
        let body = br#"{"error":{"code":403,"message":"Sign in to confirm you're not a bot"}}"#;
        assert_eq!(Error::classify(403, None, body), Error::BotCheck);
        assert_eq!(Error::classify(429, None, body), Error::BotCheck);
        assert_eq!(Error::classify(403, None, b"unusual traffic from your network"), Error::BotCheck);
        // Quota beats a bare rate limit so the caller knows a short backoff will not help.
        let quota = br#"{"error":{"message":"You have exceeded your current quota"}}"#;
        assert_eq!(Error::classify(429, Some(Duration::from_secs(5)), quota), Error::QuotaExceeded);
    }

    #[test]
    fn classify_falls_back_to_the_status() {
        assert_eq!(Error::classify(403, None, b"forbidden"), Error::Refused);
        assert_eq!(Error::classify(401, None, b""), Error::Unauthenticated);
        assert_eq!(
            Error::classify(429, Some(Duration::from_secs(2)), b"slow down"),
            Error::RateLimited { retry_after: Some(Duration::from_secs(2)) }
        );
        // A marker in a status that is not a refusal stays mapped by status: the scan is only for 400-403 and 429.
        assert_eq!(Error::classify(503, None, b"confirm you're not a bot"), Error::ServerError);
        assert_eq!(Error::classify(404, None, b"quota exceeded"), Error::Refused);
    }

    #[test]
    fn detect_markers_is_case_insensitive_and_never_fails() {
        assert_eq!(Error::detect_markers(b""), None);
        assert_eq!(Error::detect_markers(b"NOT A BOT"), Some(Error::BotCheck));
        assert_eq!(Error::detect_markers(b"x Quota Exceeded y"), Some(Error::QuotaExceeded));
        // Bot markers win over quota markers.
        assert_eq!(Error::detect_markers(b"quota exceeded: confirm you're not a bot"), Some(Error::BotCheck));
        assert_eq!(Error::detect_markers(&[b'a'; 4096]), None);
    }

    #[test]
    fn only_transient_failures_are_retryable() {
        for e in [Error::RateLimited { retry_after: None }, Error::ServerError, Error::Timeout, Error::Network] {
            assert!(e.is_retryable(), "{e:?}");
        }
        for e in [
            Error::BotCheck,
            Error::QuotaExceeded,
            Error::Unauthenticated,
            Error::Refused,
            Error::Parse,
            Error::TooLarge,
            Error::InvalidId,
            Error::InvalidRequest,
        ] {
            assert!(!e.is_retryable(), "{e:?}");
        }
    }

    #[test]
    fn retry_after_only_comes_from_a_rate_limit() {
        let d = Duration::from_secs(9);
        assert_eq!(Error::RateLimited { retry_after: Some(d) }.retry_after(), Some(d));
        assert_eq!(Error::RateLimited { retry_after: None }.retry_after(), None);
        for e in [Error::BotCheck, Error::QuotaExceeded, Error::ServerError, Error::Timeout, Error::Network] {
            assert_eq!(e.retry_after(), None, "{e:?}");
        }
    }
}
