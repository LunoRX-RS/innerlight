//! Offline tests for `Youtube::search_videos`. The fixture is hand-written (`yt_handwritten_*`), not a recording.

mod common;
use common::builder;
use innerlight::youtube::{Youtube, parse_video_page};
use innerlight::{Error, FixtureTransport};
use serde_json::{Value, json};
use std::sync::Arc;

fn fx(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn load(name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(fx(name)).unwrap()).unwrap()
}

#[test]
fn search_page_fields_and_flags() {
    let page = parse_video_page(&load("yt_handwritten_search_videos.json")).unwrap();
    let ids: Vec<_> = page.items.iter().map(|v| v.id.as_str()).collect();
    // channel row and bad id are skipped; the short is kept and flagged
    assert_eq!(ids, ["dQw4w9WgXcQ", "kJQP7kiw5Fk", "aqz-KE-bpKQ", "9bZkp7q19f0", "JGwWNGJdvx8"]);
    assert_eq!(page.continuation.as_deref(), Some("EooDEgZsb2ZpIGEa-SEARCH2"));
    let a = &page.items[0];
    assert_eq!((a.title.as_str(), a.duration_ms, a.thumbnails.len()), ("Handwritten song one", Some(213_000), 2));
    assert_eq!(a.channel.as_ref().unwrap().id.as_deref(), Some("UCuAXFkgsw1L7xaCfnd5JJOw"));
    assert!(page.items[1].is_live && page.items[1].duration_ms.is_none());
    // a premiere that has not started: not live, no length
    assert!(!page.items[2].is_live && page.items[2].duration_ms.is_none());
    assert_eq!(page.items[3].duration_ms, Some(3_723_000));
    assert!(page.items[4].is_short);
}

#[tokio::test]
async fn search_videos_sends_an_anonymous_web_request_and_pages() {
    let t = Arc::new(
        FixtureTransport::new()
            .with_match("search", "SEARCH2", json!({"responseContext": {}}))
            .with_file("search", fx("yt_handwritten_search_videos.json")),
    );
    let c = builder().transport(t.clone()).build().unwrap();
    c.set_cookie("SAPISID=abc123; SID=x").unwrap();
    let yt = Youtube::new(&c);
    let p1 = yt.search_videos("  lofi beats ", None).await.unwrap();
    assert_eq!(p1.items.len(), 5);
    let p2 = yt.search_videos("ignored on later pages", p1.continuation.as_deref()).await.unwrap();
    assert!(p2.items.is_empty() && p2.continuation.is_none());

    let reqs = t.requests();
    assert_eq!((reqs[0].host.as_str(), reqs[0].endpoint.as_str()), ("www.youtube.com", "search"));
    assert_eq!(reqs[0].body["query"], "lofi beats");
    assert_eq!(reqs[0].body["params"], "EgIQAQ%3D%3D");
    assert_eq!(reqs[0].body["context"]["client"]["clientName"], "WEB");
    assert!(reqs[0].header("authorization").is_none() && reqs[0].header("cookie").is_none());
    assert_eq!(reqs[1].body["continuation"], "EooDEgZsb2ZpIGEa-SEARCH2");
    assert!(reqs[1].body.get("query").is_none());
}

#[tokio::test]
async fn search_videos_validates_before_sending_and_maps_errors() {
    let t = Arc::new(FixtureTransport::new().with_raw("search", 429, b"{}"));
    let c = builder().transport(t.clone()).build().unwrap();
    let yt = Youtube::new(&c);
    for bad in ["", "   ", "tab\there", &"x".repeat(513)] {
        assert_eq!(yt.search_videos(bad, None).await.unwrap_err(), Error::InvalidRequest, "{bad:?}");
    }
    for bad in ["", "has space", "ctl\u{7}x"] {
        assert_eq!(yt.search_videos("x", Some(bad)).await.unwrap_err(), Error::InvalidRequest, "{bad:?}");
    }
    assert!(t.requests().is_empty());
    assert!(matches!(yt.search_videos("x", None).await.unwrap_err(), Error::RateLimited { .. }));
}

#[test]
fn empty_and_malformed_search_answers_do_not_panic() {
    assert!(parse_video_page(&json!({"contents": {}})).unwrap().items.is_empty());
    assert_eq!(parse_video_page(&json!(null)).unwrap_err(), Error::Parse);
    let weird = json!({"contents": [{"videoRenderer": 5}, {"videoRenderer": {"videoId": ["x"]}}]});
    assert!(parse_video_page(&weird).unwrap().items.is_empty());
}
