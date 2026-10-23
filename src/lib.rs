//! `innerlight`: the YouTube and YouTube Music client shared by LunoRX desktop and mobile.
//!
//! Written from scratch under the MIT licence. It must not contain code copied or translated from `rustypipe`
//! (GPL-3.0); see `docs/DESIGN.md` for the clean-room rules.
pub mod ids;

use thiserror::Error;

/// Every failure a caller can see. Messages are fixed text: no URL, cookie or response body ever appears in one.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    #[error("invalid id")]
    InvalidId,
    #[error("network unavailable")]
    Network,
    #[error("sign-in required")]
    Unauthenticated,
    #[error("the service refused the request")]
    Refused,
    #[error("unexpected response")]
    Parse,
}
