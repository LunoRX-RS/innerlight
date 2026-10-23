//! Parsing of the item renderers that every page shares: `musicResponsiveListItemRenderer` (rows),
//! `musicTwoRowItemRenderer` (cards), `playlistPanelVideoRenderer` (queue entries), shelves and continuation handles.
//! Everything is lenient: a malformed node yields `None` or fewer items, never a panic.

use super::ids;
use super::model::*;
use crate::ids as core_ids;
use crate::jsonx;
use serde_json::Value;

const ARTIST_PAGES: [&str; 2] = ["MUSIC_PAGE_TYPE_ARTIST", "MUSIC_PAGE_TYPE_USER_CHANNEL"];
/// Subtitle labels the first run may carry (English only; other languages simply leave `label` empty).
const LABELS: [&str; 10] =
    ["song", "video", "album", "single", "ep", "playlist", "artist", "profile", "episode", "podcast"];
const STAT_WORDS: [&str; 16] = [
    "views",
    "view",
    "plays",
    "play",
    "subscribers",
    "subscriber",
    "listeners",
    "likes",
    "songs",
    "song",
    "tracks",
    "videos",
    "minutes",
    "minute",
    "hours",
    "hour",
];
/// Node names that hold a list of items.
const SHELVES: [&str; 8] = [
    "musicCarouselShelfRenderer",
    "musicShelfRenderer",
    "musicPlaylistShelfRenderer",
    "gridRenderer",
    "musicShelfContinuation",
    "musicPlaylistShelfContinuation",
    "gridContinuation",
    "continuationItems",
];

// ---------------------------------------------------------------- small helpers

fn runs_of(v: &Value) -> Vec<&Value> {
    v.get("runs").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default()
}

fn is_sep(t: &str) -> bool {
    matches!(t, "•" | "&" | "," | "·" | "|" | "-")
}

/// `m:ss` or `h:mm:ss` to milliseconds. Needs at least two parts, every part 1..=2 digits except the first (1..=3).
pub(crate) fn parse_duration_ms(t: &str) -> Option<u64> {
    let parts: Vec<&str> = t.trim().split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let mut secs = 0u64;
    for (i, p) in parts.iter().enumerate() {
        let max = if i == 0 { 3 } else { 2 };
        if p.is_empty() || p.len() > max || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        secs = secs * 60 + p.parse::<u64>().ok()?;
    }
    Some(secs * 1000)
}

fn parse_year(t: &str) -> Option<u16> {
    let y: u16 = t.parse().ok().filter(|_| t.len() == 4)?;
    (1900..=2100).contains(&y).then_some(y)
}

fn is_stat(t: &str) -> bool {
    t.to_lowercase().rsplit_once(' ').is_some_and(|(_, w)| STAT_WORDS.contains(&w))
}

/// All thumbnails under `v` (the list itself, `{"thumbnails":[..]}` or any wrapper around it), source order.
pub(crate) fn thumbs(v: &Value) -> Vec<Thumbnail> {
    let list = v.as_array().or_else(|| jsonx::find_all_depth(v, "thumbnails", 8).into_iter().find_map(Value::as_array));
    list.map(|l| {
        l.iter()
            .filter_map(|t| {
                let url = t.get("url")?.as_str().filter(|u| !u.is_empty())?;
                let n = |k: &str| t.get(k).and_then(Value::as_u64).map_or(0, |n| n.min(u64::from(u32::MAX)) as u32);
                let url = if url.starts_with("//") { format!("https:{url}") } else { url.to_owned() };
                Some(Thumbnail { url, width: n("width"), height: n("height") })
            })
            .collect()
    })
    .unwrap_or_default()
}

fn badge_icons(v: &Value) -> Vec<&str> {
    ["badges", "subtitleBadges"]
        .iter()
        .filter_map(|k| v.get(*k)?.as_array())
        .flatten()
        .flat_map(|b| jsonx::find_all_depth(b, "iconType", 6))
        .filter_map(Value::as_str)
        .collect()
}

fn first_music_video_type(v: &Value) -> Option<&str> {
    jsonx::find_all_depth(v, "musicVideoType", 14).into_iter().find_map(Value::as_str)
}

/// `(browseId, pageType)` of the `browseEndpoint` stored at `v.navigationEndpoint`.
fn browse_of(v: &Value) -> Option<(&str, &str)> {
    let b = v.get("navigationEndpoint")?.get("browseEndpoint")?;
    let id = b.get("browseId")?.as_str()?;
    let page = jsonx::get(b, "browseEndpointContextSupportedConfigs.browseEndpointContextMusicConfig.pageType")
        .and_then(Value::as_str)
        .unwrap_or("");
    Some((id, page))
}

#[derive(PartialEq, Eq)]
enum Link {
    Artist,
    Album,
    Playlist,
    Other,
}

fn link_kind(id: &str, page: &str) -> Link {
    if page == "MUSIC_PAGE_TYPE_ALBUM" || id.starts_with("MPREb_") {
        Link::Album
    } else if ARTIST_PAGES.contains(&page) || core_ids::is_channel_id(id) {
        Link::Artist
    } else if page == "MUSIC_PAGE_TYPE_PLAYLIST" || id.starts_with("VL") {
        Link::Playlist
    } else {
        Link::Other
    }
}

// ---------------------------------------------------------------- run scanning

/// What a byline (a list of runs such as `Artist • Album • 3:45`) contained.
#[derive(Default)]
pub(crate) struct Sub {
    pub label: Option<String>,
    pub artists: Vec<ArtistRef>,
    pub album: Option<AlbumRef>,
    pub year: Option<u16>,
    pub duration_ms: Option<u64>,
    /// Unlinked text that was not a separator, label, year or duration.
    pub plain: Vec<String>,
}

impl Sub {
    /// Linked artists, else the first unlinked text that is not a statistic ("1.2M views").
    pub(crate) fn artists_or_plain(&self) -> Vec<ArtistRef> {
        if !self.artists.is_empty() {
            return self.artists.clone();
        }
        self.plain
            .iter()
            .find(|p| !is_stat(p))
            .map(|n| vec![ArtistRef { id: None, name: n.clone() }])
            .unwrap_or_default()
    }
}

pub(crate) fn scan(runs: &[&Value]) -> Sub {
    let mut s = Sub::default();
    let mut first = true;
    for r in runs {
        let t = r.get("text").and_then(Value::as_str).unwrap_or("").trim();
        if t.is_empty() || is_sep(t) {
            continue;
        }
        let was_first = std::mem::replace(&mut first, false);
        if let Some((id, page)) = browse_of(r) {
            match link_kind(id, page) {
                Link::Artist => {
                    s.artists
                        .push(ArtistRef { id: core_ids::is_channel_id(id).then(|| id.to_owned()), name: t.to_owned() });
                    continue;
                }
                Link::Album => {
                    if s.album.is_none() {
                        s.album = Some(AlbumRef {
                            id: ids::is_album_browse_id(id).then(|| id.to_owned()),
                            name: t.to_owned(),
                        });
                    }
                    continue;
                }
                _ => {}
            }
        }
        if let Some(d) = parse_duration_ms(t) {
            s.duration_ms.get_or_insert(d);
        } else if let Some(y) = parse_year(t) {
            s.year.get_or_insert(y);
        } else if was_first && LABELS.contains(&t.to_lowercase().as_str()) {
            s.label = Some(t.to_owned());
        } else {
            s.plain.push(t.to_owned());
        }
    }
    s
}

fn album_kind(label: Option<&str>) -> AlbumKind {
    match label.map(str::to_lowercase).as_deref() {
        Some("album") => AlbumKind::Album,
        Some("single") => AlbumKind::Single,
        Some("ep") => AlbumKind::Ep,
        _ => AlbumKind::Unknown,
    }
}

/// `None` means "not a music track, skip it" (podcast episode, livestream).
fn classify(vtype: Option<&str>, icons: &[&str], plain: &[String]) -> Option<TrackKind> {
    if icons.iter().any(|i| i.contains("LIVE")) || plain.iter().any(|p| p == "LIVE") {
        return None;
    }
    match vtype {
        Some(t) if ["PODCAST", "EPISODE", "NON_MUSIC"].iter().any(|w| t.contains(w)) => None,
        Some("MUSIC_VIDEO_TYPE_ATV") => Some(TrackKind::Audio),
        Some("MUSIC_VIDEO_TYPE_OMV" | "MUSIC_VIDEO_TYPE_UGC" | "MUSIC_VIDEO_TYPE_OFFICIAL_SOURCE_MUSIC") => {
            Some(TrackKind::Video)
        }
        _ => Some(TrackKind::Unknown),
    }
}

/// First playlist id under `v` that passes validation and starts with `prefix`.
fn playlist_id_in(v: &Value, prefix: &str) -> Option<String> {
    jsonx::find_all_depth(v, "playlistId", 12)
        .into_iter()
        .filter_map(Value::as_str)
        .find(|p| p.starts_with(prefix) && ids::is_music_playlist_id(p))
        .map(str::to_owned)
}

// ---------------------------------------------------------------- list items (rows)

/// One `musicResponsiveListItemRenderer`: a track, album, artist or playlist row. `None` for anything else
/// (podcast shows and episodes, livestreams, rows without a usable id).
pub(crate) fn list_item(r: &Value) -> Option<Item> {
    let cols: Vec<&Value> = r
        .get("flexColumns")?
        .as_array()?
        .iter()
        .filter_map(|c| c.get("musicResponsiveListItemFlexColumnRenderer")?.get("text"))
        .collect();
    let title = jsonx::text(cols.first()?)?;
    let mut runs: Vec<&Value> = cols.iter().skip(1).flat_map(|c| runs_of(c)).collect();
    if let Some(fixed) = r.get("fixedColumns").and_then(Value::as_array) {
        for c in fixed {
            if let Some(t) = jsonx::get(c, "musicResponsiveListItemFixedColumnRenderer.text") {
                runs.extend(runs_of(t));
            }
        }
    }
    let sub = scan(&runs);
    let thumbnails = r.get("thumbnail").map(thumbs).unwrap_or_default();
    let explicit = badge_icons(r).contains(&"MUSIC_EXPLICIT_BADGE");

    if let Some((id, page)) = browse_of(r) {
        return match link_kind(id, page) {
            Link::Album => Some(Item::Album(AlbumSummary {
                id: ids::is_album_browse_id(id).then(|| id.to_owned())?,
                title,
                artists: sub.artists_or_plain(),
                year: sub.year,
                kind: album_kind(sub.label.as_deref()),
                thumbnails,
                is_explicit: explicit,
                audio_playlist_id: playlist_id_in(r, "OLAK5uy_"),
            })),
            Link::Artist => Some(Item::Artist(ArtistSummary {
                id: core_ids::is_channel_id(id).then(|| id.to_owned())?,
                name: title,
                thumbnails,
                subtitle: sub.plain.iter().find(|p| is_stat(p)).cloned(),
            })),
            Link::Playlist => Some(Item::Playlist(PlaylistSummary {
                id: playlist_from_browse(id)?,
                title,
                author: sub.artists_or_plain().into_iter().next(),
                thumbnails,
                count_text: sub.plain.iter().find(|p| is_stat(p)).cloned(),
            })),
            Link::Other => None,
        };
    }

    let id = [
        jsonx::get(r, "playlistItemData.videoId"),
        jsonx::get(r, "overlay.musicItemThumbnailOverlayRenderer.content.musicPlayButtonRenderer.playNavigationEndpoint.watchEndpoint.videoId"),
        jsonx::get(cols[0], "runs[0].navigationEndpoint.watchEndpoint.videoId"),
        jsonx::get(r, "navigationEndpoint.watchEndpoint.videoId"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .find(|i| core_ids::is_video_id(i))?
    .to_owned();
    let kind = classify(first_music_video_type(r), &badge_icons(r), &sub.plain)?;
    Some(Item::Track(Track {
        id,
        title,
        artists: sub.artists_or_plain(),
        album: sub.album,
        duration_ms: sub.duration_ms,
        thumbnails,
        is_explicit: explicit,
        kind,
    }))
}

fn playlist_from_browse(id: &str) -> Option<String> {
    let bare = id.strip_prefix("VL").unwrap_or(id);
    ids::is_music_playlist_id(bare).then(|| bare.to_owned())
}

// ---------------------------------------------------------------- two-row items (cards)

/// One `musicTwoRowItemRenderer`: album, artist, playlist or video card.
pub(crate) fn two_row(r: &Value) -> Option<Item> {
    let title = jsonx::text(r.get("title")?)?;
    let sub = scan(&r.get("subtitle").map(runs_of).unwrap_or_default());
    let thumbnails = r.get("thumbnailRenderer").map(thumbs).unwrap_or_default();
    let explicit = badge_icons(r).contains(&"MUSIC_EXPLICIT_BADGE");

    if let Some((id, page)) = browse_of(r) {
        return match link_kind(id, page) {
            Link::Album => Some(Item::Album(AlbumSummary {
                id: ids::is_album_browse_id(id).then(|| id.to_owned())?,
                title,
                artists: sub.artists_or_plain(),
                year: sub.year,
                kind: album_kind(sub.label.as_deref()),
                thumbnails,
                is_explicit: explicit,
                audio_playlist_id: playlist_id_in(r, "OLAK5uy_"),
            })),
            Link::Artist => Some(Item::Artist(ArtistSummary {
                id: core_ids::is_channel_id(id).then(|| id.to_owned())?,
                name: title,
                thumbnails,
                subtitle: sub.plain.iter().find(|p| is_stat(p)).cloned(),
            })),
            Link::Playlist => Some(Item::Playlist(PlaylistSummary {
                id: playlist_from_browse(id)?,
                title,
                author: sub.artists_or_plain().into_iter().next(),
                thumbnails,
                count_text: sub.plain.iter().find(|p| is_stat(p)).cloned(),
            })),
            Link::Other => None,
        };
    }
    if let Some(w) = jsonx::get(r, "navigationEndpoint.watchEndpoint") {
        let id = w.get("videoId")?.as_str().filter(|i| core_ids::is_video_id(i))?.to_owned();
        let kind = classify(first_music_video_type(w), &badge_icons(r), &sub.plain)?;
        return Some(Item::Track(Track {
            id,
            title,
            artists: sub.artists_or_plain(),
            album: sub.album,
            duration_ms: sub.duration_ms,
            thumbnails,
            is_explicit: explicit,
            kind: if kind == TrackKind::Unknown { TrackKind::Video } else { kind },
        }));
    }
    if let Some(p) = jsonx::get(r, "navigationEndpoint.watchPlaylistEndpoint.playlistId").and_then(Value::as_str) {
        return Some(Item::Playlist(PlaylistSummary {
            id: ids::is_music_playlist_id(p).then(|| p.to_owned())?,
            title,
            author: sub.artists_or_plain().into_iter().next(),
            thumbnails,
            count_text: sub.plain.iter().find(|p| is_stat(p)).cloned(),
        }));
    }
    None
}

/// One element of a shelf: a row or a card. Anything else (buttons, continuation markers, podcast rows) is `None`.
pub(crate) fn element_item(e: &Value) -> Option<Item> {
    if let Some(r) = e.get("musicResponsiveListItemRenderer") {
        list_item(r)
    } else if let Some(r) = e.get("musicTwoRowItemRenderer") {
        two_row(r)
    } else {
        None
    }
}

/// Every track row anywhere under `v`, in document order (non-track rows and skipped media are dropped).
pub(crate) fn tracks_in(v: &Value) -> Vec<Track> {
    jsonx::find_all(v, "musicResponsiveListItemRenderer")
        .into_iter()
        .filter_map(|r| match list_item(r) {
            Some(Item::Track(t)) => Some(t),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------- queue entries

/// One `playlistPanelVideoRenderer` (radio queue entry).
pub(crate) fn panel_track(r: &Value) -> Option<Track> {
    let title = jsonx::text(r.get("title")?)?;
    let by = r.get("longBylineText").or_else(|| r.get("shortBylineText")).map(runs_of).unwrap_or_default();
    let sub = scan(&by);
    let id = [r.get("videoId"), jsonx::get(r, "navigationEndpoint.watchEndpoint.videoId")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find(|i| core_ids::is_video_id(i))?
        .to_owned();
    let kind = classify(r.get("navigationEndpoint").and_then(first_music_video_type), &badge_icons(r), &sub.plain)?;
    let duration_ms = r.get("lengthText").and_then(jsonx::text).and_then(|t| parse_duration_ms(&t)).or(sub.duration_ms);
    Some(Track {
        id,
        title,
        artists: sub.artists_or_plain(),
        album: sub.album,
        duration_ms,
        thumbnails: r.get("thumbnail").map(thumbs).unwrap_or_default(),
        is_explicit: badge_icons(r).contains(&"MUSIC_EXPLICIT_BADGE"),
        kind,
    })
}

// ---------------------------------------------------------------- shelves and continuations

fn shelf_title(n: &Value) -> Option<String> {
    ["header.musicCarouselShelfBasicHeaderRenderer.title", "header.gridHeaderRenderer.title", "title"]
        .iter()
        .find_map(|p| jsonx::get(n, p).and_then(jsonx::text))
}

/// Every shelf in document order, with its items. Empty shelves are dropped. Does not look inside a shelf for
/// nested shelves.
pub(crate) fn sections(v: &Value) -> Vec<Section> {
    fn walk(v: &Value, depth: usize, out: &mut Vec<Section>) {
        if depth > jsonx::DEFAULT_MAX_DEPTH {
            return;
        }
        match v {
            Value::Object(m) => {
                for (k, c) in m {
                    if SHELVES.contains(&k.as_str()) {
                        let list = c
                            .as_array()
                            .or_else(|| c.get("contents")?.as_array())
                            .or_else(|| c.get("items")?.as_array());
                        let items: Vec<Item> =
                            list.map(|l| l.iter().filter_map(element_item).collect()).unwrap_or_default();
                        if !items.is_empty() {
                            out.push(Section { title: shelf_title(c), items });
                        }
                    } else {
                        walk(c, depth + 1, out);
                    }
                }
            }
            Value::Array(a) => a.iter().for_each(|c| walk(c, depth + 1, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(v, 0, &mut out);
    out
}

/// The handle for the next page: `nextContinuationData.continuation` or a `continuationItemRenderer` token,
/// validated. `None` on the last page.
pub(crate) fn find_continuation(v: &Value) -> Option<String> {
    let old = jsonx::find_all(v, "nextContinuationData").into_iter().filter_map(|n| n.get("continuation")?.as_str());
    let new = jsonx::find_all(v, "continuationItemRenderer")
        .into_iter()
        .filter_map(|n| jsonx::get(n, "continuationEndpoint.continuationCommand.token")?.as_str());
    old.chain(new).find(|t| ids::is_continuation_token(t)).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration_ms("3:45"), Some(225_000));
        assert_eq!(parse_duration_ms("1:02:03"), Some(3_723_000));
        assert_eq!(parse_duration_ms("0:07"), Some(7_000));
        for bad in ["", "45", "3:4x", "3:456", ":45", "1:2:3:4", "12 songs", "1.2M views", "3:"] {
            assert_eq!(parse_duration_ms(bad), None, "{bad}");
        }
    }

    #[test]
    fn stats_and_years() {
        assert!(is_stat("1.2M views") && is_stat("25 songs") && is_stat("3.4M subscribers"));
        assert!(!is_stat("Maroon 5") && !is_stat("views"));
        assert_eq!(parse_year("2021"), Some(2021));
        assert_eq!(parse_year("0042"), None);
        assert_eq!(parse_year("20211"), None);
    }
}
