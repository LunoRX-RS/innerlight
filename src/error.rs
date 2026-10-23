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
    /// Connect failure, timeout, TLS failure, or a 5xx answer.
    #[error("network unavailable")]
    Network,
    /// HTTP 401, or an authed call without a usable cookie session.
    #[error("sign-in required")]
    Unauthenticated,
    /// HTTP 403 or any other unexpected non-success status.
    #[error("the service refused the request")]
    Refused,
    /// HTTP 429. `retry_after` is the `Retry-After` header in whole seconds, when the service sent one.
    #[error("rate limited")]
    RateLimited { retry_after: Option<Duration> },
    /// The response body was not valid JSON or lacked the expected shape.
    #[error("unexpected response")]
    Parse,
    /// The response body exceeded the configured size limit.
    #[error("response too large")]
    TooLarge,
}

impl Error {
    /// Maps a non-2xx HTTP status to an error. Call only for statuses outside `200..300`.
    pub fn from_status(status: u16, retry_after: Option<Duration>) -> Error {
        match status {
            401 => Error::Unauthenticated,
            429 => Error::RateLimited { retry_after },
            500..=599 => Error::Network,
            _ => Error::Refused,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_mapping() {
        assert_eq!(Error::from_status(401, None), Error::Unauthenticated);
        assert_eq!(Error::from_status(403, None), Error::Refused);
        assert_eq!(Error::from_status(404, None), Error::Refused);
        assert_eq!(Error::from_status(302, None), Error::Refused);
        assert_eq!(Error::from_status(500, None), Error::Network);
        assert_eq!(Error::from_status(503, None), Error::Network);
        let d = Some(Duration::from_secs(7));
        assert_eq!(Error::from_status(429, d), Error::RateLimited { retry_after: d });
    }

    #[test]
    fn messages_are_fixed() {
        assert_eq!(Error::RateLimited { retry_after: Some(Duration::from_secs(1)) }.to_string(), "rate limited");
        assert_eq!(Error::TooLarge.to_string(), "response too large");
    }
}
