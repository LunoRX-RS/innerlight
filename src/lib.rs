//! `innerlight`: the YouTube and YouTube Music client shared by LunoRX desktop and mobile.
//!
//! Polite by default: a `429`, `5xx`, timeout or connect failure is waited out under [`Policy`] with the concurrency and
//! rate caps it names, a `Retry-After` is honoured exactly rather than truncated, and a bot or login challenge is reported
//! as [`Error::BotCheck`] instead of worked around.
//!
//! Written from scratch under the MIT licence. It must not contain code copied or translated from `rustypipe`
//! (GPL-3.0); see `docs/DESIGN.md` for the clean-room rules.
pub mod account;
pub mod client;
pub mod decipher;
pub mod error;
pub mod ids;
pub mod jsonx;
pub mod music;
pub mod player;
pub mod policy;
pub mod session;
pub mod transport;
pub mod youtube;

pub use client::{Client, ClientBuilder, ClientKind};
pub use error::Error;
pub use policy::{Clock, FakeClock, Policy, TokioClock, spread};
pub use session::Session;
pub use transport::{FixtureTransport, ReqwestTransport, Transport};
