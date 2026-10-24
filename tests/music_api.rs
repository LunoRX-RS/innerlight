//! `Music` handle against `FixtureTransport`: request shapes, id validation, auth handling, error pass-through.

mod common;
use common::builder;
use innerlight::music::Music;
use innerlight::music::model::{Item, SearchKind};
use innerlight::{Client, Error, FixtureTransport};
use serde_json::json;
use std::sync::Arc;

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/music_handwritten_{name}.json", env!("CARGO_MANIFEST_DIR"))
}

fn client(fx: FixtureTransport) -> (Client, Arc<FixtureTransport>) {
    let fx = Arc::new(fx);
    (builder().transport(fx.clone()).build().unwrap(), fx)
}

const TOK: &str = "4qmFsgKPARIiVkxQTFRFU1RQTEFZTElTVDEyMzQ1Njc4OTBhYmNkZWZnaBoEOGdFQg%3D%3D";
const TOKEN_ALBUMS: &str = "TOKEN_ALBUM_MORE";
const TOKEN_ARTISTS: &str = "TOKEN_ARTIST_MORE";
const HOME_TOK: &str = "4qmFsgJhEiIZRkVtdXNpY19ob21lX2NvbnRpbnVhdGlvbl90b2tlbg%3D%3D";
const RADIO_TOK1: &str = "4qmFsgJfEiBhdXRvTWl4X2NvbnRpbnVhdGlvbl9maXJzdF9iYXRjaA%3D%3D";
const RADIO_TOK2: &str = "4qmFsgJfEiBhdXRvTWl4X2NvbnRpbnVhdGlvbl9zZWNvbmRfYmF0Y2g%3D%3D";
const ALBUM_ID: &str = "MPREb_9nqEki4ZDpp";
/// The handle inside `music_handwritten_album.json`'s track shelf.
const ALB_TOK: &str = "4qmFsgKcEiYYRkVtdXNpY19hbGJ1bV90b2tlbl9mb3JfcGF0Y2hfdGVzdCUzRCUzRA==";

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

/// The wire values of the four `params` filter chips, asserted here so a change in one place is visible in the other.
const TRACKS_CHIP: &str = "EgWKAQIIAWoMEA4QChADEAQQCRAF";
const ALBUMS_CHIP: &str = "EgWKAQIIA2oMEA4QChADEAQQCRAF";
const ARTISTS_CHIP: &str = "EgWKAQIIBGoMEA4QChADEAQQCRAF";
const PLAYLISTS_CHIP: &str = "EgWKAQIIBWoMEA4QChADEAQQCRAF";

#[tokio::test]
async fn typed_search_sends_its_chip_and_narrows_the_page() {
    let empty_shelf = json!({"continuationContents": {"musicShelfContinuation": {"contents": []}}});
    let (c, fx) = client(
        FixtureTransport::new()
            .with_match("search", TRACKS_CHIP, json!(fixture_value("search_songs")))
            .with_match("search", ALBUMS_CHIP, json!(fixture_value("search_albums")))
            .with_match("search", ARTISTS_CHIP, json!(fixture_value("search_artists")))
            .with_match("search", PLAYLISTS_CHIP, json!(fixture_value("search_playlists")))
            .with_match("search", &format!("\"continuation\":\"{TOKEN_ALBUMS}\""), empty_shelf.clone())
            .with_match(
                "search",
                &format!("\"continuation\":\"{TOKEN_ARTISTS}\""),
                json!(fixture_value("search_artists")),
            ),
    );
    let m = Music::new(&c);

    let albums = m.search_albums("jam").await.unwrap();
    assert_eq!(albums.continuation.as_deref(), Some(TOKEN_ALBUMS));
    assert_eq!(m.search_artists("jam").await.unwrap().items.len(), 2);
    assert_eq!(m.search_playlists("jam").await.unwrap().items.len(), 2);
    // the tracks chip is the one search_tracks has always sent, so a tracks page is unchanged
    assert_eq!(m.search_items("jam", SearchKind::Tracks).await.unwrap().items.len(), 3);

    // a mixed page keeps rows and cards in document order, including the top result
    let mixed = m.search_items("jam", SearchKind::Albums).await.unwrap();
    assert!(mixed.items.iter().all(|i| matches!(i, Item::Album(_))));

    // an empty continuation page is an empty page, not an error
    let none = m.search_albums_more(TOKEN_ALBUMS).await.unwrap();
    assert!(none.items.is_empty() && none.continuation.is_none());
    // the narrowed follow-up drops foreign cards; the generic one reports what came back
    assert_eq!(m.search_artists_more(TOKEN_ARTISTS).await.unwrap().items.len(), 2);
    let generic = m.search_items_more(TOKEN_ARTISTS).await.unwrap();
    assert_eq!(generic.items.len(), 3, "artist shelf plus the stray album");
    assert!(generic.items.iter().any(|i| matches!(i, Item::Album(_))));

    let sent = fx.requests();
    assert_eq!(sent[0].body["params"].as_str(), Some(ALBUMS_CHIP));
    assert_eq!(sent[1].body["params"].as_str(), Some(ARTISTS_CHIP));
    assert_eq!(sent[2].body["params"].as_str(), Some(PLAYLISTS_CHIP));
    assert_eq!(sent[3].body["params"].as_str(), Some(TRACKS_CHIP));
    assert_eq!(sent[4].body["params"].as_str(), Some(ALBUMS_CHIP));
    assert_eq!(sent[5].body["continuation"].as_str(), Some(TOKEN_ALBUMS));
    assert_eq!(sent[6].body["continuation"].as_str(), Some(TOKEN_ARTISTS));
    assert_eq!(sent[7].body["continuation"].as_str(), Some(TOKEN_ARTISTS));
    assert_eq!(sent.len(), 8);
    assert!(sent.iter().all(|r| r.header("cookie").is_none()));
}

#[tokio::test]
async fn album_more_pages_the_album_track_list() {
    let (c, fx) = client(
        FixtureTransport::new()
            .with_match("browse", &format!("\"continuation\":\"{ALB_TOK}\""), json!(fixture_value("playlist_last")))
            .with_match("browse", ALBUM_ID, json!(fixture_value("album"))),
    );
    let m = Music::new(&c);
    let album = m.album(ALBUM_ID).await.unwrap();
    assert_eq!(album.tracks.len(), 2);
    assert_eq!(album.continuation.as_deref(), Some(ALB_TOK));
    let page = m.album_more(album.continuation.as_deref().unwrap()).await.unwrap();
    assert_eq!(page.items.len(), 1);
    let sent = fx.requests();
    assert_eq!(sent[0].body["browseId"].as_str(), Some(ALBUM_ID));
    assert_eq!(sent[1].body["continuation"].as_str(), Some(ALB_TOK));
    assert_eq!(sent.len(), 2);
    assert!(sent.iter().all(|r| r.header("cookie").is_none()));
    assert_eq!(m.album_more("bad token").await.unwrap_err(), Error::InvalidId);
}

#[tokio::test]
async fn home_feed_is_anonymous_until_asked_signed_in() {
    let (c, fx) =
        client(FixtureTransport::new().with_match("browse", "FEmusic_home", json!(fixture_value("home"))).with_match(
            "browse",
            &format!("\"continuation\":\"{HOME_TOK}\""),
            json!(fixture_value("home_more")),
        ));
    let m = Music::new(&c);
    let page = m.home().await.unwrap();
    assert_eq!(page.items.len(), 3);
    assert_eq!(fx.requests()[0].body["browseId"].as_str(), Some("FEmusic_home"));
    let more = m.home_more(page.continuation.as_deref().unwrap()).await.unwrap();
    assert_eq!(more.items.len(), 2);
    assert!(more.continuation.is_none());
    assert_eq!(fx.requests()[1].body["continuation"].as_str(), Some(HOME_TOK));
    assert!(fx.requests().iter().all(|r| r.header("cookie").is_none()));

    // the personalized feed needs a cookie and sends nothing without one
    let (c2, fx2) = client(
        FixtureTransport::new()
            .with_match("browse", &format!("\"continuation\":\"{HOME_TOK}\""), json!(fixture_value("home_more")))
            .with_match("browse", "FEmusic_home", json!(fixture_value("home"))),
    );
    let m2 = Music::new(&c2);
    assert_eq!(m2.home_authed().await.unwrap_err(), Error::Unauthenticated);
    assert_eq!(m2.home_more_authed(HOME_TOK).await.unwrap_err(), Error::Unauthenticated);
    assert!(fx2.requests().is_empty());
    c2.set_cookie("SAPISID=abc123; SID=x").unwrap();
    assert_eq!(m2.home_authed().await.unwrap().items.len(), 3);
    assert_eq!(m2.home_more_authed(HOME_TOK).await.unwrap().items.len(), 2);
    for r in fx2.requests() {
        assert!(r.header("authorization").is_some_and(|a| a.starts_with("SAPISIDHASH ")));
    }
}

#[tokio::test]
async fn radio_more_pages_the_queue() {
    let (c, fx) =
        client(FixtureTransport::new().with_match("next", "videoId", json!(fixture_value("next_radio"))).with_match(
            "next",
            &format!("\"continuation\":\"{RADIO_TOK1}\""),
            json!(fixture_value("next_radio_more")),
        ));
    let m = Music::new(&c);
    let r = m.radio("aqz-KE-bpKQ").await.unwrap();
    assert_eq!(r.continuation.as_deref(), Some(RADIO_TOK1));
    let more = m.radio_more(r.continuation.as_deref().unwrap()).await.unwrap();
    let ids: Vec<_> = more.items.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["aqz-KE-bpKQ", "nnnnnnnnnn2", "mmmmmmmmmm3"]);
    assert_eq!(more.continuation.as_deref(), Some(RADIO_TOK2));

    let sent = fx.requests();
    assert_eq!(sent[1].body["continuation"].as_str(), Some(RADIO_TOK1));
    assert!(sent[1].body.get("videoId").is_none(), "a paging call asks for the next batch, not a new radio");
}

#[tokio::test]
async fn bad_input_is_rejected_before_any_request() {
    let (c, fx) = client(FixtureTransport::new());
    let m = Music::new(&c);
    for q in ["", "   ", "a\nb", "a\u{0}b"] {
        assert_eq!(m.search_tracks(q).await.unwrap_err(), Error::InvalidRequest, "{q:?}");
        assert_eq!(m.search_suggestions(q).await.unwrap_err(), Error::InvalidRequest, "{q:?}");
        assert_eq!(m.search_suggestions_authed(q).await.unwrap_err(), Error::InvalidRequest, "{q:?}");
    }
    assert_eq!(m.search_tracks(&"x".repeat(513)).await.unwrap_err(), Error::InvalidRequest);
    assert_eq!(m.search_tracks_more("bad token").await.unwrap_err(), Error::InvalidId);
    for kind in [SearchKind::Tracks, SearchKind::Albums, SearchKind::Artists, SearchKind::Playlists] {
        assert_eq!(m.search_items(" ", kind).await.unwrap_err(), Error::InvalidRequest, "{kind:?}");
    }
    assert_eq!(m.search_albums("").await.unwrap_err(), Error::InvalidRequest);
    assert_eq!(m.search_artists("  ").await.unwrap_err(), Error::InvalidRequest);
    assert_eq!(m.search_playlists("").await.unwrap_err(), Error::InvalidRequest);
    assert_eq!(m.search_items_more("bad token!").await.unwrap_err(), Error::InvalidId);
    assert_eq!(m.search_albums_more("").await.unwrap_err(), Error::InvalidId);
    assert_eq!(m.album_more("bad token!").await.unwrap_err(), Error::InvalidId);
    assert_eq!(m.home_more("bad token!").await.unwrap_err(), Error::InvalidId);
    assert_eq!(m.home_more_authed("").await.unwrap_err(), Error::InvalidId);
    assert_eq!(m.radio_more("bad token!").await.unwrap_err(), Error::InvalidId);
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
async fn suggestions_follow_the_cookie_rule() {
    let (c, fx) = client(FixtureTransport::new().with_file("music/get_search_suggestions", fixture("suggestions")));
    let m = Music::new(&c);
    // Signed out, the authed variant says so before anything leaves the client.
    assert_eq!(m.search_suggestions_authed("hel").await.unwrap_err(), Error::Unauthenticated);
    assert!(fx.requests().is_empty());

    c.set_cookie("SAPISID=abc123; SID=x").unwrap();
    assert_eq!(m.search_suggestions_authed("hel").await.unwrap().len(), 3);
    let signed = &fx.requests()[0];
    assert_eq!(signed.endpoint, "music/get_search_suggestions");
    assert_eq!(signed.body["input"], "hel");
    assert!(signed.header("authorization").is_some_and(|a| a.starts_with("SAPISIDHASH ")));

    // The anonymous twin still sends no credential with a cookie stored.
    assert_eq!(m.search_suggestions("hel").await.unwrap().len(), 3);
    let plain = &fx.requests()[1];
    assert!(plain.header("cookie").is_none() && plain.header("authorization").is_none());
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
    assert_eq!(m.new_albums().await.unwrap_err(), Error::ServerError);
    // valid JSON with the wrong shape is a Parse error, not an empty success
    assert_eq!(m.radio("aqz-KE-bpKQ").await.unwrap_err(), Error::Parse);
}
