//! PO tokens are supplied by the host, never made here: attached only on request, validated, never leaked.

mod common;
use common::builder;
use innerlight::player::{Player, PlayerError, PoTokenSource, ResolveOptions};
use innerlight::{Error, FixtureTransport};
use serde_json::json;
use std::sync::Arc;

const VID: &str = "dQw4w9WgXcQ";
const TOKEN: &str = "MnQx_synthetic-token-AAAA-BBBB_cccc=";

struct Fixed(Option<&'static str>);
impl PoTokenSource for Fixed {
    fn token_for(&self, _: &str) -> Option<String> {
        self.0.map(str::to_owned)
    }
}

fn challenge() -> FixtureTransport {
    FixtureTransport::new().with(
        "player",
        json!({
            "responseContext": {},
            "playabilityStatus": {"status": "LOGIN_REQUIRED", "reason": "Sign in to confirm you're not a bot"}
        }),
    )
}

#[tokio::test]
async fn no_source_means_no_token_on_the_wire() {
    let fx = Arc::new(challenge());
    let c = builder().transport(fx.clone()).build().unwrap();
    let _ = Player::new(&c).resolve(VID, ResolveOptions::default()).await;
    assert!(fx.requests()[0].body.get("serviceIntegrityDimensions").is_none());
}

#[tokio::test]
async fn a_host_token_rides_one_request_and_a_challenge_still_stops_there() {
    let fx = Arc::new(challenge());
    let c = builder().transport(fx.clone()).build().unwrap();
    let p = Player::new(&c).with_po_token_source(Arc::new(Fixed(Some(TOKEN))));
    let err = p.resolve(VID, ResolveOptions::default()).await.unwrap_err();
    assert_eq!(err, PlayerError::LoginRequired { bot_check: true }, "a token does not turn a challenge into a retry");
    let reqs = fx.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].body["serviceIntegrityDimensions"]["poToken"], TOKEN);
    assert!(reqs[0].header("user-agent").is_some_and(|u| u.starts_with("innerlight/")), "same identity as ever");
    // Nothing printable carries it.
    assert!(!format!("{p:?}{err:?}{err}").contains(TOKEN));
}

#[tokio::test]
async fn a_source_with_nothing_to_give_sends_the_plain_request() {
    let fx = Arc::new(challenge());
    let c = builder().transport(fx.clone()).build().unwrap();
    let p = Player::new(&c).with_po_token_source(Arc::new(Fixed(None)));
    let _ = p.resolve(VID, ResolveOptions::default()).await;
    assert!(fx.requests()[0].body.get("serviceIntegrityDimensions").is_none());
}

#[tokio::test]
async fn an_implausible_token_is_refused_before_anything_is_sent() {
    for bad in ["", "short", "has space in it 0123456789", "quote\"inject0123456789"] {
        let fx = Arc::new(challenge());
        let c = builder().transport(fx.clone()).build().unwrap();
        let leaked: &'static str = Box::leak(bad.to_owned().into_boxed_str());
        let p = Player::new(&c).with_po_token_source(Arc::new(Fixed(Some(leaked))));
        let err = p.resolve(VID, ResolveOptions::default()).await.unwrap_err();
        assert_eq!(err, PlayerError::InvalidPoToken, "{bad:?}");
        assert!(fx.requests().is_empty(), "{bad:?}");
        assert_eq!(Error::from(err), Error::InvalidRequest);
    }
}
