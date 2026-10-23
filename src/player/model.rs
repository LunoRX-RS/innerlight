//! Plain data types of the player module. Nothing here prints a URL: `Debug` of a stream source and of a
//! [`Resolved`] hides every URL and signature.

use std::fmt;
use std::time::Duration;

/// Audio quality wish. `Low` <= 64 kbit/s, `Normal` <= 128 kbit/s, `High` = the best there is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    Low,
    Normal,
    #[default]
    High,
}

impl Quality {
    /// Upper bitrate bound in bit/s that [`select_audio`](super::select_audio) tries to stay under.
    pub fn bitrate_limit(self) -> u32 {
        match self {
            Quality::Low => 64_000,
            Quality::Normal => 128_000,
            Quality::High => u32::MAX,
        }
    }
}

/// One audio language/track of a multi-language video (`audioTrack`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTrack {
    pub id: String,
    pub display_name: String,
    pub is_default: bool,
}

/// Where the bytes of a stream live.
#[derive(Clone, PartialEq, Eq)]
pub enum StreamSource {
    /// A ready `url`.
    Direct(String),
    /// `signatureCipher` / `cipher`: `url` plus a scrambled signature `s` that must be unscrambled and appended
    /// as the query parameter named `sp`.
    Ciphered { url: String, s: String, sp: String },
}

impl fmt::Debug for StreamSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            StreamSource::Direct(_) => "Direct(..)",
            StreamSource::Ciphered { .. } => "Ciphered(..)",
        })
    }
}

/// One entry of `streamingData.adaptiveFormats` or `streamingData.formats`.
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    pub itag: u32,
    /// Full `mimeType`, e.g. `audio/webm; codecs="opus"`.
    pub mime_type: String,
    /// The `codecs="..."` part of the mime type, if present.
    pub codecs: Option<String>,
    /// Peak bitrate in bit/s (`bitrate`; falls back to `averageBitrate`, then 0).
    pub bitrate: u32,
    pub average_bitrate: Option<u32>,
    /// `contentLength` in bytes, 0 when absent or unparsable.
    pub content_length: u64,
    pub audio_quality: Option<String>,
    pub sample_rate: Option<u32>,
    pub loudness_db: Option<f64>,
    pub track: Option<AudioTrack>,
    /// `drmFamilies` non-empty or `drmTrackType` present.
    pub drm: bool,
    pub source: StreamSource,
}

impl Stream {
    /// `audio/*` only. Muxed `formats` (video+audio) are `video/*` and so not audio.
    pub fn is_audio(&self) -> bool {
        self.mime_type.trim_start().to_ascii_lowercase().starts_with("audio/")
    }

    /// Opus in WebM (as opposed to AAC in MP4).
    pub fn is_opus(&self) -> bool {
        match &self.codecs {
            Some(c) => c.to_ascii_lowercase().contains("opus"),
            None => self.mime_type.to_ascii_lowercase().contains("webm"),
        }
    }
}

/// The parsed `streamingData` object.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StreamingData {
    /// `expiresInSeconds`, relative to the moment the service answered.
    pub expires_in: Option<Duration>,
    pub streams: Vec<Stream>,
}

/// Why a video is `Unplayable`. Derived from fixed keywords in the reason text; the text itself is never kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnplayableReason {
    Region,
    Private,
    MembersOnly,
    Other,
}

/// The service's verdict (`playabilityStatus`). A stream is only ever returned for `Ok`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Playability {
    Ok,
    /// Sign-in is needed. `bot_check` is true for the "confirm you're not a bot" challenge, false for a plain sign-in.
    LoginRequired {
        bot_check: bool,
    },
    AgeRestricted,
    Unplayable {
        reason: UnplayableReason,
    },
    /// A live stream or a premiere/stream that has not started (no downloadable audio stream).
    Live,
    /// Removed, deleted, never existed.
    Unavailable,
    /// Any other status.
    Error,
}

/// A playable audio stream. `Debug` hides the URL.
#[derive(Clone, PartialEq)]
pub struct Resolved {
    /// Validated `https://*.googlevideo.com/...` URL. Fetch it with HTTP Range requests, see
    /// [`ranged_reader_plan`](super::ranged_reader_plan). Never log it.
    pub url: String,
    pub itag: u32,
    pub mime_type: String,
    pub codecs: Option<String>,
    pub bitrate: u32,
    /// Size in bytes (> 0).
    pub size: u64,
    pub sample_rate: Option<u32>,
    pub loudness_db: Option<f64>,
    pub track: Option<AudioTrack>,
    /// URL lifetime from the moment of the answer.
    pub expires_in: Option<Duration>,
    /// The URL carries a throttling parameter (`n`) and no [`Decipher`](super::Decipher) fixed it: the server may
    /// serve it slowly. Honest flag, not an error.
    pub needs_throttle_fix: bool,
}

impl fmt::Debug for Resolved {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Resolved")
            .field("itag", &self.itag)
            .field("mime_type", &self.mime_type)
            .field("bitrate", &self.bitrate)
            .field("size", &self.size)
            .field("expires_in", &self.expires_in)
            .field("needs_throttle_fix", &self.needs_throttle_fix)
            .finish_non_exhaustive()
    }
}
