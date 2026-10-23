//! Signed-in account actions: like, subscribe, playlist edits. Every call goes through `Client::post_authed`
//! (client WEB), so without a cookie session it fails with `Unauthenticated` and sends nothing.
//!
//! A call succeeds only when the answer is JSON with a `responseContext` and no top-level `error`.
//! A top-level `error` maps to `Error::Refused`, a body without `responseContext` to `Error::Parse`.

use crate::{Client, ClientKind, Error, ids};
use serde_json::{Value, json};

/// Privacy of a new playlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Privacy {
    Private,
    Unlisted,
    Public,
}

impl Privacy {
    fn wire(self) -> &'static str {
        match self {
            Privacy::Private => "PRIVATE",
            Privacy::Unlisted => "UNLISTED",
            Privacy::Public => "PUBLIC",
        }
    }
}

/// Handle for the account actions of one [`Client`] (borrowed; cheap to create).
#[derive(Clone, Copy)]
pub struct Account<'a> {
    client: &'a Client,
}

fn video(id: &str) -> Result<(), Error> {
    ids::is_video_id(id).then_some(()).ok_or(Error::InvalidId)
}

fn channel(id: &str) -> Result<(), Error> {
    ids::is_channel_id(id).then_some(()).ok_or(Error::InvalidId)
}

/// A playlist id as the edit endpoints want it: no `VL` browse prefix.
fn playlist(id: &str) -> Result<(), Error> {
    (ids::is_playlist_id(id) && !id.starts_with("VL")).then_some(()).ok_or(Error::InvalidId)
}

/// 2xx JSON with `responseContext` and no `error`.
fn check(v: &Value) -> Result<(), Error> {
    let obj = v.as_object().ok_or(Error::Parse)?;
    if obj.contains_key("error") {
        return Err(Error::Refused);
    }
    if !obj.contains_key("responseContext") {
        return Err(Error::Parse);
    }
    Ok(())
}

impl<'a> Account<'a> {
    pub fn new(client: &'a Client) -> Self {
        Self { client }
    }

    async fn call(&self, endpoint: &str, body: Value) -> Result<Value, Error> {
        let v = self.client.post_authed(ClientKind::Web, endpoint, body).await?;
        check(&v)?;
        Ok(v)
    }

    /// `like/like`
    pub async fn like_video(&self, video_id: &str) -> Result<(), Error> {
        video(video_id)?;
        self.call("like/like", json!({"target": {"videoId": video_id}})).await.map(drop)
    }

    /// `like/removelike`: removes a like or a dislike.
    pub async fn unlike_video(&self, video_id: &str) -> Result<(), Error> {
        video(video_id)?;
        self.call("like/removelike", json!({"target": {"videoId": video_id}})).await.map(drop)
    }

    /// `like/dislike`
    pub async fn dislike_video(&self, video_id: &str) -> Result<(), Error> {
        video(video_id)?;
        self.call("like/dislike", json!({"target": {"videoId": video_id}})).await.map(drop)
    }

    /// `subscription/subscribe`
    pub async fn subscribe(&self, channel_id: &str) -> Result<(), Error> {
        channel(channel_id)?;
        self.call("subscription/subscribe", json!({"channelIds": [channel_id]})).await.map(drop)
    }

    /// `subscription/unsubscribe`
    pub async fn unsubscribe(&self, channel_id: &str) -> Result<(), Error> {
        channel(channel_id)?;
        self.call("subscription/unsubscribe", json!({"channelIds": [channel_id]})).await.map(drop)
    }

    /// `browse/edit_playlist` with `ACTION_ADD_VIDEO`. `playlist_id` has no `VL` prefix (`WL` is accepted).
    pub async fn add_to_playlist(&self, playlist_id: &str, video_id: &str) -> Result<(), Error> {
        playlist(playlist_id)?;
        video(video_id)?;
        let body = json!({
            "playlistId": playlist_id,
            "actions": [{"action": "ACTION_ADD_VIDEO", "addedVideoId": video_id}],
        });
        self.call("browse/edit_playlist", body).await.map(drop)
    }

    /// `browse/edit_playlist` removing one entry. With `set_video_id` (from [`crate::youtube::Video::set_video_id`])
    /// exactly that entry goes (`ACTION_REMOVE_VIDEO`); without it every entry of the video does
    /// (`ACTION_REMOVE_VIDEO_BY_VIDEO_ID`).
    pub async fn remove_from_playlist(
        &self,
        playlist_id: &str,
        video_id: &str,
        set_video_id: Option<&str>,
    ) -> Result<(), Error> {
        playlist(playlist_id)?;
        video(video_id)?;
        let action = match set_video_id {
            Some(sid) => {
                let ok = !sid.is_empty()
                    && sid.len() <= 128
                    && sid.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
                if !ok {
                    return Err(Error::InvalidId);
                }
                json!({"action": "ACTION_REMOVE_VIDEO", "removedVideoId": video_id, "setVideoId": sid})
            }
            None => json!({"action": "ACTION_REMOVE_VIDEO_BY_VIDEO_ID", "removedVideoId": video_id}),
        };
        self.call("browse/edit_playlist", json!({"playlistId": playlist_id, "actions": [action]})).await.map(drop)
    }

    /// `playlist/create`. Returns the new playlist's id (a success without one is `Err(Parse)`).
    /// `title` is 1..=150 characters without control characters; `video_ids` may be empty.
    pub async fn create_playlist(&self, title: &str, privacy: Privacy, video_ids: &[&str]) -> Result<String, Error> {
        let n = title.chars().count();
        if n == 0 || n > 150 || title.chars().any(char::is_control) || title.trim().is_empty() {
            return Err(Error::InvalidRequest);
        }
        video_ids.iter().try_for_each(|v| video(v))?;
        let body = json!({"title": title, "privacyStatus": privacy.wire(), "videoIds": video_ids});
        let v = self.call("playlist/create", body).await?;
        let id = v.get("playlistId").and_then(Value::as_str).ok_or(Error::Parse)?;
        playlist(id).map_err(|_| Error::Parse)?;
        Ok(id.to_owned())
    }

    /// `playlist/delete`. `LL`, `WL` and ids with a `VL` prefix are rejected with `InvalidId`.
    pub async fn delete_playlist(&self, playlist_id: &str) -> Result<(), Error> {
        playlist(playlist_id)?;
        if matches!(playlist_id, "LL" | "WL") {
            return Err(Error::InvalidId);
        }
        self.call("playlist/delete", json!({"playlistId": playlist_id})).await.map(drop)
    }
}
