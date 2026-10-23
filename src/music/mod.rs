//! YouTube Music (`music.youtube.com`, client `WEB_REMIX`): search, tracks, albums, artists, playlists, explore,
//! lyrics and the signed-in library.
//!
//! Network access goes through [`Client::post`] / [`Client::post_authed`] only. The pure parsers live in [`parse`] and
//! work on any `serde_json::Value`, which is how the offline tests drive them. See `docs/MUSIC.md`.
//!
//! Podcast episodes and livestreams are skipped, never returned as tracks.

pub mod ids;
mod item;
pub mod model;
pub mod parse;

pub use model::*;

use crate::{Client, ClientKind, Error, ids as core_ids};
use serde_json::{Value, json};

/// `params` of a song-only search (the site's own "Songs" filter chip).
const SONGS_FILTER: &str = "EgWKAQIIAWoMEA4QChADEAQQCRAF";
const MAX_QUERY_LEN: usize = 512;

/// A short-lived handle: `Music::new(&client)`.
#[derive(Clone, Copy)]
pub struct Music<'a> {
    client: &'a Client,
}

fn check_text(q: &str) -> Result<&str, Error> {
    let q = q.trim();
    if q.is_empty() || q.len() > MAX_QUERY_LEN || q.chars().any(char::is_control) {
        return Err(Error::InvalidRequest);
    }
    Ok(q)
}

fn check_token(t: &str) -> Result<&str, Error> {
    ids::is_continuation_token(t).then_some(t).ok_or(Error::InvalidId)
}

fn check(ok: bool, id: &str) -> Result<&str, Error> {
    ok.then_some(id).ok_or(Error::InvalidId)
}

impl<'a> Music<'a> {
    pub fn new(client: &'a Client) -> Self {
        Music { client }
    }

    async fn browse(&self, body: Value) -> Result<Value, Error> {
        self.client.post(ClientKind::WebMusic, "browse", body).await
    }

    async fn browse_authed(&self, body: Value) -> Result<Value, Error> {
        self.client.post_authed(ClientKind::WebMusic, "browse", body).await
    }

    // ------------------------------------------------------------ 1. search

    /// Song search, first page. `Error::InvalidRequest` for an empty, control-character or over-long query.
    pub async fn search_tracks(&self, query: &str) -> Result<Page<Track>, Error> {
        let q = check_text(query)?;
        let v = self.client.post(ClientKind::WebMusic, "search", json!({"query": q, "params": SONGS_FILTER})).await?;
        parse::search_tracks(&v)
    }

    /// Next page of a song search, from [`Page::continuation`].
    pub async fn search_tracks_more(&self, continuation: &str) -> Result<Page<Track>, Error> {
        let t = check_token(continuation)?;
        parse::track_page(&self.client.post(ClientKind::WebMusic, "search", json!({"continuation": t})).await?)
    }

    /// Autocomplete strings for a partial query.
    pub async fn search_suggestions(&self, input: &str) -> Result<Vec<String>, Error> {
        let q = check_text(input)?;
        let v = self.client.post(ClientKind::WebMusic, "music/get_search_suggestions", json!({"input": q})).await?;
        parse::search_suggestions(&v)
    }

    // ------------------------------------------------------------ 2. track details, radio

    /// The radio queue for a video (`playlistId = RDAMVM<id>`) plus the lyrics and related browse ids.
    pub async fn radio(&self, video_id: &str) -> Result<Radio, Error> {
        let id = check(core_ids::is_video_id(video_id), video_id)?;
        let body = json!({
            "videoId": id,
            "playlistId": format!("RDAMVM{id}"),
            "isAudioOnly": true,
            "enablePersistentPlaylistPanel": true,
            "tunerSettingValue": "AUTOMIX_SETTING_NORMAL",
        });
        parse::radio(&self.client.post(ClientKind::WebMusic, "next", body).await?)
    }

    // ------------------------------------------------------------ 3. album, artist, playlist

    /// `browse_id` is an `MPREb_…` album browse id.
    pub async fn album(&self, browse_id: &str) -> Result<Album, Error> {
        let id = check(ids::is_album_browse_id(browse_id), browse_id)?;
        parse::album(&self.browse(json!({"browseId": id})).await?, id)
    }

    /// `channel_id` is a `UC…` id.
    pub async fn artist(&self, channel_id: &str) -> Result<Artist, Error> {
        let id = check(core_ids::is_channel_id(channel_id), channel_id)?;
        parse::artist(&self.browse(json!({"browseId": id})).await?, id)
    }

    /// First page of a playlist. A leading `VL` on `playlist_id` is accepted and stripped.
    pub async fn playlist(&self, playlist_id: &str) -> Result<Playlist, Error> {
        let id = playlist_id.strip_prefix("VL").unwrap_or(playlist_id);
        let id = check(ids::is_music_playlist_id(id), id)?;
        parse::playlist(&self.browse(json!({"browseId": format!("VL{id}")})).await?, id)
    }

    /// Next page of a playlist, from [`Playlist::tracks`]`.continuation`.
    pub async fn playlist_more(&self, continuation: &str) -> Result<Page<Track>, Error> {
        parse::track_page(&self.browse(json!({"continuation": check_token(continuation)?})).await?)
    }

    // ------------------------------------------------------------ 4. explore

    /// Charts for a country (`"ZZ"` is global). `None` asks for the service default.
    pub async fn charts(&self, country: Option<&str>) -> Result<Vec<Section>, Error> {
        let mut body = json!({"browseId": "FEmusic_charts"});
        if let Some(c) = country {
            body["formData"] = json!({"selectedValues": [check(ids::is_chart_country(c), c)?]});
        }
        parse::charts(&self.browse(body).await?)
    }

    /// The moods and genres overview.
    pub async fn genres(&self) -> Result<Vec<GenreGroup>, Error> {
        parse::genres(&self.browse(json!({"browseId": "FEmusic_moods_and_genres"})).await?)
    }

    /// Playlist shelves of one mood or genre; `params` is [`Genre::params`].
    pub async fn genre_playlists(&self, params: &str) -> Result<Vec<Section>, Error> {
        let p = check(ids::is_genre_params(params), params)?;
        parse::genre_playlists(
            &self.browse(json!({"browseId": "FEmusic_moods_and_genres_category", "params": p})).await?,
        )
    }

    pub async fn new_albums(&self) -> Result<Vec<AlbumSummary>, Error> {
        parse::new_albums(&self.browse(json!({"browseId": "FEmusic_new_releases_albums"})).await?)
    }

    pub async fn new_videos(&self) -> Result<Vec<Track>, Error> {
        parse::new_videos(&self.browse(json!({"browseId": "FEmusic_new_releases_videos"})).await?)
    }

    // ------------------------------------------------------------ 5. lyrics

    /// `browse_id` is [`Radio::lyrics_browse_id`]. Plain text, or timed lines when the response carries them.
    pub async fn lyrics(&self, browse_id: &str) -> Result<Lyrics, Error> {
        let id = check(ids::is_lyrics_browse_id(browse_id), browse_id)?;
        parse::lyrics(&self.browse(json!({"browseId": id})).await?)
    }

    // ------------------------------------------------------------ 6. signed-in library (no like/unlike here)

    /// Liked songs. `Error::Unauthenticated` (nothing sent) without a signable cookie.
    pub async fn saved_tracks(&self) -> Result<Page<Track>, Error> {
        parse::library_tracks(&self.browse_authed(json!({"browseId": "FEmusic_liked_videos"})).await?)
    }

    pub async fn saved_tracks_more(&self, continuation: &str) -> Result<Page<Track>, Error> {
        parse::library_tracks(&self.browse_authed(json!({"continuation": check_token(continuation)?})).await?)
    }

    /// `saved_tracks` plus what the page held before the non-track rows were dropped.
    pub async fn saved_tracks_tally(&self) -> Result<(Page<Track>, RowTally, PageProbe), Error> {
        parse::library_tracks_tally(&self.browse_authed(json!({"browseId": "FEmusic_liked_videos"})).await?)
    }

    /// `saved_tracks_more` plus the row tally of that page.
    pub async fn saved_tracks_more_tally(
        &self,
        continuation: &str,
    ) -> Result<(Page<Track>, RowTally, PageProbe), Error> {
        parse::library_tracks_tally(&self.browse_authed(json!({"continuation": check_token(continuation)?})).await?)
    }

    /// Saved playlists.
    pub async fn saved_playlists(&self) -> Result<Page<PlaylistSummary>, Error> {
        parse::library_playlists(&self.browse_authed(json!({"browseId": "FEmusic_liked_playlists"})).await?)
    }

    pub async fn saved_playlists_more(&self, continuation: &str) -> Result<Page<PlaylistSummary>, Error> {
        parse::library_playlists(&self.browse_authed(json!({"continuation": check_token(continuation)?})).await?)
    }

    /// Listening history, newest first, flattened across the "Today/Yesterday/…" groups.
    pub async fn history(&self) -> Result<Page<Track>, Error> {
        parse::library_tracks(&self.browse_authed(json!({"browseId": "FEmusic_history"})).await?)
    }

    pub async fn history_more(&self, continuation: &str) -> Result<Page<Track>, Error> {
        parse::library_tracks(&self.browse_authed(json!({"continuation": check_token(continuation)?})).await?)
    }
}
