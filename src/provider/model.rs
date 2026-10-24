//! Data a [`Provider`](crate::provider::Provider) returns. Deliberately separate from [`crate::music::model`]:
//! the music types validate YouTube id shapes (`MPREb_…`, `UC…`, 11-character video ids) and a non-YouTube source
//! must not pretend its ids have that shape, nor have those checks weakened.
//!
//! Nothing here is decoded or played. A [`Resolved`] is a URL the caller hands to its own player.

use crate::music::model::Thumbnail;
use serde::{Deserialize, Serialize};

/// One page of results plus the opaque handle for the next one. `None` means there is no next page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub continuation: Option<String>,
}

/// A single recording. `id` is meaningful only to the provider that produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    /// Credit order as the source gave it. Plain names: a provider has no YouTube channel id to offer.
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    pub thumbnails: Vec<Thumbnail>,
    /// A license identifier as the source stated it, for example a CC tag. Never inferred.
    pub license: Option<String>,
    /// The credit line a caller should show alongside the track.
    pub attribution: Option<String>,
    /// [`Provider::name`](crate::provider::Provider::name) of the source.
    pub source: String,
}

/// An album as a search hit: metadata, no track list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumSummary {
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub year: Option<u32>,
    pub thumbnails: Vec<Thumbnail>,
    pub license: Option<String>,
    pub attribution: Option<String>,
    pub source: String,
}

/// One album with its tracks. `tracks` is empty until the source is asked for the list, which is exactly what a
/// search hit is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Album {
    pub summary: AlbumSummary,
    pub description: Option<String>,
    pub tracks: Page<Track>,
}

/// What a list can hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Item {
    Track(Track),
    Album(AlbumSummary),
}

/// Where a track's audio lives. The URL is https and was checked before it was put here; nothing is fetched or
/// decoded by this crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolved {
    pub url: String,
    pub duration_ms: Option<u64>,
    pub mime: Option<String>,
    pub license: Option<String>,
    pub attribution: Option<String>,
}

impl Item {
    /// The provider this came from.
    pub fn source(&self) -> &str {
        match self {
            Item::Track(t) => &t.source,
            Item::Album(a) => &a.source,
        }
    }

    /// Title text for display and for the merge key of [`Catalog`](super::Catalog).
    pub fn title(&self) -> &str {
        match self {
            Item::Track(t) => &t.title,
            Item::Album(a) => &a.title,
        }
    }

    /// The first credit, if any.
    pub fn primary_artist(&self) -> Option<&str> {
        match self {
            Item::Track(t) => t.artists.first().map(String::as_str),
            Item::Album(a) => a.artists.first().map(String::as_str),
        }
    }
}
