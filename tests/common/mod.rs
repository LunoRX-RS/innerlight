//! Shared scaffolding for the offline tests. `builder` is `Client::builder` on a clock the test owns, so the
//! rate cap and the retry backoff move simulated time instead of making the suite wait for real.
#![allow(dead_code)] // each test crate uses a different subset

use innerlight::{Client, ClientBuilder, FakeClock};
use std::sync::Arc;

/// A builder whose waits happen on a fresh fake clock, starting at zero.
pub fn builder() -> ClientBuilder {
    let clock = Arc::new(FakeClock::new());
    Client::builder().clock(clock)
}

/// A builder plus the clock it runs on, for asserting how long the client said it would wait.
pub fn builder_with_clock() -> (ClientBuilder, Arc<FakeClock>) {
    let clock = Arc::new(FakeClock::new());
    (Client::builder().clock(clock.clone()), clock)
}
