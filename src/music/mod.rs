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

    /// A search for one kind of item, mixed page in document order: rows and cards of the filtered kind plus a
    /// "top result". The kind only selects the `params` chip; items of another kind are dropped.
    pub async fn search_items(&self, query: &str, kind: SearchKind) -> Result<Page<Item>, Error> {
        let q = check_text(query)?;
        self.search_filtered(json!({"query": q, "params": kind.params()}), kind).await
    }

    /// Next page of a [`search_items`](Self::search_items) page. Nothing is filtered here: the handle carries the
    /// request, and a generic caller should see whatever comes back.
    pub async fn search_items_more(&self, continuation: &str) -> Result<Page<Item>, Error> {
        let t = check_token(continuation)?;
        let v = self.client.post(ClientKind::WebMusic, "search", json!({"continuation": t})).await?;
        parse::search_items(&v)
    }

    /// Album search, first page.
    pub async fn search_albums(&self, query: &str) -> Result<Page<AlbumSummary>, Error> {
        self.search_as_albums(json!({"query": check_text(query)?, "params": SearchKind::Albums.params()})).await
    }

    /// Next page of a [`search_albums`](Self::search_albums) page.
    pub async fn search_albums_more(&self, continuation: &str) -> Result<Page<AlbumSummary>, Error> {
        self.search_as_albums(json!({"continuation": check_token(continuation)?})).await
    }

    /// Artist (channel) search, first page.
    pub async fn search_artists(&self, query: &str) -> Result<Page<ArtistSummary>, Error> {
        self.search_as_artists(json!({"query": check_text(query)?, "params": SearchKind::Artists.params()})).await
    }

    /// Next page of a [`search_artists`](Self::search_artists) page.
    pub async fn search_artists_more(&self, continuation: &str) -> Result<Page<ArtistSummary>, Error> {
        self.search_as_artists(json!({"continuation": check_token(continuation)?})).await
    }

    /// Playlist search, first page.
    pub async fn search_playlists(&self, query: &str) -> Result<Page<PlaylistSummary>, Error> {
        self.search_as_playlists(json!({"query": check_text(query)?, "params": SearchKind::Playlists.params()})).await
    }

    /// Next page of a [`search_playlists`](Self::search_playlists) page.
    pub async fn search_playlists_more(&self, continuation: &str) -> Result<Page<PlaylistSummary>, Error> {
        self.search_as_playlists(json!({"continuation": check_token(continuation)?})).await
    }

    /// Next page of an [`album`](Self::album)'s track list, from [`Album::continuation`]. The answer is a plain row
    /// page like a playlist continuation.
    pub async fn album_more(&self, continuation: &str) -> Result<Page<Track>, Error> {
        self.playlist_more(continuation).await
    }

    async fn search_filtered(&self, body: Value, kind: SearchKind) -> Result<Page<Item>, Error> {
        let mut page = self.search_raw(body).await?;
        page.items.retain(|item| kind.holds(item));
        Ok(page)
    }

    async fn search_raw(&self, body: Value) -> Result<Page<Item>, Error> {
        let v = self.client.post(ClientKind::WebMusic, "search", body).await?;
        parse::search_items(&v)
    }

    /// One page narrowed to a single item type. A card of another kind is dropped, not converted.
    async fn search_only<T>(
        &self,
        body: Value,
        kind: SearchKind,
        take: fn(Item) -> Option<T>,
    ) -> Result<Page<T>, Error> {
        let page = self.search_filtered(body, kind).await?;
        Ok(Page { items: page.items.into_iter().filter_map(take).collect(), continuation: page.continuation })
    }

    async fn search_as_albums(&self, body: Value) -> Result<Page<AlbumSummary>, Error> {
        self.search_only(body, SearchKind::Albums, |i| match i {
            Item::Album(a) => Some(a),
            _ => None,
        })
        .await
    }

    async fn search_as_artists(&self, body: Value) -> Result<Page<ArtistSummary>, Error> {
        self.search_only(body, SearchKind::Artists, |i| match i {
            Item::Artist(a) => Some(a),
            _ => None,
        })
        .await
    }

    async fn search_as_playlists(&self, body: Value) -> Result<Page<PlaylistSummary>, Error> {
        self.search_only(body, SearchKind::Playlists, |i| match i {
            Item::Playlist(p) => Some(p),
            _ => None,
        })
        .await
    }

    /// Autocomplete strings for a partial query.
    pub async fn search_suggestions(&self, input: &str) -> Result<Vec<String>, Error> {
        let q = check_text(input)?;
        let v = self.client.post(ClientKind::WebMusic, "music/get_search_suggestions", json!({"input": q})).await?;
        parse::search_suggestions(&v)
    }

    /// [`search_suggestions`](Self::search_suggestions) sent signed in, which is the only way history entries
    /// (`historySuggestionRenderer`) can appear. `Error::Unauthenticated` without a `SAPISID` cookie, and then
    /// nothing is sent.
    pub async fn search_suggestions_authed(&self, input: &str) -> Result<Vec<String>, Error> {
        let q = check_text(input)?;
        let v =
            self.client.post_authed(ClientKind::WebMusic, "music/get_search_suggestions", json!({"input": q})).await?;
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

    /// Next queue entries of a radio, from [`Radio::continuation`]. The tab ids are not repeated on these pages, so
    /// this returns the rows only.
    pub async fn radio_more(&self, continuation: &str) -> Result<Page<Track>, Error> {
        let t = check_token(continuation)?;
        parse::radio_page(&self.client.post(ClientKind::WebMusic, "next", json!({"continuation": t})).await?)
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

    // ------------------------------------------------------------ 4. home feed and explore

    /// The home feed sent anonymously: generic shelves rather than a personalized mix. [`Page::items`] are the shelves
    /// in document order; hand [`Page::continuation`] to [`home_more`](Self::home_more) for the next batch.
    pub async fn home(&self) -> Result<Page<Section>, Error> {
        parse::home_page(&self.browse(json!({"browseId": "FEmusic_home"})).await?)
    }

    /// The home feed signed in, which is the one the site personalizes. `Error::Unauthenticated` without a cookie.
    pub async fn home_authed(&self) -> Result<Page<Section>, Error> {
        parse::home_page(&self.browse_authed(json!({"browseId": "FEmusic_home"})).await?)
    }

    /// Next shelves of [`home`](Self::home).
    pub async fn home_more(&self, continuation: &str) -> Result<Page<Section>, Error> {
        let t = check_token(continuation)?;
        parse::home_page(&self.browse(json!({"continuation": t})).await?)
    }

    /// Next shelves of [`home_authed`](Self::home_authed).
    pub async fn home_more_authed(&self, continuation: &str) -> Result<Page<Section>, Error> {
        let t = check_token(continuation)?;
        parse::home_page(&self.browse_authed(json!({"continuation": t})).await?)
    }

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
