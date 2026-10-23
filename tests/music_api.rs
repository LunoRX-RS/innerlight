//! `Music` handle against `FixtureTransport`: request shapes, id validation, auth handling, error pass-through.

use innerlight::music::Music;
use innerlight::{Client, Error, FixtureTransport};
use serde_json::json;
use std::sync::Arc;

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/music_handwritten_{name}.json", env!("CARGO_MANIFEST_DIR"))
}

fn client(fx: FixtureTransport) -> (Client, Arc<FixtureTransport>) {
    let fx = Arc::new(fx);
    (Client::builder().transport(fx.clone()).build().unwrap(), fx)
}

const TOK: &str = "4qmFsgKPARIiVkxQTFRFU1RQTEFZTElTVDEyMzQ1Njc4OTBhYmNkZWZnaBoEOGdFQg%3D%3D";

#[tokio::test]
async fn search_then_more_then_suggestions() {
    let (c, fx) = client(
        FixtureTransport::new()
            .with_match(
                "search",
                "\"continuation\"",
                json!({"continuationContents": {"musicShelfContinuation": {"contents": []}}}),
            )
            .with_file("search", fixture("search_songs"))
            .with_file("music/get_search_suggestions", fixture("suggestions")),
    );
    let m = Music::new(&c);
    let page = m.search_tracks("  hello  ").await.unwrap();
    assert_eq!(page.items.len(), 3);
    let more = m.search_tracks_more(page.continuation.as_deref().unwrap()).await.unwrap();
    assert!(more.items.is_empty() && more.continuation.is_none());
    assert_eq!(m.search_suggestions("hel").await.unwrap().len(), 3);

    let reqs = fx.requests();
    assert_eq!(reqs[0].host, "music.youtube.com");
    assert_eq!(reqs[0].body["query"], "hello");
    assert!(reqs[0].body["params"].as_str().is_some_and(|p| !p.is_empty()));
    assert_eq!(reqs[0].body["context"]["client"]["clientName"], "WEB_REMIX");
    assert_eq!(reqs[1].body["continuation"].as_str(), page.continuation.as_deref());
    assert_eq!(reqs[2].endpoint, "music/get_search_suggestions");
    assert_eq!(reqs[2].body["input"], "hel");
    assert!(reqs.iter().all(|r| r.header("cookie").is_none()));
}

#[tokio::test]
async fn bad_input_is_rejected_before_any_request() {
    let (c, fx) = client(FixtureTransport::new());
    let m = Music::new(&c);
    for q in ["", "   ", "a\nb", "a\u{0}b"] {
        assert_eq!(m.search_tracks(q).await.unwrap_err(), Error::InvalidRequest, "{q:?}");
        assert_eq!(m.search_suggestions(q).await.unwrap_err(), Error::InvalidRequest, "{q:?}");
    }
    assert_eq!(m.search_tracks(&"x".repeat(513)).await.unwrap_err(), Error::InvalidRequest);
    assert_eq!(m.search_tracks_more("bad token").await.unwrap_err(), Error::InvalidId);
    assert_eq!(m.radio("short").await.unwrap_err(), Error::InvalidId);
    assert_eq!(m.radio("../../etc/pa").await.unwrap_err(), Error::InvalidId);
    assert_eq!(m.album("UCuAXFkgsw1L7xaCfnd5JJOw").await.unwrap_err(), Error::InvalidId);
    assert_eq!(m.artist("MPREb_9nqEki4ZDpp").await.unwrap_err(), Error::InvalidId);
    assert_eq!(m.playlist("a/b").await.unwrap_err(), Error::InvalidId);
    assert_eq!(m.playlist("VL").await.unwrap_err(), Error::InvalidId);
    assert_eq!(m.playlist_more("x").await.unwrap_err(), Error::InvalidId);
    assert_eq!(m.lyrics("MPREb_9nqEki4ZDpp").await.unwrap_err(), Error::InvalidId);
    assert_eq!(m.charts(Some("th")).await.unwrap_err(), Error::InvalidId);
    assert_eq!(m.genre_playlists("a b").await.unwrap_err(), Error::InvalidId);
    assert_eq!(m.saved_tracks_more("").await.unwrap_err(), Error::InvalidId);
    assert!(fx.requests().is_empty());
}

#[tokio::test]
async fn radio_then_lyrics() {
    let (c, fx) = client(
        FixtureTransport::new()
            .with_file("next", fixture("next_radio"))
            .with_match("browse", "MPLYt_s7WaJYzUc2cfGsaF", json!({"contents": {"sectionListRenderer": {"contents": [{"musicDescriptionShelfRenderer": {"description": {"runs": [{"text": "la la"}]}}}]}}})),
    );
    let m = Music::new(&c);
    let r = m.radio("aqz-KE-bpKQ").await.unwrap();
    assert_eq!(r.queue.len(), 2);
    let l = m.lyrics(r.lyrics_browse_id.as_deref().unwrap()).await.unwrap();
    assert_eq!((l.text.as_str(), l.is_timed(), l.source.as_deref()), ("la la", false, None));

    let sent = fx.requests();
    assert_eq!(sent[0].endpoint, "next");
    assert_eq!(sent[0].body["videoId"], "aqz-KE-bpKQ");
    assert_eq!(sent[0].body["playlistId"], "RDAMVMaqz-KE-bpKQ");
    assert_eq!(sent[1].body["browseId"], "MPLYt_s7WaJYzUc2cfGsaF");
}

#[tokio::test]
async fn album_artist_playlist_requests() {
    let (c, fx) = client(
        FixtureTransport::new()
            .with_match("browse", "MPREb_9nqEki4ZDpp", json!(fixture_value("album")))
            .with_match("browse", "UCuAXFkgsw1L7xaCfnd5JJOw", json!(fixture_value("artist")))
            .with_match("browse", "VLPLTESTPLAYLIST1234567890abcdefgh", json!(fixture_value("playlist")))
            .with_match("browse", &format!("\"continuation\":\"{TOK}\""), json!(fixture_value("playlist_last"))),
    );
    let m = Music::new(&c);
    assert_eq!(m.album("MPREb_9nqEki4ZDpp").await.unwrap().tracks.len(), 2);
    assert_eq!(m.artist("UCuAXFkgsw1L7xaCfnd5JJOw").await.unwrap().albums.len(), 2);
    let p = m.playlist("PLTESTPLAYLIST1234567890abcdefgh").await.unwrap();
    assert_eq!(p.id, "PLTESTPLAYLIST1234567890abcdefgh");
    // the browse form with the VL prefix is accepted too and gives the same id
    let p2 = m.playlist("VLPLTESTPLAYLIST1234567890abcdefgh").await.unwrap();
    assert_eq!(p2.id, p.id);
    let more = m.playlist_more(p.tracks.continuation.as_deref().unwrap()).await.unwrap();
    assert_eq!(more.items.len(), 1);
    assert_eq!(fx.requests()[2].body["browseId"], "VLPLTESTPLAYLIST1234567890abcdefgh");
}

fn fixture_value(name: &str) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(fixture(name)).unwrap()).unwrap()
}

#[tokio::test]
async fn explore_requests() {
    let (c, fx) = client(
        FixtureTransport::new()
            .with_match("browse", "FEmusic_charts", fixture_value("charts"))
            .with_match("browse", "FEmusic_moods_and_genres_category", fixture_value("genre_playlists"))
            .with_match("browse", "FEmusic_moods_and_genres", fixture_value("genres"))
            .with_match("browse", "FEmusic_new_releases_albums", fixture_value("new_albums"))
            .with_match("browse", "FEmusic_new_releases_videos", fixture_value("new_videos")),
    );
    let m = Music::new(&c);
    assert_eq!(m.charts(Some("ZZ")).await.unwrap().len(), 4);
    assert_eq!(m.charts(None).await.unwrap().len(), 4);
    let genres = m.genres().await.unwrap();
    assert_eq!(m.genre_playlists(&genres[0].genres[0].params).await.unwrap().len(), 2);
    assert_eq!(m.new_albums().await.unwrap().len(), 2);
    assert_eq!(m.new_videos().await.unwrap().len(), 2);

    let sent = fx.requests();
    assert_eq!(sent[0].body["formData"]["selectedValues"][0], "ZZ");
    assert!(sent[1].body.get("formData").is_none());
    assert_eq!(sent[3].body["params"], "ggMPOg1uX1ZaRGhUa29OSlhr");
}

#[tokio::test]
async fn library_needs_a_cookie_and_signs_requests() {
    let (c, fx) = client(
        FixtureTransport::new()
            .with_match("browse", "FEmusic_liked_videos", fixture_value("library_liked"))
            .with_match("browse", "FEmusic_liked_playlists", fixture_value("library_playlists"))
            .with_match("browse", "FEmusic_history", fixture_value("history"))
            .with("browse", fixture_value("library_liked")),
    );
    let m = Music::new(&c);
    // signed out: typed error, nothing sent
    assert_eq!(m.saved_tracks().await.unwrap_err(), Error::Unauthenticated);
    assert_eq!(m.saved_playlists().await.unwrap_err(), Error::Unauthenticated);
    assert_eq!(m.history().await.unwrap_err(), Error::Unauthenticated);
    assert_eq!(m.history_more(TOK).await.unwrap_err(), Error::Unauthenticated);
    assert!(fx.requests().is_empty());

    c.set_cookie("SAPISID=abc123; SID=x").unwrap();
    let liked = m.saved_tracks().await.unwrap();
    assert_eq!(liked.items.len(), 2);
    assert_eq!(m.saved_playlists().await.unwrap().items.len(), 2);
    assert_eq!(m.history().await.unwrap().items.len(), 2);
    assert_eq!(m.saved_tracks_more(liked.continuation.as_deref().unwrap()).await.unwrap().items.len(), 2);
    for r in fx.requests() {
        assert!(r.header("authorization").is_some_and(|a| a.starts_with("SAPISIDHASH ")));
    }
    // public endpoints stay anonymous even with a cookie set
    let (c2, fx2) = client(FixtureTransport::new().with("browse", fixture_value("new_albums")));
    c2.set_cookie("SAPISID=abc123").unwrap();
    Music::new(&c2).new_albums().await.unwrap();
    assert!(fx2.requests()[0].header("cookie").is_none());
}

#[tokio::test]
async fn errors_pass_through() {
    let (c, _) = client(
        FixtureTransport::new()
            .with_raw("search", 429, b"{}")
            .with_raw("browse", 503, b"{}")
            .with("next", json!({"x": 1})),
    );
    let m = Music::new(&c);
    assert!(matches!(m.search_tracks("x").await, Err(Error::RateLimited { .. })));
    assert_eq!(m.new_albums().await.unwrap_err(), Error::Network);
    // valid JSON with the wrong shape is a Parse error, not an empty success
    assert_eq!(m.radio("aqz-KE-bpKQ").await.unwrap_err(), Error::Parse);
}
