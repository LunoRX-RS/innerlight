//! Offline tests for `innerlight::youtube`. Fixtures are hand-written (`yt_handwritten_*`), not recordings.

use innerlight::youtube::{Youtube, parse_playlist_page, parse_video_page};
use innerlight::{Client, Error, FixtureTransport};
use serde_json::{Value, json};
use std::sync::Arc;

fn fx(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn load(name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(fx(name)).unwrap()).unwrap()
}

fn client(t: FixtureTransport, cookie: bool) -> (Client, Arc<FixtureTransport>) {
    let t = Arc::new(t);
    let c = Client::builder().transport(t.clone()).build().unwrap();
    if cookie {
        c.set_cookie("SAPISID=abc123; SID=x").unwrap();
    }
    (c, t)
}

#[test]
fn liked_page_one_fields_and_flags() {
    let page = parse_video_page(&load("yt_handwritten_liked_page1.json")).unwrap();
    let ids: Vec<_> = page.items.iter().map(|v| v.id.as_str()).collect();
    // no-id, bad-id and no-title rows are skipped; deleted video (has a title) stays
    assert_eq!(ids, ["dQw4w9WgXcQ", "aqz-KE-bpKQ", "kJQP7kiw5Fk", "JGwWNGJdvx8", "M7lc1UVf-VE"]);
    assert_eq!(page.continuation.as_deref(), Some("4qmFsgKLARIGVkxMTExMGgxTQUJRVkE%3D-PAGE2"));

    let a = &page.items[0];
    assert_eq!(a.title, "Never Gonna Give You Up");
    assert_eq!(a.duration_ms, Some(213_000));
    assert_eq!(a.set_video_id.as_deref(), Some("A1B2C3D4E5F6"));
    let ch = a.channel.as_ref().unwrap();
    assert_eq!((ch.name.as_str(), ch.id.as_deref()), ("Rick Astley", Some("UCuAXFkgsw1L7xaCfnd5JJOw")));
    assert_eq!(a.thumbnails.len(), 2);
    assert_eq!(a.thumbnails[1].width, 336);
    assert!(!a.is_live && !a.is_short);

    let b = &page.items[1];
    assert_eq!(b.title, "Big Buck Bunny 60fps 4K");
    assert!(b.thumbnails[0].url.starts_with("https://i.ytimg.com"));
    assert_eq!(b.channel.as_ref().unwrap().id.as_deref(), Some("UCSMOQeBJ2RAnuFungnQOxLg"));

    let live = &page.items[2];
    assert!(live.is_live && !live.is_short);
    assert_eq!(live.duration_ms, None);

    let short = &page.items[3];
    assert!(short.is_short && !short.is_live);
    assert_eq!(short.duration_ms, Some(42_000));
    assert_eq!(short.channel.as_ref().unwrap().id, None);

    let deleted = &page.items[4];
    assert_eq!((deleted.channel.is_none(), deleted.duration_ms, deleted.thumbnails.len()), (true, None, 0));
}

#[test]
fn continuation_action_page_keeps_duplicate_entries() {
    let page = parse_video_page(&load("yt_handwritten_liked_page2.json")).unwrap();
    assert_eq!(page.continuation, None);
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].id, page.items[1].id);
    assert_ne!(page.items[0].set_video_id, page.items[1].set_video_id);
    assert_eq!(page.items[0].duration_ms, Some(253_000));
    assert_eq!(page.items[1].duration_ms, Some(3_853_000));
    assert_eq!(page.items[1].channel.as_ref().unwrap().id, None);
}

#[test]
fn history_sections() {
    let page = parse_video_page(&load("yt_handwritten_history.json")).unwrap();
    let ids: Vec<_> = page.items.iter().map(|v| v.id.as_str()).collect();
    assert_eq!(ids, ["dQw4w9WgXcQ", "kJQP7kiw5Fk", "JGwWNGJdvx8", "aqz-KE-bpKQ"]);
    assert_eq!(page.continuation.as_deref(), Some("4qmFsgKsARIJRkVoaXN0b3J5-HIST2"));
    assert_eq!(page.items[0].duration_ms, Some(213_000));
    assert!(page.items[1].is_live && page.items[1].duration_ms.is_none());
    assert!(page.items[2].is_short);
    // overlay text is the fallback length; a non-channel browse id is dropped but the name stays
    assert_eq!(page.items[3].duration_ms, Some(635_000));
    let ch = page.items[3].channel.as_ref().unwrap();
    assert_eq!((ch.name.as_str(), ch.id.as_deref()), ("Blender", None));
}

#[test]
fn lockup_and_reel_videos() {
    let page = parse_video_page(&load("yt_handwritten_lockup_videos.json")).unwrap();
    let ids: Vec<_> = page.items.iter().map(|v| v.id.as_str()).collect();
    assert_eq!(ids, ["dQw4w9WgXcQ", "kJQP7kiw5Fk", "JGwWNGJdvx8", "9bZkp7q19f0"]);
    assert_eq!(page.continuation.as_deref(), Some("LOCKUP-TOKEN-2"));
    let a = &page.items[0];
    assert_eq!((a.duration_ms, a.title.as_str()), (Some(213_000), "Never Gonna Give You Up"));
    let ch = a.channel.as_ref().unwrap();
    assert_eq!((ch.name.as_str(), ch.id.as_deref()), ("Rick Astley", Some("UCuAXFkgsw1L7xaCfnd5JJOw")));
    assert!(page.items[1].is_live && page.items[1].duration_ms.is_none());
    assert!(page.items[2].is_short && page.items[3].is_short);
    assert_eq!(page.items[2].title, "A short clip");
}

#[test]
fn saved_playlists_lockup_layout() {
    let page = parse_playlist_page(&load("yt_handwritten_saved_playlists_lockup.json")).unwrap();
    assert_eq!(page.continuation.as_deref(), Some("PLAYLISTS-TOKEN-2"));
    assert_eq!(page.items.len(), 2); // channel lockup and title-less lockup skipped
    assert_eq!(page.items[0].id, "PLrAXtmErZgOeiKm4sgNOknGvNjby9efdf");
    assert_eq!((page.items[0].title.as_str(), page.items[0].video_count), ("Road trip", Some(1025)));
    assert_eq!(page.items[0].thumbnails.len(), 2);
    assert_eq!(page.items[1].video_count, None);
    // a video parser over the same response finds no videos
    assert!(parse_video_page(&load("yt_handwritten_saved_playlists_lockup.json")).unwrap().items.is_empty());
}

#[test]
fn saved_playlists_grid_layout() {
    let page = parse_playlist_page(&load("yt_handwritten_saved_playlists_grid.json")).unwrap();
    assert_eq!(page.continuation, None);
    assert_eq!(page.items.len(), 2); // "a/b" skipped
    assert_eq!((page.items[0].title.as_str(), page.items[0].video_count), ("Study music", Some(37)));
    assert_eq!(page.items[1].video_count, None);
    assert_eq!(page.items[1].thumbnails.len(), 1); // sidebarThumbnails fallback
}

#[test]
fn empty_and_malformed_inputs() {
    let empty = parse_video_page(&load("yt_handwritten_empty_signed_out.json")).unwrap();
    assert!(empty.items.is_empty() && empty.continuation.is_none());
    assert_eq!(parse_video_page(&json!({})).unwrap().items.len(), 0);
    assert_eq!(parse_video_page(&json!([1, 2])).unwrap_err(), Error::Parse);
    assert_eq!(parse_video_page(&json!("x")).unwrap_err(), Error::Parse);
    assert_eq!(parse_video_page(&json!({"error": {"code": 400}})).unwrap_err(), Error::Refused);
    // wrong types in every field: skipped, no panic
    let weird = json!({"a": [{"videoRenderer": {"videoId": 5, "title": 7}}, {"videoRenderer": null},
        {"playlistVideoRenderer": {"videoId": "dQw4w9WgXcQ", "title": {"runs": "no"}}},
        {"playlistVideoRenderer": {"videoId": "dQw4w9WgXcQ", "title": "ok", "lengthSeconds": {"x": 1}, "thumbnail": 3, "shortBylineText": []}},
        {"continuationItemRenderer": {"continuationEndpoint": {"continuationCommand": {"token": 5}}}}]});
    let p = parse_video_page(&weird).unwrap();
    assert_eq!(p.items.len(), 1);
    assert_eq!((p.items[0].duration_ms, p.items[0].channel.is_none(), p.continuation.is_none()), (None, true, true));
}

#[tokio::test]
async fn liked_videos_requests_and_pages() {
    let (c, t) = client(
        FixtureTransport::new()
            .with_match("browse", "PAGE2", load("yt_handwritten_liked_page2.json"))
            .with_file("browse", fx("yt_handwritten_liked_page1.json")),
        true,
    );
    let yt = Youtube::new(&c);
    let p1 = yt.liked_videos(None).await.unwrap();
    let p2 = yt.liked_videos(p1.continuation.as_deref()).await.unwrap();
    assert_eq!((p1.items.len(), p2.items.len(), p2.continuation), (5, 2, None));

    let reqs = t.requests();
    assert_eq!((reqs[0].host.as_str(), reqs[0].endpoint.as_str()), ("www.youtube.com", "browse"));
    assert_eq!(reqs[0].body["browseId"], "VLLL");
    assert_eq!(reqs[0].body["context"]["client"]["clientName"], "WEB");
    assert!(reqs[0].header("authorization").is_some_and(|a| a.starts_with("SAPISIDHASH ")));
    assert_eq!(reqs[1].body["continuation"], "4qmFsgKLARIGVkxMTExMGgxTQUJRVkE%3D-PAGE2");
    assert!(reqs[1].body.get("browseId").is_none());
}

#[tokio::test]
async fn browse_ids_per_listing() {
    let (c, t) = client(FixtureTransport::new().with("browse", json!({"responseContext": {}})), true);
    let yt = Youtube::new(&c);
    yt.watch_later(None).await.unwrap();
    yt.history(None).await.unwrap();
    yt.saved_playlists(None).await.unwrap();
    let got: Vec<_> = t.requests().iter().map(|r| r.body["browseId"].as_str().unwrap().to_owned()).collect();
    assert_eq!(got, ["VLWL", "FEhistory", "FEplaylist_aggregation"]);
}

#[tokio::test]
async fn signed_in_listings_need_a_cookie_and_send_nothing() {
    let (c, t) = client(FixtureTransport::new().with("browse", json!({})), false);
    let yt = Youtube::new(&c);
    assert_eq!(yt.liked_videos(None).await.unwrap_err(), Error::Unauthenticated);
    assert_eq!(yt.watch_later(None).await.unwrap_err(), Error::Unauthenticated);
    assert_eq!(yt.history(None).await.unwrap_err(), Error::Unauthenticated);
    assert_eq!(yt.saved_playlists(Some("tok")).await.unwrap_err(), Error::Unauthenticated);
    assert!(t.requests().is_empty());
}

#[tokio::test]
async fn public_playlist_is_anonymous_and_prefixed() {
    let (c, t) = client(FixtureTransport::new().with_file("browse", fx("yt_handwritten_liked_page1.json")), true);
    let yt = Youtube::new(&c);
    let p = yt.playlist("PLrAXtmErZgOeiKm4sgNOknGvNjby9efdf", None).await.unwrap();
    assert_eq!(p.items.len(), 5);
    yt.playlist("VLPLrAXtmErZgOeiKm4sgNOknGvNjby9efdf", None).await.unwrap();
    let reqs = t.requests();
    assert_eq!(reqs[0].body["browseId"], "VLPLrAXtmErZgOeiKm4sgNOknGvNjby9efdf");
    assert_eq!(reqs[1].body["browseId"], "VLPLrAXtmErZgOeiKm4sgNOknGvNjby9efdf");
    assert!(reqs[0].header("cookie").is_none() && reqs[0].header("authorization").is_none());
}

#[tokio::test]
async fn invalid_inputs_are_rejected_before_sending() {
    let (c, t) = client(FixtureTransport::new().with("browse", json!({})), true);
    let yt = Youtube::new(&c);
    for bad in ["", "P", "a/b", "../x", "PL id", "LL", "WL", "VL"] {
        assert_eq!(yt.playlist(bad, None).await.unwrap_err(), Error::InvalidId, "{bad:?}");
    }
    for bad in ["", "a b", "tab\there", &"x".repeat(9000)] {
        assert_eq!(yt.history(Some(bad)).await.unwrap_err(), Error::InvalidRequest);
    }
    assert!(t.requests().is_empty());
}

#[tokio::test]
async fn service_error_and_bad_json_map_to_errors() {
    let (c, _) = client(FixtureTransport::new().with("browse", json!({"error": {"code": 403, "message": "x"}})), true);
    assert_eq!(Youtube::new(&c).history(None).await.unwrap_err(), Error::Refused);
    let (c, _) = client(FixtureTransport::new().with_raw("browse", 200, b"{\"contents\": [1,"), true);
    assert_eq!(Youtube::new(&c).history(None).await.unwrap_err(), Error::Parse);
    let (c, _) = client(FixtureTransport::new().with_raw("browse", 200, b"[]"), true);
    assert_eq!(Youtube::new(&c).history(None).await.unwrap_err(), Error::Parse);
    let (c, _) = client(FixtureTransport::new().with_raw("browse", 401, b"{}"), true);
    assert_eq!(Youtube::new(&c).history(None).await.unwrap_err(), Error::Unauthenticated);
}
