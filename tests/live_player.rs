//! LIVE, manual, off by default. One standard `player` request to the real service, no retry, no client switch.
//!
//! A human runs it deliberately:
//!
//! ```text
//! INNERLIGHT_LIVE=1 INNERLIGHT_VIDEO=<11-char public video id> [INNERLIGHT_COOKIE='<Cookie header>'] \
//!     [INNERLIGHT_KIND=music] cargo test --test live_player -- --ignored --nocapture
//! ```
//!
//! Prints the typed outcome only: never a URL, never the cookie. Without `INNERLIGHT_LIVE=1` it does nothing.

use innerlight::player::{Player, ResolveOptions};
use innerlight::{Client, ClientKind};

#[tokio::test]
#[ignore = "talks to the real service; run deliberately, see the file header"]
async fn one_live_player_request() {
    if std::env::var("INNERLIGHT_LIVE").as_deref() != Ok("1") {
        println!("skipped: set INNERLIGHT_LIVE=1 to make one real request");
        return;
    }
    let Ok(video) = std::env::var("INNERLIGHT_VIDEO") else {
        println!("skipped: set INNERLIGHT_VIDEO to a public video id");
        return;
    };
    let client = Client::builder().build().unwrap();
    let authed = match std::env::var("INNERLIGHT_COOKIE") {
        Ok(c) if !c.is_empty() => {
            client.set_cookie(&c).expect("cookie string rejected");
            true
        }
        _ => false,
    };
    let kind =
        if std::env::var("INNERLIGHT_KIND").as_deref() == Ok("music") { ClientKind::WebMusic } else { ClientKind::Web };
    let opts = ResolveOptions { kind, authed, ..ResolveOptions::default() };
    // `Resolved`'s Debug and `PlayerError`'s Display/Debug contain no URL, cookie or response text.
    match Player::new(&client).resolve(&video, opts).await {
        Ok(r) => println!("live outcome: resolved {r:?}"),
        Err(e) => println!("live outcome: {e:?} ({e})"),
    }
}
