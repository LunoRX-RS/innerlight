//! Audio stream selection.

use super::model::{Quality, Stream};

/// Picks the audio stream to play.
///
/// Eligible: `audio/*`, no DRM, `content_length > 0`. If any eligible stream is in a default track (or has no track
/// info), only those are considered, so a dubbed track never wins on bitrate. Then: the highest bitrate within
/// `quality.bitrate_limit()`, otherwise the lowest one above it. Ties prefer Opus (WebM) over AAC.
pub fn select_audio(streams: &[Stream], quality: Quality) -> Option<&Stream> {
    select_by(streams.iter(), quality)
}

pub(super) fn select_by<'a>(streams: impl Iterator<Item = &'a Stream> + Clone, quality: Quality) -> Option<&'a Stream> {
    let eligible = || streams.clone().filter(|s| s.is_audio() && !s.drm && s.content_length > 0);
    let is_default = |s: &Stream| s.track.as_ref().is_none_or(|t| t.is_default);
    let any_default = eligible().any(is_default);
    let candidates = || eligible().filter(|s| !any_default || is_default(s));
    candidates()
        .filter(|s| s.bitrate <= quality.bitrate_limit())
        .max_by_key(|s| (s.bitrate, s.is_opus()))
        .or_else(|| candidates().min_by_key(|s| (s.bitrate, !s.is_opus())))
}
