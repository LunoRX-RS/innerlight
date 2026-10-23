//! `innerlight`: the YouTube and YouTube Music client shared by LunoRX desktop and mobile.
//!
//! Written from scratch under the MIT licence. It must not contain code copied or translated from `rustypipe`
//! (GPL-3.0); see `docs/DESIGN.md` for the clean-room rules.
pub mod account;
pub mod client;
pub mod error;
pub mod ids;
pub mod jsonx;
pub mod session;
pub mod transport;
pub mod youtube;

pub use client::{Client, ClientBuilder, ClientKind};
pub use error::Error;
pub use session::Session;
pub use transport::{FixtureTransport, ReqwestTransport, Transport};
