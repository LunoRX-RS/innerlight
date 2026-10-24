//! Offline tests for `innerlight::account`: exact endpoints and bodies, auth gate, error mapping.

mod common;
use common::builder;
use innerlight::account::{Account, Privacy};
use innerlight::{Client, Error, FixtureTransport};
use serde_json::json;
use std::sync::Arc;

const VID: &str = "dQw4w9WgXcQ";
const CHAN: &str = "UCuAXFkgsw1L7xaCfnd5JJOw";
const PL: &str = "PLrAXtmErZgOeiKm4sgNOknGvNjby9efdf";

fn fx(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// A client whose every listed endpoint answers `file`.
fn setup(endpoints: &[&str], file: &str, cookie: bool) -> (Client, Arc<FixtureTransport>) {
    let t = endpoints.iter().fold(FixtureTransport::new(), |t, e| t.with_file(e, fx(file)));
    let t = Arc::new(t);
    let c = builder().transport(t.clone()).build().unwrap();
    if cookie {
        c.set_cookie("SAPISID=abc123; SID=x").unwrap();
    }
    (c, t)
}

#[tokio::test]
async fn like_family_sends_exact_endpoint_and_target() {
    let (c, t) = setup(&["like/like", "like/removelike", "like/dislike"], "account_handwritten_ok.json", true);
    let a = Account::new(&c);
    a.like_video(VID).await.unwrap();
    a.unlike_video(VID).await.unwrap();
    a.dislike_video(VID).await.unwrap();
    let reqs = t.requests();
    let eps: Vec<_> = reqs.iter().map(|r| r.endpoint.as_str()).collect();
    assert_eq!(eps, ["like/like", "like/removelike", "like/dislike"]);
    for r in &reqs {
        assert_eq!(r.host, "www.youtube.com");
        assert_eq!(r.body["target"], json!({"videoId": VID}));
        assert_eq!(r.body["context"]["client"]["clientName"], "WEB");
        assert!(r.header("authorization").is_some_and(|h| h.starts_with("SAPISIDHASH ")));
    }
}

#[tokio::test]
async fn subscribe_family() {
    let (c, t) = setup(&["subscription/subscribe", "subscription/unsubscribe"], "account_handwritten_ok.json", true);
    let a = Account::new(&c);
    a.subscribe(CHAN).await.unwrap();
    a.unsubscribe(CHAN).await.unwrap();
    let reqs = t.requests();
    assert_eq!(reqs[0].endpoint, "subscription/subscribe");
    assert_eq!(reqs[1].endpoint, "subscription/unsubscribe");
    for r in &reqs {
        assert_eq!(r.body["channelIds"], json!([CHAN]));
    }
}

#[tokio::test]
async fn playlist_edits() {
    let (c, t) = setup(&["browse/edit_playlist", "playlist/delete"], "account_handwritten_edit_ok.json", true);
    let a = Account::new(&c);
    a.add_to_playlist(PL, VID).await.unwrap();
    a.add_to_playlist("WL", VID).await.unwrap();
    a.remove_from_playlist(PL, VID, Some("A1B2C3D4E5F6")).await.unwrap();
    a.remove_from_playlist(PL, VID, None).await.unwrap();
    a.delete_playlist(PL).await.unwrap();
    let r = t.requests();
    assert_eq!(r[0].endpoint, "browse/edit_playlist");
    assert_eq!(r[0].body["playlistId"], PL);
    assert_eq!(r[0].body["actions"], json!([{"action": "ACTION_ADD_VIDEO", "addedVideoId": VID}]));
    assert_eq!(r[1].body["playlistId"], "WL");
    assert_eq!(
        r[2].body["actions"],
        json!([{"action": "ACTION_REMOVE_VIDEO", "removedVideoId": VID, "setVideoId": "A1B2C3D4E5F6"}])
    );
    assert_eq!(r[3].body["actions"], json!([{"action": "ACTION_REMOVE_VIDEO_BY_VIDEO_ID", "removedVideoId": VID}]));
    assert_eq!((r[4].endpoint.as_str(), &r[4].body["playlistId"]), ("playlist/delete", &json!(PL)));
}

#[tokio::test]
async fn create_playlist_returns_id() {
    let (c, t) = setup(&["playlist/create"], "account_handwritten_create_ok.json", true);
    let id = Account::new(&c).create_playlist("Road trip", Privacy::Unlisted, &[VID]).await.unwrap();
    assert_eq!(id, PL);
    let r = &t.requests()[0];
    assert_eq!(r.endpoint, "playlist/create");
    assert_eq!(r.body["title"], "Road trip");
    assert_eq!(r.body["privacyStatus"], "UNLISTED");
    assert_eq!(r.body["videoIds"], json!([VID]));

    // success without a playlist id cannot be reported as success
    let (c, _) = setup(&["playlist/create"], "account_handwritten_ok.json", true);
    assert_eq!(Account::new(&c).create_playlist("x", Privacy::Private, &[]).await.unwrap_err(), Error::Parse);
}

#[tokio::test]
async fn unauthenticated_sends_nothing() {
    let all = [
        "like/like",
        "like/removelike",
        "like/dislike",
        "subscription/subscribe",
        "subscription/unsubscribe",
        "browse/edit_playlist",
        "playlist/create",
        "playlist/delete",
    ];
    let (c, t) = setup(&all, "account_handwritten_ok.json", false);
    let a = Account::new(&c);
    assert_eq!(a.like_video(VID).await, Err(Error::Unauthenticated));
    assert_eq!(a.unlike_video(VID).await, Err(Error::Unauthenticated));
    assert_eq!(a.dislike_video(VID).await, Err(Error::Unauthenticated));
    assert_eq!(a.subscribe(CHAN).await, Err(Error::Unauthenticated));
    assert_eq!(a.unsubscribe(CHAN).await, Err(Error::Unauthenticated));
    assert_eq!(a.add_to_playlist(PL, VID).await, Err(Error::Unauthenticated));
    assert_eq!(a.remove_from_playlist(PL, VID, None).await, Err(Error::Unauthenticated));
    assert_eq!(a.create_playlist("t", Privacy::Public, &[]).await, Err(Error::Unauthenticated));
    assert_eq!(a.delete_playlist(PL).await, Err(Error::Unauthenticated));
    assert!(t.requests().is_empty());
}

#[tokio::test]
async fn bad_ids_are_rejected_before_anything_else() {
    let all = ["like/like", "subscription/subscribe", "browse/edit_playlist", "playlist/create", "playlist/delete"];
    // signed in, so only validation can stop these
    let (c, t) = setup(&all, "account_handwritten_ok.json", true);
    let a = Account::new(&c);
    for bad in ["", "short", "aqz-KE-bpKQx", "has space!!"] {
        assert_eq!(a.like_video(bad).await, Err(Error::InvalidId), "{bad:?}");
    }
    for bad in ["", "UCshort", "XXuAXFkgsw1L7xaCfnd5JJOw"] {
        assert_eq!(a.subscribe(bad).await, Err(Error::InvalidId), "{bad:?}");
    }
    for bad in ["", "a/b", "VLPLrAXtmErZgOeiKm4sgNOknGvNjby9efdf", "P"] {
        assert_eq!(a.add_to_playlist(bad, VID).await, Err(Error::InvalidId), "{bad:?}");
    }
    assert_eq!(a.add_to_playlist(PL, "bad").await, Err(Error::InvalidId));
    assert_eq!(a.remove_from_playlist(PL, VID, Some("")).await, Err(Error::InvalidId));
    assert_eq!(a.remove_from_playlist(PL, VID, Some("a b\"")).await, Err(Error::InvalidId));
    assert_eq!(a.delete_playlist("WL").await, Err(Error::InvalidId));
    assert_eq!(a.delete_playlist("LL").await, Err(Error::InvalidId));
    assert_eq!(a.create_playlist("t", Privacy::Private, &["bad"]).await, Err(Error::InvalidId));
    for bad in ["", "   ", "line\nbreak", &"y".repeat(151)] {
        assert_eq!(a.create_playlist(bad, Privacy::Private, &[]).await, Err(Error::InvalidRequest), "{bad:?}");
    }
    assert!(t.requests().is_empty());
}

#[tokio::test]
async fn service_error_object_maps_to_refused() {
    let (c, t) = setup(&["like/like", "browse/edit_playlist"], "account_handwritten_error.json", true);
    let a = Account::new(&c);
    assert_eq!(a.like_video(VID).await, Err(Error::Refused));
    assert_eq!(a.add_to_playlist(PL, VID).await, Err(Error::Refused));
    assert_eq!(t.requests().len(), 2);
}

#[tokio::test]
async fn missing_response_context_or_non_object_is_parse_error() {
    let (c, _) = setup(&["like/like"], "account_handwritten_no_context.json", true);
    assert_eq!(Account::new(&c).like_video(VID).await, Err(Error::Parse));
    for body in [&b"[]"[..], b"null", b"\"ok\"", b"not json"] {
        let t = Arc::new(FixtureTransport::new().with_raw("like/like", 200, body));
        let c = builder().transport(t).build().unwrap();
        c.set_cookie("SAPISID=abc123").unwrap();
        assert_eq!(Account::new(&c).like_video(VID).await, Err(Error::Parse));
    }
}

#[tokio::test]
async fn http_failures_pass_through_the_transport_mapping() {
    for (status, want) in [(401u16, Error::Unauthenticated), (403, Error::Refused), (500, Error::ServerError)] {
        let t = Arc::new(FixtureTransport::new().with_raw("like/like", status, b"{}"));
        let c = builder().transport(t).build().unwrap();
        c.set_cookie("SAPISID=abc123").unwrap();
        assert_eq!(Account::new(&c).like_video(VID).await, Err(want));
    }
}
