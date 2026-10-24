//! The player's error type. Messages are fixed text: no URL, cookie, signature or response text ever appears.

use super::model::UnplayableReason;
use crate::Error;
use thiserror::Error;

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum PlayerError {
    #[error("invalid video id")]
    InvalidId,
    /// The request itself failed (network, HTTP status, rate limit, not signed in for an authed call, ...).
    #[error("request failed: {0}")]
    Transport(#[from] Error),
    /// The service demands a sign-in. `bot_check` is true for the "confirm you're not a bot" challenge.
    /// This crate stops here: no retry, no other client, no workaround.
    #[error("sign-in required")]
    LoginRequired { bot_check: bool },
    #[error("age-restricted video")]
    AgeRestricted,
    #[error("video is not playable")]
    Unplayable { reason: UnplayableReason },
    #[error("live stream")]
    Live,
    #[error("video unavailable")]
    Unavailable,
    /// `playabilityStatus` was something else (ERROR, unknown).
    #[error("the service reported an error")]
    ServiceError,
    /// The answer lacked the expected shape.
    #[error("unexpected response")]
    Parse,
    /// Status was OK but there is no usable audio stream (none, DRM only, empty).
    #[error("no usable audio stream")]
    NoAudio,
    /// The only usable stream is ciphered and no [`Decipher`](super::Decipher) was supplied.
    #[error("stream needs a signature decipher")]
    NeedsDecipher,
    /// A [`Decipher`](super::Decipher) failed or produced unusable output.
    #[error("signature decipher failed")]
    DecipherFailed,
    /// A stream URL failed [`check_stream_url`](super::check_stream_url).
    #[error("stream url rejected")]
    UnsafeUrl,
}

impl From<PlayerError> for Error {
    /// Lossy mapping into the transport error: a bot challenge becomes [`Error::BotCheck`], a plain sign-in wall
    /// `Unauthenticated`, every other refusal `Refused`, shape problems `Parse`, transport errors pass through.
    fn from(e: PlayerError) -> Error {
        match e {
            PlayerError::Transport(e) => e,
            PlayerError::InvalidId => Error::InvalidId,
            PlayerError::LoginRequired { bot_check: true } => Error::BotCheck,
            PlayerError::LoginRequired { bot_check: false } => Error::Unauthenticated,
            PlayerError::AgeRestricted
            | PlayerError::Unplayable { .. }
            | PlayerError::Live
            | PlayerError::Unavailable
            | PlayerError::ServiceError => Error::Refused,
            PlayerError::Parse
            | PlayerError::NoAudio
            | PlayerError::NeedsDecipher
            | PlayerError::DecipherFailed
            | PlayerError::UnsafeUrl => Error::Parse,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_and_messages() {
        assert_eq!(PlayerError::from(Error::Network), PlayerError::Transport(Error::Network));
        assert_eq!(Error::from(PlayerError::Transport(Error::TooLarge)), Error::TooLarge);
        assert_eq!(Error::from(PlayerError::LoginRequired { bot_check: true }), Error::BotCheck);
        assert_eq!(Error::from(PlayerError::LoginRequired { bot_check: false }), Error::Unauthenticated);
        assert_eq!(Error::from(PlayerError::Live), Error::Refused);
        assert_eq!(Error::from(PlayerError::NeedsDecipher), Error::Parse);
        assert_eq!(PlayerError::UnsafeUrl.to_string(), "stream url rejected");
        assert_eq!(PlayerError::Transport(Error::Network).to_string(), "request failed: network unavailable");
    }
}
