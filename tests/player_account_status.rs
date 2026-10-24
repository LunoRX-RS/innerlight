//! Regression: a 2xx answer carrying `"status": "STATUS_FAILED"` is a refusal, not a success (src/account.rs).

mod common;
use common::builder;
use innerlight::account::Account;
use innerlight::{Client, Error, FixtureTransport};
use serde_json::json;
use std::sync::Arc;

const VID: &str = "dQw4w9WgXcQ";
const PL: &str = "PLrAXtmErZgOeiKm4sgNOknGvNjby9efdf";

fn client(t: FixtureTransport) -> Client {
    let c = builder().transport(Arc::new(t)).build().unwrap();
    c.set_cookie("SAPISID=abc123; SID=x").unwrap();
    c
}

#[tokio::test]
async fn status_failed_fixture_is_refused() {
    let path = format!("{}/tests/fixtures/player_handwritten_account_status_failed.json", env!("CARGO_MANIFEST_DIR"));
    let c = client(FixtureTransport::new().with_file("browse/edit_playlist", path));
    assert_eq!(Account::new(&c).add_to_playlist(PL, VID).await, Err(Error::Refused));
}

#[tokio::test]
async fn any_fail_status_is_refused_and_success_still_passes() {
    for (status, ok) in
        [("STATUS_FAILED", false), ("status_failed", false), ("FAILURE", false), ("STATUS_SUCCEEDED", true), ("", true)]
    {
        let c = client(FixtureTransport::new().with("like/like", json!({"responseContext": {}, "status": status})));
        let r = Account::new(&c).like_video(VID).await;
        assert_eq!(r.is_ok(), ok, "{status}: {r:?}");
        if !ok {
            assert_eq!(r, Err(Error::Refused));
        }
    }
    // a non-string status is ignored
    let c = client(FixtureTransport::new().with("like/like", json!({"responseContext": {}, "status": 3})));
    assert!(Account::new(&c).like_video(VID).await.is_ok());
}
