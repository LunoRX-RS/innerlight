//! Errors of the `decipher` module. Fixed messages: no script text, URL, signature or `n` value ever appears in one.

use crate::Error;
use crate::player::PlayerError;
use thiserror::Error;

/// Which piece of the player script a structural lookup was about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    /// The signature descrambler (`signatureCipher` -> `s` -> value for `sp`).
    Signature,
    /// The `n` throttling-parameter transform.
    Throttle,
    /// The `signatureTimestamp` constant.
    SignatureTimestamp,
    /// The player version in the loader / script.
    Version,
}

impl Which {
    fn name(self) -> &'static str {
        match self {
            Which::Signature => "signature",
            Which::Throttle => "throttle (n)",
            Which::SignatureTimestamp => "signatureTimestamp",
            Which::Version => "version",
        }
    }
}

impl std::fmt::Display for Which {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum DecipherError {
    /// A request for the loader or the script failed.
    #[error("request failed: {0}")]
    Transport(#[from] Error),
    /// The downloaded script failed validation (host, path, size, encoding).
    #[error("player script rejected")]
    BadScript,
    /// None of the structural heuristics found (and verified) the piece. The script changed shape: add a heuristic
    /// (see `docs/DECIPHER.md`). The input is never returned silently in this case.
    #[error("player script shape changed: could not locate {which}")]
    ScriptShapeChanged { which: Which },
    /// The sandboxed evaluation hit the wall-clock timeout.
    #[error("sandbox timed out")]
    SandboxTimeout,
    /// The sandboxed evaluation hit the loop / recursion budget.
    #[error("sandbox budget exceeded")]
    SandboxBudget,
    /// The sandboxed code threw, or the engine could not run it.
    #[error("sandbox evaluation failed")]
    SandboxFailed,
    /// The input (signature, `n` value, URL) was empty, too long or malformed. Nothing was evaluated.
    #[error("invalid decipher input")]
    BadInput,
    /// The transform produced an empty, over-long, malformed, unchanged or failure-marked value.
    #[error("invalid decipher output")]
    BadOutput,
    /// The rewritten URL failed `player::check_stream_url`.
    #[error("stream url rejected")]
    UnsafeUrl,
}

impl From<DecipherError> for PlayerError {
    /// A transport failure passes through; a rejected URL stays `UnsafeUrl`; everything else is `DecipherFailed`.
    fn from(e: DecipherError) -> PlayerError {
        match e {
            DecipherError::Transport(e) => PlayerError::Transport(e),
            DecipherError::UnsafeUrl => PlayerError::UnsafeUrl,
            _ => PlayerError::DecipherFailed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_and_messages() {
        assert_eq!(PlayerError::from(DecipherError::BadOutput), PlayerError::DecipherFailed);
        assert_eq!(PlayerError::from(DecipherError::UnsafeUrl), PlayerError::UnsafeUrl);
        assert_eq!(PlayerError::from(DecipherError::Transport(Error::Network)), PlayerError::Transport(Error::Network));
        assert_eq!(
            DecipherError::ScriptShapeChanged { which: Which::Throttle }.to_string(),
            "player script shape changed: could not locate throttle (n)"
        );
    }
}
