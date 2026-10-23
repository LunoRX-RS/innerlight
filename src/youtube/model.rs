//! Types returned by the `youtube` module. Independent of the `music` module's types.

use serde::{Deserialize, Serialize};

/// One page of a listing. `continuation` is the opaque token for the next page (pass it back unchanged), `None` on the last page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub continuation: Option<String>,
}

/// One image of a thumbnail list. A width or height the service did not give is 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thumbnail {
    pub url: String,
    pub width: u32,
    pub height: u32,
}

/// The channel a video belongs to. `id` is `None` when the page showed a name without a usable channel link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelRef {
    pub id: Option<String>,
    pub name: String,
}

/// A video row of a playlist, history or similar listing.
///
/// `id` always passes [`crate::ids::is_video_id`]. Live streams and Shorts are only flagged, never dropped.
/// Playlist entries are not unique per video: the same `id` can appear twice, told apart by `set_video_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub channel: Option<ChannelRef>,
    /// `None` for live streams and when the row showed no length.
    pub duration_ms: Option<u64>,
    pub thumbnails: Vec<Thumbnail>,
    pub is_live: bool,
    pub is_short: bool,
    /// The service's id of this entry inside a playlist (needed to remove exactly this entry). Only playlist rows carry it.
    pub set_video_id: Option<String>,
}

/// A playlist row of the saved-playlists listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistSummary {
    /// Playlist id without the `VL` browse prefix; passes [`crate::ids::is_playlist_id`].
    pub id: String,
    pub title: String,
    pub video_count: Option<u64>,
    pub thumbnails: Vec<Thumbnail>,
}
