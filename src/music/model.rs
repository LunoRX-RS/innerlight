//! Typed results of the music API. Own design: plain data, no behaviour beyond small helpers.
//!
//! Choice on non-music content: podcast episodes and livestreams are **skipped** by every parser (they are never
//! returned as a [`Track`]); [`TrackKind`] only separates audio tracks from music videos.

use serde::{Deserialize, Serialize};

/// One image. `width`/`height` are 0 when the service did not say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thumbnail {
    pub url: String,
    pub width: u32,
    pub height: u32,
}

/// A credited artist. `id` is a validated channel id (`UC…`) or `None` when the service gave plain text only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistRef {
    pub id: Option<String>,
    pub name: String,
}

/// The album a track belongs to. `id` is a validated `MPREb_…` browse id or `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumRef {
    pub id: Option<String>,
    pub name: String,
}

/// What kind of media a track is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackKind {
    /// Audio-only release (`MUSIC_VIDEO_TYPE_ATV`).
    Audio,
    /// Official music video, user upload or other video.
    Video,
    /// The response did not say.
    Unknown,
}

/// What one library page held, counted before the non-track rows are dropped. For diagnosing short imports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowTally {
    /// Every `musicResponsiveListItemRenderer` on the page.
    pub rows: usize,
    /// Rows that became a `Track`.
    pub tracks: usize,
    /// Rows with no video id anywhere they could carry one (unavailable, removed or private songs, usually).
    pub no_id: usize,
    /// Rows with a video id that were still not tracks (podcast episodes, livestreams, other kinds).
    pub other: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    /// Validated 11-character video id.
    pub id: String,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub album: Option<AlbumRef>,
    pub duration_ms: Option<u64>,
    pub thumbnails: Vec<Thumbnail>,
    pub is_explicit: bool,
    pub kind: TrackKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlbumKind {
    Album,
    Single,
    Ep,
    /// Unlabelled, or labelled in a language this crate does not know.
    Unknown,
}

/// An album as listed somewhere (search, artist page, new releases).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumSummary {
    /// Validated `MPREb_…` browse id.
    pub id: String,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub year: Option<u16>,
    pub kind: AlbumKind,
    pub thumbnails: Vec<Thumbnail>,
    pub is_explicit: bool,
    /// The audio playlist (`OLAK5uy_…`) that plays the whole album, when listed.
    pub audio_playlist_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Album {
    pub id: String,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub year: Option<u16>,
    pub kind: AlbumKind,
    pub thumbnails: Vec<Thumbnail>,
    pub description: Option<String>,
    pub audio_playlist_id: Option<String>,
    pub track_count: Option<u32>,
    /// Sum of the track durations, only when every track has one.
    pub duration_ms: Option<u64>,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistSummary {
    /// Validated `UC…` channel id.
    pub id: String,
    pub name: String,
    pub thumbnails: Vec<Thumbnail>,
    /// The audience line as shown, e.g. `1.2M subscribers`. Not parsed.
    pub subtitle: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub thumbnails: Vec<Thumbnail>,
    /// Subscriber count text as shown (`1.2M`). Not parsed.
    pub subscribers: Option<String>,
    pub top_songs: Vec<Track>,
    /// Playlist behind "show all" of the top songs, when present.
    pub top_songs_playlist_id: Option<String>,
    pub albums: Vec<AlbumSummary>,
    /// Singles and EPs (recognised by English labels; other locales land in `albums`).
    pub singles: Vec<AlbumSummary>,
    pub related: Vec<ArtistSummary>,
}

/// A playlist as listed somewhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistSummary {
    /// Bare playlist id (no `VL` prefix).
    pub id: String,
    pub title: String,
    pub author: Option<ArtistRef>,
    pub thumbnails: Vec<Thumbnail>,
    /// The "25 songs" style line as shown, when present. Not parsed.
    pub count_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub title: String,
    pub author: Option<ArtistRef>,
    pub description: Option<String>,
    pub thumbnails: Vec<Thumbnail>,
    /// From the header ("25 songs"), English only.
    pub track_count: Option<u32>,
    /// First page of tracks; use `Music::playlist_more` with the continuation for the rest.
    pub tracks: Page<Track>,
}

/// One page of items plus the opaque handle for the next page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub continuation: Option<String>,
}

/// Anything a list or shelf can hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Item {
    Track(Track),
    Album(AlbumSummary),
    Artist(ArtistSummary),
    Playlist(PlaylistSummary),
}

/// A titled shelf (carousel, grid or list).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub title: Option<String>,
    pub items: Vec<Item>,
}

/// A mood or genre button. `params` is the opaque value to hand to `Music::genre_playlists`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Genre {
    pub name: String,
    pub params: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenreGroup {
    pub title: Option<String>,
    pub genres: Vec<Genre>,
}

/// Result of the `next` endpoint for one video: the radio queue plus ids of the lyrics and related tabs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Radio {
    /// The queue; the first entry is normally the requested track itself.
    pub queue: Vec<Track>,
    pub playlist_id: Option<String>,
    /// `MPLYt_…`, present when the track has lyrics.
    pub lyrics_browse_id: Option<String>,
    /// `MPTRt_…`, present when the related tab exists.
    pub related_browse_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LyricLine {
    pub start_ms: u64,
    pub end_ms: Option<u64>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lyrics {
    /// The whole text, lines joined with `\n`.
    pub text: String,
    /// Timed lines; empty when the response only had plain text.
    pub lines: Vec<LyricLine>,
    /// The attribution as shown (`Source: …`), when the response has one.
    pub source: Option<String>,
}

impl Lyrics {
    pub fn is_timed(&self) -> bool {
        !self.lines.is_empty()
    }
}
