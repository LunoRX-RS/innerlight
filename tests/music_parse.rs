//! Parser tests against hand-authored fixtures (`tests/fixtures/music_handwritten_*.json`).
//! The fixtures mirror the shape of music.youtube.com responses as the author knows them; they are NOT recordings.

use innerlight::Error;
use innerlight::music::model::*;
use innerlight::music::parse;
use serde_json::{Value, json};

const A1: &str = "UCuAXFkgsw1L7xaCfnd5JJOw";
const A2: &str = "UC1234567890abcdefghij_-";
const ALB1: &str = "MPREb_9nqEki4ZDpp";

fn load(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/music_handwritten_{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))).unwrap()
}

fn names(a: &[ArtistRef]) -> Vec<&str> {
    a.iter().map(|a| a.name.as_str()).collect()
}

#[test]
fn search_songs_keeps_music_and_skips_the_rest() {
    let page = parse::search_tracks(&load("search_songs")).unwrap();
    let ids: Vec<_> = page.items.iter().map(|t| t.id.as_str()).collect();
    // podcast episode, livestream and the row without a video id are gone
    assert_eq!(ids, ["aqz-KE-bpKQ", "dQw4w9WgXcQ", "zzzzzzzzzz1"]);
    assert!(page.continuation.as_deref().unwrap().starts_with("EogDEg"));

    let song = &page.items[0];
    assert_eq!(song.title, "Example Song");
    assert_eq!(names(&song.artists), ["Example Artist", "Guest Two"]);
    assert_eq!(song.artists[0].id.as_deref(), Some(A1));
    assert_eq!(song.artists[1].id.as_deref(), Some(A2));
    assert_eq!(song.album, Some(AlbumRef { id: Some(ALB1.into()), name: "Example Album".into() }));
    assert_eq!(song.duration_ms, Some(225_000));
    assert!(song.is_explicit);
    assert_eq!(song.kind, TrackKind::Audio);
    assert_eq!(song.thumbnails.len(), 2);
    assert_eq!(song.thumbnails[1].width, 226);

    let video = &page.items[1];
    assert_eq!(video.kind, TrackKind::Video);
    assert_eq!(names(&video.artists), ["Example Artist"]); // "1.2M views" is not an artist
    assert_eq!(video.album, None);
    assert_eq!(video.duration_ms, Some(245_000));
    assert!(!video.is_explicit);

    let plain = &page.items[2];
    assert_eq!(plain.artists, [ArtistRef { id: None, name: "Solo Plain".into() }]);
    assert_eq!(plain.album, None);
    assert_eq!(plain.duration_ms, Some(120_000));
}

#[test]
fn search_empty_and_wrong_shape() {
    let page = parse::search_tracks(&load("search_empty")).unwrap();
    assert!(page.items.is_empty() && page.continuation.is_none());
    assert_eq!(parse::search_tracks(&json!({"hello": 1})), Err(Error::Parse));
    assert_eq!(parse::search_tracks(&json!(null)), Err(Error::Parse));
    assert_eq!(parse::search_tracks(&json!([1, 2])), Err(Error::Parse));
}

#[test]
fn continuation_pages_in_both_shapes() {
    let legacy = parse::track_page(&load("search_more_legacy")).unwrap();
    assert_eq!(legacy.items.len(), 1);
    assert_eq!(legacy.items[0].id, "bbbbbbbbbb2");
    assert_eq!(legacy.continuation, None);

    let actions = parse::track_page(&load("search_more_actions")).unwrap();
    assert_eq!(actions.items.len(), 1);
    assert_eq!(actions.items[0].title, "Third Page Song");
    assert!(actions.continuation.unwrap().starts_with("4qmFsg"));

    // a token that fails validation is ignored rather than handed to the caller
    let v = json!({"continuationContents": {"musicShelfContinuation": {"contents": [], "continuations": [{"nextContinuationData": {"continuation": "bad token!"}}]}}});
    assert_eq!(parse::track_page(&v).unwrap().continuation, None);
}

#[test]
fn suggestions() {
    let s = parse::search_suggestions(&load("suggestions")).unwrap();
    assert_eq!(s, ["hello world", "hello again", "hello history"]);
    assert_eq!(parse::search_suggestions(&json!({})), Err(Error::Parse));
    assert_eq!(parse::search_suggestions(&json!({"contents": []})).unwrap(), Vec::<String>::new());
}

#[test]
fn radio_queue_and_tab_ids() {
    let r = parse::radio(&load("next_radio")).unwrap();
    let ids: Vec<_> = r.queue.iter().map(|t| t.id.as_str()).collect();
    // counterpart (the video version) is not queued; podcast and unknown renderers are skipped
    assert_eq!(ids, ["aqz-KE-bpKQ", "nnnnnnnnnn2"]);
    let first = &r.queue[0];
    assert_eq!(first.duration_ms, Some(225_000));
    assert!(first.is_explicit);
    assert_eq!(first.album.as_ref().unwrap().id.as_deref(), Some(ALB1));
    assert_eq!(first.thumbnails.len(), 2);
    assert_eq!(r.playlist_id.as_deref(), Some("RDAMVMaqz-KE-bpKQ"));
    assert_eq!(r.lyrics_browse_id.as_deref(), Some("MPLYt_s7WaJYzUc2cfGsaF"));
    assert_eq!(r.related_browse_id.as_deref(), Some("MPTRt_s7WaJYzUc2cfGsaF"));

    let none = parse::radio(&load("next_no_tabs")).unwrap();
    assert_eq!(none.queue.len(), 2);
    assert_eq!((none.lyrics_browse_id, none.related_browse_id), (None, None));
    assert_eq!(parse::radio(&json!({"contents": {}})), Err(Error::Parse));
}

#[test]
fn lyrics_plain_and_timed() {
    let p = parse::lyrics(&load("lyrics_plain")).unwrap();
    assert!(!p.is_timed());
    assert_eq!(p.text, "First line\nSecond line\n\nThird line");
    assert_eq!(p.source.as_deref(), Some("Source: Example Lyrics Provider"));

    let t = parse::lyrics(&load("lyrics_timed")).unwrap();
    assert!(t.is_timed());
    // the line without a start time is dropped, the last one has no end time
    assert_eq!(
        t.lines,
        [
            LyricLine { start_ms: 1500, end_ms: Some(3000), text: "First line".into() },
            LyricLine { start_ms: 3000, end_ms: Some(4500), text: "Second line".into() },
            LyricLine { start_ms: 4500, end_ms: None, text: "Last line".into() },
        ]
    );
    assert_eq!(t.text, "First line\nSecond line\nLast line");
    assert_eq!(t.source.as_deref(), Some("Source: Example Lyrics Provider"));

    assert_eq!(parse::lyrics(&json!({"contents": {}})), Err(Error::Parse));
}

#[test]
fn album_new_layout() {
    let a = parse::album(&load("album"), ALB1).unwrap();
    assert_eq!((a.id.as_str(), a.title.as_str()), (ALB1, "Example Album"));
    assert_eq!(names(&a.artists), ["Example Artist"]);
    assert_eq!((a.year, a.kind, a.track_count), (Some(2021), AlbumKind::Album, Some(2)));
    assert_eq!(a.description.as_deref(), Some("An invented album."));
    assert_eq!(a.audio_playlist_id.as_deref(), Some("OLAK5uy_nPVVQ5bB1Fx4XqXVWwTn7b0sc5nB7sYzE"));
    assert_eq!(a.thumbnails.len(), 2);
    assert_eq!(a.tracks.len(), 2);
    // tracks without their own byline inherit the album artists and the album ref
    assert_eq!(names(&a.tracks[0].artists), ["Example Artist"]);
    assert_eq!(a.tracks[0].album.as_ref().unwrap().id.as_deref(), Some(ALB1));
    assert_eq!(names(&a.tracks[1].artists), ["Guest Two"]);
    assert_eq!(a.duration_ms, Some(225_000 + 200_000));
}

#[test]
fn album_legacy_layout() {
    let a = parse::album(&load("album_legacy"), "MPREb_Single00001").unwrap();
    assert_eq!((a.title.as_str(), a.kind, a.year), ("Legacy Single", AlbumKind::Single, Some(2019)));
    assert_eq!(names(&a.artists), ["Example Artist"]);
    assert_eq!(a.tracks.len(), 1);
    assert_eq!(a.track_count, Some(1));
    assert_eq!(a.tracks[0].duration_ms, Some(181_000));
    assert_eq!(parse::album(&json!({"contents": {}}), ALB1), Err(Error::Parse));
}

#[test]
fn artist_page() {
    let a = parse::artist(&load("artist"), A1).unwrap();
    assert_eq!((a.name.as_str(), a.subscribers.as_deref()), ("Example Artist", Some("1.2M")));
    assert_eq!(a.description.as_deref(), Some("An invented artist."));
    assert_eq!(a.top_songs_playlist_id.as_deref(), Some("OLAK5uy_topsongs0123456789"));
    assert_eq!(a.top_songs.len(), 2);
    // first top song has no artist column: the page artist is used; "10M plays" is not an artist
    assert_eq!(a.top_songs[0].artists, [ArtistRef { id: Some(A1.into()), name: "Example Artist".into() }]);
    assert_eq!(a.top_songs[0].album.as_ref().unwrap().name, "Example Album");
    let albums: Vec<_> = a.albums.iter().map(|x| (x.title.as_str(), x.kind, x.year, x.is_explicit)).collect();
    assert_eq!(
        albums,
        [("Example Album", AlbumKind::Unknown, Some(2021), false), ("Album Two", AlbumKind::Album, Some(2018), true)]
    );
    assert_eq!(names(&a.albums[0].artists), ["Example Artist"]);
    let singles: Vec<_> = a.singles.iter().map(|x| (x.title.as_str(), x.kind)).collect();
    assert_eq!(singles, [("Lone Single", AlbumKind::Single), ("Mini EP", AlbumKind::Ep)]);
    // the artist itself is not "related"
    assert_eq!(a.related.len(), 1);
    assert_eq!(a.related[0].name, "Other Artist");
    assert_eq!(a.related[0].subtitle.as_deref(), Some("350K subscribers"));
    assert_eq!(parse::artist(&json!({"contents": {}}), A1), Err(Error::Parse));
}

#[test]
fn playlist_first_and_last_page() {
    let id = "PLTESTPLAYLIST1234567890abcdefgh";
    let p = parse::playlist(&load("playlist"), id).unwrap();
    assert_eq!((p.title.as_str(), p.track_count), ("Test Mix", Some(1234)));
    assert_eq!(p.author.as_ref().map(|a| (a.name.as_str(), a.id.as_deref())), Some(("Some Curator", Some(A2))));
    assert_eq!(p.description.as_deref(), Some("A playlist for tests."));
    // the unavailable row (no video id) is skipped
    assert_eq!(p.tracks.items.len(), 2);
    assert_eq!(p.tracks.items[1].duration_ms, Some(3_723_000));
    assert_eq!(p.tracks.items[1].album.as_ref().unwrap().id.as_deref(), Some("MPREb_Zk3abcDEF12"));
    assert!(p.tracks.continuation.is_some());

    let last = parse::playlist(&load("playlist_last"), id).unwrap();
    assert_eq!(last.tracks.continuation, None);
    assert_eq!(parse::playlist(&json!({"contents": {}}), id), Err(Error::Parse));
}

#[test]
fn charts_and_genres() {
    let charts = parse::charts(&load("charts")).unwrap();
    let titles: Vec<_> = charts.iter().map(|s| s.title.as_deref().unwrap()).collect();
    assert_eq!(titles, ["Top songs", "Top music videos", "Top artists", "Trending"]); // the empty shelf is dropped
    assert!(matches!(&charts[0].items[0], Item::Track(t) if t.kind == TrackKind::Video));
    assert!(matches!(&charts[1].items[0], Item::Track(t) if t.id == "dQw4w9WgXcQ"));
    assert!(matches!(&charts[2].items[0], Item::Artist(a) if a.id == "UCzzzzzzzzzzzzzzzzzzzzzz"));
    assert!(matches!(&charts[3].items[0], Item::Playlist(p) if p.id == "PLTRENDING123456789012345678901"));
    assert_eq!(parse::charts(&json!({"contents": {}})), Err(Error::Parse));

    let g = parse::genres(&load("genres")).unwrap();
    assert_eq!(g.len(), 2);
    assert_eq!(g[0].title.as_deref(), Some("Moods & moments"));
    // unusable params and buttons without a click command are dropped
    assert_eq!(g[0].genres, [Genre { name: "Chill".into(), params: "ggMPOg1uX1ZaRGhUa29OSlhr".into() }]);
    assert_eq!(g[1].genres[0].name, "Pop");
    assert_eq!(parse::genres(&json!({})), Err(Error::Parse));
}

#[test]
fn genre_playlists_new_albums_new_videos() {
    let s = parse::genre_playlists(&load("genre_playlists")).unwrap();
    assert_eq!(s.len(), 2);
    assert_eq!(s[0].items.len(), 2);
    let Item::Playlist(p) = &s[0].items[1] else { panic!("not a playlist") };
    assert_eq!(p.author.as_ref().map(|a| a.name.as_str()), Some("Some Curator"));
    let Item::Playlist(p) = &s[1].items[0] else { panic!("not a playlist") };
    assert_eq!((p.author.as_ref().unwrap().name.as_str(), p.count_text.as_deref()), ("Fan", Some("12 songs")));

    let albums = parse::new_albums(&load("new_albums")).unwrap();
    assert_eq!(albums.len(), 2); // the stray video card is not an album
    assert_eq!(albums[0].audio_playlist_id.as_deref(), Some("OLAK5uy_newalbum0123456789012345678901"));
    assert_eq!((albums[0].year, albums[0].kind), (Some(2026), AlbumKind::Album));
    assert!(albums[1].is_explicit && albums[1].kind == AlbumKind::Single);

    let videos = parse::new_videos(&load("new_videos")).unwrap();
    assert_eq!(videos.len(), 2);
    assert_eq!(videos[0].kind, TrackKind::Video);
    assert_eq!(videos[1].artists, [ArtistRef { id: None, name: "Plain Artist".into() }]);
    assert_eq!(parse::new_albums(&json!({})), Err(Error::Parse));
    assert_eq!(parse::new_videos(&json!({})), Err(Error::Parse));
}

#[test]
fn library_pages() {
    let liked = parse::library_tracks(&load("library_liked")).unwrap();
    assert_eq!(liked.items.len(), 2);
    assert_eq!(liked.items[0].duration_ms, Some(225_000));
    assert!(liked.continuation.is_some());

    let pl = parse::library_playlists(&load("library_playlists")).unwrap();
    // "New playlist" button is skipped; "VLLM" becomes the bare id "LM"
    let ids: Vec<_> = pl.items.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["LM", "PLMYMIX1234567890123456789012345"]);
    assert!(pl.continuation.is_some());

    let hist = parse::library_tracks(&load("history")).unwrap();
    assert_eq!(hist.items.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Example Song", "Old Song"]);
    assert!(hist.continuation.is_some());

    // an empty library is fine, a response without contents is not
    assert_eq!(parse::library_tracks(&json!({"contents": {}})).unwrap().items.len(), 0);
    assert_eq!(parse::library_tracks(&json!({})), Err(Error::Parse));
    assert_eq!(parse::library_playlists(&json!({})), Err(Error::Parse));
}

#[test]
fn hostile_ids_never_reach_the_models() {
    // album/artist/playlist links with bad ids are dropped, not passed through
    let bad_album = json!({"musicTwoRowItemRenderer": {"title": {"runs": [{"text": "x"}]}, "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_../../etc"}}}});
    let bad_video = json!({"musicTwoRowItemRenderer": {"title": {"runs": [{"text": "x"}]}, "navigationEndpoint": {"watchEndpoint": {"videoId": "../../../x1"}}}});
    let doc = json!({"contents": [{"gridRenderer": {"items": [bad_album, bad_video]}}]});
    assert_eq!(parse::new_albums(&doc), Err(Error::Parse));
    assert_eq!(parse::new_videos(&doc), Err(Error::Parse));
}
