//! Stream resolution: `POST player`, check `playabilityStatus`, pick an audio stream, validate its URL.
//!
//! One standard request with one of the two existing client kinds (`Web`, `WebMusic`), signed with the user's own
//! session cookie only when `authed` is set. A login, bot-check or age challenge comes back as a typed
//! [`PlayerError`] and the call stops: no retry, no other client identity, no workaround. See `docs/PLAYER.md`.

mod error;
pub mod model;
pub mod parse;
mod select;
mod url;

pub use error::PlayerError;
pub use model::*;
pub use select::select_audio;
pub use url::{ByteRange, check_stream_url, ranged_reader_plan};

use crate::{Client, ClientKind, ids};
use serde_json::{Value, json};
use std::sync::Arc;

/// Turns a ciphered stream into a usable URL and removes throttling. **No implementation ships with this crate.**
/// An implementation needs the site's player script; see `docs/PLAYER.md`.
pub trait Decipher: Send + Sync {
    /// Unscrambles the `s` value of a `signatureCipher`. The result is appended as `&{sp}=<result>`.
    fn decipher(&self, s: &str) -> Result<String, PlayerError>;
    /// Rewrites the throttling parameter `n` of a (direct or deciphered) URL. Called for every URL that has an `n`.
    fn throttle_fix(&self, url: &str) -> Result<String, PlayerError>;
}

/// What to ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolveOptions {
    pub kind: ClientKind,
    /// Sign the request with the stored session cookie (`Client::post_authed`). `Unauthenticated` if there is none.
    pub authed: bool,
    pub quality: Quality,
    /// `signatureTimestamp` of the player script the caller knows. Omitted from the request when `None`.
    pub signature_timestamp: Option<u32>,
}

impl Default for ResolveOptions {
    fn default() -> Self {
        Self { kind: ClientKind::Web, authed: false, quality: Quality::High, signature_timestamp: None }
    }
}

/// Handle for stream resolution: `Player::new(&client)`.
#[derive(Clone)]
pub struct Player<'a> {
    client: &'a Client,
    decipher: Option<Arc<dyn Decipher>>,
}

impl std::fmt::Debug for Player<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Player(..)")
    }
}

/// The body of the `player` request (the client adds `context`).
pub fn request_body(video_id: &str, signature_timestamp: Option<u32>) -> Value {
    let mut cpc = json!({"html5Preference": "HTML5_PREF_WANTS"});
    if let Some(ts) = signature_timestamp {
        cpc["signatureTimestamp"] = json!(ts);
    }
    json!({
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true,
        "playbackContext": {"contentPlaybackContext": cpc},
    })
}

/// Percent-encodes everything but unreserved characters.
fn pct(s: &str) -> String {
    s.bytes().fold(String::with_capacity(s.len()), |mut out, b| {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
        out
    })
}

fn has_throttle_param(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| u.query_pairs().any(|(k, _)| k == "n"))
}

/// Direct URL, or deciphered URL, validated. The bool is `needs_throttle_fix`.
fn finish(stream: &Stream, decipher: Option<&dyn Decipher>) -> Result<(String, bool), PlayerError> {
    let mut url = match &stream.source {
        StreamSource::Direct(u) => check_stream_url(u)?,
        StreamSource::Ciphered { url, s, sp } => {
            let d = decipher.ok_or(PlayerError::NeedsDecipher)?;
            if sp.is_empty() || sp.len() > 32 || !sp.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Err(PlayerError::UnsafeUrl);
            }
            let base = check_stream_url(url)?;
            let sig = d.decipher(s)?;
            let sep = if base.contains('?') { '&' } else { '?' };
            format!("{base}{sep}{sp}={}", pct(&sig))
        }
    };
    let mut needs_fix = false;
    if has_throttle_param(&url) {
        match decipher {
            Some(d) => url = d.throttle_fix(&url)?,
            None => needs_fix = true,
        }
    }
    Ok((check_stream_url(&url)?, needs_fix))
}

impl<'a> Player<'a> {
    pub fn new(client: &'a Client) -> Self {
        Self { client, decipher: None }
    }

    /// Supplies the implementation that handles ciphered streams and throttling.
    pub fn with_decipher(mut self, decipher: Arc<dyn Decipher>) -> Self {
        self.decipher = Some(decipher);
        self
    }

    /// One `player` request, then [`Player::resolve_response`]. `InvalidId` (nothing sent) for a bad id.
    pub async fn resolve(&self, video_id: &str, opts: ResolveOptions) -> Result<Resolved, PlayerError> {
        if !ids::is_video_id(video_id) {
            return Err(PlayerError::InvalidId);
        }
        let body = request_body(video_id, opts.signature_timestamp);
        let v = if opts.authed {
            self.client.post_authed(opts.kind, "player", body).await?
        } else {
            self.client.post(opts.kind, "player", body).await?
        };
        self.resolve_response(&v, opts.quality)
    }

    /// The pure part: playability, stream selection, URL validation. A stream is returned only for `Playability::Ok`.
    pub fn resolve_response(&self, v: &Value, quality: Quality) -> Result<Resolved, PlayerError> {
        match parse::parse_playability(v)? {
            Playability::Ok => {}
            Playability::LoginRequired { bot_check } => return Err(PlayerError::LoginRequired { bot_check }),
            Playability::AgeRestricted => return Err(PlayerError::AgeRestricted),
            Playability::Unplayable { reason } => return Err(PlayerError::Unplayable { reason }),
            Playability::Live => return Err(PlayerError::Live),
            Playability::Unavailable => return Err(PlayerError::Unavailable),
            Playability::Error => return Err(PlayerError::ServiceError),
        }
        let data = parse::parse_streaming_data(v).ok_or(PlayerError::Parse)?;
        let all = select::select_by(data.streams.iter(), quality);
        // Without a Decipher a ciphered pick falls back to the best direct stream, if any.
        let pick = match (all, &self.decipher) {
            (Some(s), None) if matches!(s.source, StreamSource::Ciphered { .. }) => {
                let direct = data.streams.iter().filter(|s| matches!(s.source, StreamSource::Direct(_)));
                select::select_by(direct, quality).ok_or(PlayerError::NeedsDecipher)?
            }
            (Some(s), _) => s,
            (None, _) => return Err(PlayerError::NoAudio),
        };
        let (url, needs_throttle_fix) = finish(pick, self.decipher.as_deref())?;
        Ok(Resolved {
            url,
            itag: pick.itag,
            mime_type: pick.mime_type.clone(),
            codecs: pick.codecs.clone(),
            bitrate: pick.bitrate,
            size: pick.content_length,
            sample_rate: pick.sample_rate,
            loudness_db: pick.loudness_db,
            track: pick.track.clone(),
            expires_in: data.expires_in,
            needs_throttle_fix,
        })
    }
}
