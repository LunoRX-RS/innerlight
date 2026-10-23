//! Signed-in YouTube listings (client WEB): liked videos, watch later, history, saved playlists, public playlists and video search.
//!
//! Every call is a `browse` request. The first page is requested by browse id, later pages by the opaque `continuation`
//! token of the previous [`Page`]. Live streams and Shorts are flagged on [`Video`], never filtered here.

pub mod model;
mod parse;

use crate::{Client, ClientKind, Error, ids};
use serde_json::{Value, json};

pub use model::{ChannelRef, Page, PlaylistSummary, Thumbnail, Video};
pub use parse::{parse_playlist_page, parse_video_page};

/// `params` of a video-only search (the site's own "Videos" filter chip).
const VIDEOS_FILTER: &str = "EgIQAQ%3D%3D";
const MAX_QUERY_LEN: usize = 512;

/// Tokens are base64url with percent escapes; anything with controls or an absurd size is a caller bug.
fn check_token(t: &str) -> Result<&str, Error> {
    if t.is_empty() || t.len() > 8192 || t.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(Error::InvalidRequest);
    }
    Ok(t)
}

/// Handle for the YouTube listings of one [`Client`] (borrowed; cheap to create).
#[derive(Clone, Copy)]
pub struct Youtube<'a> {
    client: &'a Client,
}

impl<'a> Youtube<'a> {
    pub fn new(client: &'a Client) -> Self {
        Self { client }
    }

    /// Liked videos (`VLLL`). Needs a signed-in client, else `Err(Unauthenticated)` and nothing is sent.
    pub async fn liked_videos(&self, continuation: Option<&str>) -> Result<Page<Video>, Error> {
        parse_video_page(&self.browse(true, "VLLL", continuation).await?)
    }

    /// Watch later (`VLWL`). Signed in.
    pub async fn watch_later(&self, continuation: Option<&str>) -> Result<Page<Video>, Error> {
        parse_video_page(&self.browse(true, "VLWL", continuation).await?)
    }

    /// Watch history (`FEhistory`). Signed in. Rows repeat when a video was watched more than once.
    pub async fn history(&self, continuation: Option<&str>) -> Result<Page<Video>, Error> {
        parse_video_page(&self.browse(true, "FEhistory", continuation).await?)
    }

    /// Saved playlists (`FEplaylist_aggregation`). Signed in.
    pub async fn saved_playlists(&self, continuation: Option<&str>) -> Result<Page<PlaylistSummary>, Error> {
        parse_playlist_page(&self.browse(true, "FEplaylist_aggregation", continuation).await?)
    }

    /// A public playlist (`VL` + id), fetched anonymously. `id` is `PL...` (or `VL...`, which is kept as is).
    /// Private playlists, `LL` and `WL` need the signed-in methods above; they are rejected here with `InvalidId`.
    pub async fn playlist(&self, id: &str, continuation: Option<&str>) -> Result<Page<Video>, Error> {
        if !ids::is_playlist_id(id) || matches!(id, "LL" | "WL" | "VL") {
            return Err(Error::InvalidId);
        }
        let browse_id = if id.starts_with("VL") { id.to_owned() } else { format!("VL{id}") };
        parse_video_page(&self.browse(false, &browse_id, continuation).await?)
    }

    /// Video search (the site's "Videos" filter), fetched anonymously. `query` is trimmed and must be 1..=512 characters
    /// without control characters (`InvalidRequest` otherwise, nothing sent). Live streams and Shorts are flagged on
    /// [`Video`], not filtered; premieres that have not started have `duration_ms: None` and `is_live: false`.
    pub async fn search_videos(&self, query: &str, continuation: Option<&str>) -> Result<Page<Video>, Error> {
        let body = match continuation {
            Some(t) => json!({"continuation": check_token(t)?}),
            None => {
                let q = query.trim();
                if q.is_empty() || q.len() > MAX_QUERY_LEN || q.chars().any(char::is_control) {
                    return Err(Error::InvalidRequest);
                }
                json!({"query": q, "params": VIDEOS_FILTER})
            }
        };
        parse_video_page(&self.client.post(ClientKind::Web, "search", body).await?)
    }

    async fn browse(&self, authed: bool, browse_id: &str, continuation: Option<&str>) -> Result<Value, Error> {
        let body = match continuation {
            Some(t) => json!({"continuation": check_token(t)?}),
            None => json!({"browseId": browse_id}),
        };
        if authed {
            self.client.post_authed(ClientKind::Web, "browse", body).await
        } else {
            self.client.post(ClientKind::Web, "browse", body).await
        }
    }
}
