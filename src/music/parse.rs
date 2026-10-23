//! Pure parsers: a response `Value` in, a typed result out. No I/O, never a panic.
//!
//! Where an empty result would hide a broken or changed response, the parser returns `Error::Parse`
//! (for example an album page with neither header nor tracks). Where emptiness is normal (a search with no hits,
//! an empty library) it returns an empty page, but only when the response has the expected top-level key.

use super::ids;
use super::item::{self, find_continuation, panel_track, scan, sections, thumbs, tracks_in};
use super::model::*;
use crate::{Error, jsonx};
use serde_json::Value;

fn require_any(v: &Value, keys: &[&str]) -> Result<(), Error> {
    if keys.iter().any(|k| v.get(*k).is_some()) { Ok(()) } else { Err(Error::Parse) }
}

fn runs_of(v: &Value) -> Vec<&Value> {
    v.get("runs").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default()
}

// ---------------------------------------------------------------- search

/// A song search (first page). Items that are not music tracks are skipped.
pub fn search_tracks(v: &Value) -> Result<Page<Track>, Error> {
    require_any(v, &["contents", "continuationContents", "onResponseReceivedActions"])?;
    Ok(Page { items: tracks_in(v), continuation: find_continuation(v) })
}

/// Any later page of tracks (search, playlist, library, history): rows plus the next handle.
pub fn track_page(v: &Value) -> Result<Page<Track>, Error> {
    require_any(v, &["contents", "continuationContents", "onResponseReceivedActions"])?;
    Ok(Page { items: tracks_in(v), continuation: find_continuation(v) })
}

/// Autocomplete strings from `music/get_search_suggestions`. Signed-in history suggestions are included too.
pub fn search_suggestions(v: &Value) -> Result<Vec<String>, Error> {
    require_any(v, &["contents"])?;
    let mut out = Vec::new();
    for key in ["searchSuggestionRenderer", "historySuggestionRenderer"] {
        for n in jsonx::find_all(v, key) {
            let q = jsonx::get(n, "navigationEndpoint.searchEndpoint.query")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| n.get("suggestion").and_then(jsonx::text));
            if let Some(q) = q.filter(|q| !q.trim().is_empty()) {
                out.push(q);
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- next: radio queue and tab ids

/// The `next` response: the queue (primary renderers only, never the video "counterpart") and the lyrics/related ids.
pub fn radio(v: &Value) -> Result<Radio, Error> {
    let panel = jsonx::find_all(v, "playlistPanelRenderer").into_iter().next();
    let queue: Vec<Track> = panel
        .and_then(|p| p.get("contents")?.as_array())
        .map(|c| {
            c.iter()
                .filter_map(|e| {
                    e.get("playlistPanelVideoRenderer").or_else(|| {
                        jsonx::get(e, "playlistPanelVideoWrapperRenderer.primaryRenderer.playlistPanelVideoRenderer")
                    })
                })
                .filter_map(panel_track)
                .collect()
        })
        .unwrap_or_default();
    let lyrics_browse_id = browse_ids(v).find(|b| ids::is_lyrics_browse_id(b)).map(str::to_owned);
    let related_browse_id = browse_ids(v).find(|b| ids::is_related_browse_id(b)).map(str::to_owned);
    if panel.is_none() && lyrics_browse_id.is_none() && related_browse_id.is_none() {
        return Err(Error::Parse);
    }
    let playlist_id =
        panel.and_then(|p| p.get("playlistId")?.as_str()).filter(|p| ids::is_music_playlist_id(p)).map(str::to_owned);
    Ok(Radio { queue, playlist_id, lyrics_browse_id, related_browse_id })
}

fn browse_ids(v: &Value) -> impl Iterator<Item = &str> {
    jsonx::find_all(v, "browseId").into_iter().filter_map(Value::as_str)
}

// ---------------------------------------------------------------- lyrics

fn millis(v: Option<&Value>) -> Option<u64> {
    let v = v?;
    v.as_u64().or_else(|| v.as_str()?.trim().parse().ok())
}

/// Lyrics from a browse response. Timed lines (`timedLyricsData`) win when present; otherwise the plain text of the
/// description shelf. `Error::Parse` when the response has neither.
pub fn lyrics(v: &Value) -> Result<Lyrics, Error> {
    let source = jsonx::find_all(v, "sourceMessage").into_iter().find_map(jsonx::text).or_else(|| {
        jsonx::find_all(v, "musicDescriptionShelfRenderer").into_iter().find_map(|s| jsonx::text(s.get("footer")?))
    });
    if let Some(data) = jsonx::find_all(v, "timedLyricsData").into_iter().find_map(Value::as_array) {
        let lines: Vec<LyricLine> = data
            .iter()
            .filter_map(|l| {
                Some(LyricLine {
                    start_ms: millis(jsonx::get(l, "cueRange.startTimeMilliseconds"))?,
                    end_ms: millis(jsonx::get(l, "cueRange.endTimeMilliseconds")),
                    text: l.get("lyricLine")?.as_str()?.to_owned(),
                })
            })
            .collect();
        if !lines.is_empty() {
            let text = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
            return Ok(Lyrics { text, lines, source });
        }
    }
    let text = jsonx::find_all(v, "musicDescriptionShelfRenderer")
        .into_iter()
        .find_map(|s| jsonx::text(s.get("description")?));
    match text {
        Some(text) => Ok(Lyrics { text, lines: Vec::new(), source }),
        None => Err(Error::Parse),
    }
}

// ---------------------------------------------------------------- headers (album, playlist)

fn header_of(v: &Value) -> Option<&Value> {
    ["musicResponsiveHeaderRenderer", "musicDetailHeaderRenderer"]
        .iter()
        .find_map(|k| jsonx::find_all(v, k).into_iter().next())
}

fn description_of(h: &Value) -> Option<String> {
    jsonx::find_all_depth(h, "description", 5).into_iter().find_map(jsonx::text)
}

/// "25 songs" style count in the second subtitle (English words only).
fn count_in(runs: &[&Value]) -> Option<u32> {
    runs.iter().find_map(|r| {
        let t = r.get("text")?.as_str()?;
        let (n, w) = t.trim().split_once(' ')?;
        matches!(w.trim().to_lowercase().as_str(), "song" | "songs" | "track" | "tracks" | "video" | "videos")
            .then(|| n.replace([',', '.'], "").parse().ok())?
    })
}

/// Artists of a header: strapline first (new layout), then the subtitle (old layout).
fn header_artists(h: &Value, sub: &item::Sub) -> Vec<ArtistRef> {
    let strap = scan(&h.get("straplineTextOne").map(runs_of).unwrap_or_default());
    let a = strap.artists_or_plain();
    if a.is_empty() { sub.artists_or_plain() } else { a }
}

/// Track rows and next handle of the page's main shelf (`keys` in order of preference), so that side shelves such as
/// "Suggestions" are not mixed in. Falls back to the whole document when no such shelf exists.
fn main_shelf_tracks(v: &Value, keys: &[&str]) -> (Vec<Track>, Option<String>) {
    let scope = keys.iter().find_map(|k| jsonx::find_all(v, k).into_iter().next()).unwrap_or(v);
    (tracks_in(scope), find_continuation(scope))
}

// ---------------------------------------------------------------- album

/// An album page. `id` is the browse id that was requested.
pub fn album(v: &Value, id: &str) -> Result<Album, Error> {
    let header = header_of(v);
    let (mut tracks, _) = main_shelf_tracks(v, &["musicShelfRenderer"]);
    if header.is_none() && tracks.is_empty() {
        return Err(Error::Parse);
    }
    let sub = scan(&header.and_then(|h| h.get("subtitle")).map(runs_of).unwrap_or_default());
    let title = header
        .and_then(|h| h.get("title"))
        .and_then(jsonx::text)
        .or_else(|| tracks.iter().find_map(|t| t.album.as_ref().map(|a| a.name.clone())))
        .ok_or(Error::Parse)?;
    let artists = header.map(|h| header_artists(h, &sub)).unwrap_or_default();
    let album_ref = AlbumRef { id: Some(id.to_owned()), name: title.clone() };
    for t in &mut tracks {
        t.album.get_or_insert_with(|| album_ref.clone());
        if t.artists.is_empty() {
            t.artists = artists.clone();
        }
    }
    let duration_ms = tracks.iter().map(|t| t.duration_ms).sum::<Option<u64>>().filter(|_| !tracks.is_empty());
    let second = header.and_then(|h| h.get("secondSubtitle")).map(runs_of).unwrap_or_default();
    Ok(Album {
        id: id.to_owned(),
        title,
        artists,
        year: sub.year,
        kind: match sub.label.as_deref().map(str::to_lowercase).as_deref() {
            Some("single") => AlbumKind::Single,
            Some("ep") => AlbumKind::Ep,
            Some("album") => AlbumKind::Album,
            _ => AlbumKind::Unknown,
        },
        thumbnails: header.and_then(|h| h.get("thumbnail")).map(thumbs).unwrap_or_default(),
        description: header.and_then(description_of),
        audio_playlist_id: header
            .into_iter()
            .chain(std::iter::once(v))
            .find_map(|scope| {
                jsonx::find_all(scope, "playlistId")
                    .into_iter()
                    .filter_map(Value::as_str)
                    .find(|p| p.starts_with("OLAK5uy_") && ids::is_music_playlist_id(p))
            })
            .map(str::to_owned),
        track_count: count_in(&second).or(u32::try_from(tracks.len()).ok().filter(|n| *n > 0)),
        duration_ms,
        tracks,
    })
}

// ---------------------------------------------------------------- artist

/// An artist page. `id` is the channel id that was requested.
pub fn artist(v: &Value, id: &str) -> Result<Artist, Error> {
    let header = ["musicImmersiveHeaderRenderer", "musicVisualHeaderRenderer"]
        .iter()
        .find_map(|k| jsonx::find_all(v, k).into_iter().next());
    let name = header.and_then(|h| h.get("title")).and_then(jsonx::text).ok_or(Error::Parse)?;
    let me = ArtistRef { id: Some(id.to_owned()), name: name.clone() };

    let top = jsonx::find_all(v, "musicShelfRenderer").into_iter().next();
    let mut top_songs: Vec<Track> = top
        .and_then(|s| s.get("contents")?.as_array())
        .map(|c| {
            c.iter()
                .filter_map(|e| e.get("musicResponsiveListItemRenderer"))
                .filter_map(item::list_item)
                .filter_map(into_track)
                .collect()
        })
        .unwrap_or_default();
    for t in &mut top_songs {
        if t.artists.is_empty() {
            t.artists = vec![me.clone()];
        }
    }
    let top_songs_playlist_id = top
        .and_then(|s| jsonx::get(s, "title.runs[0].navigationEndpoint.browseEndpoint.browseId")?.as_str())
        .and_then(|b| b.strip_prefix("VL"))
        .filter(|p| ids::is_music_playlist_id(p))
        .map(str::to_owned);

    let (mut albums, mut singles, mut related) = (Vec::new(), Vec::new(), Vec::new());
    for item in sections(v).into_iter().flat_map(|s| s.items) {
        match item {
            Item::Album(mut a) => {
                if a.artists.is_empty() {
                    a.artists = vec![me.clone()];
                }
                if matches!(a.kind, AlbumKind::Single | AlbumKind::Ep) { singles.push(a) } else { albums.push(a) }
            }
            Item::Artist(a) if a.id != id => related.push(a),
            _ => {}
        }
    }
    let subscribers = header.and_then(|h| {
        jsonx::find_all(h, "subscriberCountText")
            .into_iter()
            .chain(jsonx::find_all(h, "longSubscriberCountText"))
            .find_map(jsonx::text)
    });
    let description = header.and_then(|h| h.get("description")).and_then(jsonx::text).or_else(|| {
        jsonx::find_all(v, "musicDescriptionShelfRenderer").into_iter().find_map(|s| jsonx::text(s.get("description")?))
    });
    Ok(Artist {
        id: id.to_owned(),
        name,
        description,
        thumbnails: header
            .and_then(|h| h.get("thumbnail").or_else(|| h.get("foregroundThumbnail")))
            .map(thumbs)
            .unwrap_or_default(),
        subscribers,
        top_songs,
        top_songs_playlist_id,
        albums,
        singles,
        related,
    })
}

fn into_track(i: Item) -> Option<Track> {
    match i {
        Item::Track(t) => Some(t),
        _ => None,
    }
}

// ---------------------------------------------------------------- playlist

/// A playlist page (first page). `id` is the bare playlist id that was requested.
pub fn playlist(v: &Value, id: &str) -> Result<Playlist, Error> {
    let header = header_of(v);
    let (tracks, continuation) = main_shelf_tracks(v, &["musicPlaylistShelfRenderer", "musicShelfRenderer"]);
    if header.is_none() && tracks.is_empty() {
        return Err(Error::Parse);
    }
    let sub = scan(&header.and_then(|h| h.get("subtitle")).map(runs_of).unwrap_or_default());
    let title = header.and_then(|h| h.get("title")).and_then(jsonx::text).ok_or(Error::Parse)?;
    let second = header.and_then(|h| h.get("secondSubtitle")).map(runs_of).unwrap_or_default();
    Ok(Playlist {
        id: id.to_owned(),
        title,
        author: header.and_then(|h| header_artists(h, &sub).into_iter().next()),
        description: header.and_then(description_of),
        thumbnails: header.and_then(|h| h.get("thumbnail")).map(thumbs).unwrap_or_default(),
        track_count: count_in(&second),
        tracks: Page { items: tracks, continuation },
    })
}

// ---------------------------------------------------------------- explore

/// Shelves of a charts page. `Error::Parse` when none are found.
pub fn charts(v: &Value) -> Result<Vec<Section>, Error> {
    non_empty(sections(v))
}

/// Playlist shelves of one mood/genre page. `Error::Parse` when none are found.
pub fn genre_playlists(v: &Value) -> Result<Vec<Section>, Error> {
    non_empty(sections(v))
}

fn non_empty<T>(v: Vec<T>) -> Result<Vec<T>, Error> {
    if v.is_empty() { Err(Error::Parse) } else { Ok(v) }
}

/// The "Moods & genres" page: groups of buttons. Buttons with unusable `params` are dropped.
pub fn genres(v: &Value) -> Result<Vec<GenreGroup>, Error> {
    let groups: Vec<GenreGroup> = jsonx::find_all(v, "gridRenderer")
        .into_iter()
        .filter_map(|g| {
            let genres: Vec<Genre> = g
                .get("items")?
                .as_array()?
                .iter()
                .filter_map(|i| {
                    let b = i.get("musicNavigationButtonRenderer")?;
                    let params = jsonx::get(b, "clickCommand.browseEndpoint.params")?
                        .as_str()
                        .filter(|p| ids::is_genre_params(p))?;
                    Some(Genre { name: jsonx::text(b.get("buttonText")?)?, params: params.to_owned() })
                })
                .collect();
            (!genres.is_empty()).then(|| GenreGroup {
                title: jsonx::get(g, "header.gridHeaderRenderer.title").and_then(jsonx::text),
                genres,
            })
        })
        .collect();
    non_empty(groups)
}

/// The "New albums & singles" page.
pub fn new_albums(v: &Value) -> Result<Vec<AlbumSummary>, Error> {
    non_empty(
        sections(v)
            .into_iter()
            .flat_map(|s| s.items)
            .filter_map(|i| match i {
                Item::Album(a) => Some(a),
                _ => None,
            })
            .collect(),
    )
}

/// The "New music videos" page (video cards become [`Track`]s of kind `Video`).
pub fn new_videos(v: &Value) -> Result<Vec<Track>, Error> {
    non_empty(sections(v).into_iter().flat_map(|s| s.items).filter_map(into_track).collect())
}

// ---------------------------------------------------------------- library

/// Liked songs or history: rows plus the next handle. An empty library is `Ok` with no items.
pub fn library_tracks(v: &Value) -> Result<Page<Track>, Error> {
    track_page(v)
}

/// `library_tracks` plus the row tally of the same page.
pub fn library_tracks_tally(v: &Value) -> Result<(Page<Track>, RowTally), Error> {
    let page = track_page(v)?;
    Ok((page, item::tally_rows(v)))
}

/// Saved playlists (a grid of cards). The "new playlist" button and other non-playlist cards are skipped.
pub fn library_playlists(v: &Value) -> Result<Page<PlaylistSummary>, Error> {
    require_any(v, &["contents", "continuationContents", "onResponseReceivedActions"])?;
    let items = sections(v)
        .into_iter()
        .flat_map(|s| s.items)
        .filter_map(|i| match i {
            Item::Playlist(p) => Some(p),
            _ => None,
        })
        .collect();
    Ok(Page { items, continuation: find_continuation(v) })
}
