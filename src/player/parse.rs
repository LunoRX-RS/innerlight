//! Pure parsers for the `player` answer. Never panic; odd shapes give fewer streams or a typed error.

use super::PlayerError;
use super::model::{AudioTrack, Playability, Stream, StreamSource, StreamingData, UnplayableReason};
use crate::jsonx;
use serde_json::Value;
use std::time::Duration;

const MAX_STREAMS: usize = 512;
const MAX_CIPHER_LEN: usize = 32 * 1024;

fn num(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn num32(v: Option<&Value>) -> Option<u32> {
    v.and_then(num).and_then(|n| u32::try_from(n).ok())
}

/// All human-readable text of a `playabilityStatus`, lower-cased, curly quotes flattened.
fn status_text(ps: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.extend(ps.get("reason").and_then(jsonx::text));
    if let Some(r) = jsonx::get(ps, "errorScreen.playerErrorMessageRenderer") {
        parts.extend(r.get("reason").and_then(jsonx::text));
        parts.extend(r.get("subreason").and_then(jsonx::text));
    }
    if let Some(m) = ps.get("messages").and_then(Value::as_array) {
        parts.extend(m.iter().filter_map(jsonx::text));
    }
    parts.join(" ").to_lowercase().replace(['\u{2019}', '\u{2018}'], "'")
}

fn has(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| text.contains(n))
}

const UNAVAILABLE: &[&str] =
    &["unavailable", "has been removed", "no longer available", "does not exist", "terminated"];

fn unplayable_reason(text: &str) -> UnplayableReason {
    if has(text, &["country", "your region", "your location"]) {
        UnplayableReason::Region
    } else if has(text, &["is private", "private video"]) {
        UnplayableReason::Private
    } else if has(text, &["members-only", "members only", "join this channel"]) {
        UnplayableReason::MembersOnly
    } else {
        UnplayableReason::Other
    }
}

/// Reads `playabilityStatus` (and `videoDetails.isLive` for an OK status). A missing or non-object status is
/// `Err(Parse)`; an unknown status string is `Playability::Error`.
pub fn parse_playability(v: &Value) -> Result<Playability, PlayerError> {
    let ps = v.get("playabilityStatus").filter(|p| p.is_object()).ok_or(PlayerError::Parse)?;
    let status = ps.get("status").and_then(Value::as_str).ok_or(PlayerError::Parse)?;
    if status == "OK" {
        let live = jsonx::get(v, "videoDetails.isLive").and_then(Value::as_bool).unwrap_or(false);
        return Ok(if live { Playability::Live } else { Playability::Ok });
    }
    let text = status_text(ps);
    // The bot challenge is recognised by its text, whatever status carries it.
    if has(&text, &["not a bot"]) {
        return Ok(Playability::LoginRequired { bot_check: true });
    }
    if matches!(status, "AGE_VERIFICATION_REQUIRED" | "AGE_CHECK_REQUIRED")
        || has(&text, &["confirm your age", "age-restricted", "inappropriate for some users"])
    {
        return Ok(Playability::AgeRestricted);
    }
    Ok(match status {
        "LOGIN_REQUIRED" => Playability::LoginRequired { bot_check: false },
        "LIVE_STREAM_OFFLINE" => Playability::Live,
        "UNPLAYABLE" | "CONTENT_CHECK_REQUIRED" | "ERROR" if has(&text, UNAVAILABLE) && !has(&text, &["country"]) => {
            Playability::Unavailable
        }
        "UNPLAYABLE" | "CONTENT_CHECK_REQUIRED" => Playability::Unplayable { reason: unplayable_reason(&text) },
        _ => Playability::Error,
    })
}

/// Splits a `signatureCipher` / `cipher` query string into `(url, s, sp)`; `sp` defaults to `signature`.
fn parse_cipher(q: &str) -> Option<StreamSource> {
    if q.len() > MAX_CIPHER_LEN || q.chars().any(char::is_control) {
        return None;
    }
    let parsed = reqwest::Url::parse(&format!("https://cipher.invalid/?{q}")).ok()?;
    let (mut url, mut s, mut sp) = (None, None, None);
    for (k, v) in parsed.query_pairs() {
        match k.as_ref() {
            "url" => url = Some(v.into_owned()),
            "s" => s = Some(v.into_owned()),
            "sp" => sp = Some(v.into_owned()),
            _ => {}
        }
    }
    let (url, s) = (url.filter(|u| !u.is_empty())?, s.filter(|s| !s.is_empty())?);
    Some(StreamSource::Ciphered { url, s, sp: sp.filter(|p| !p.is_empty()).unwrap_or_else(|| "signature".into()) })
}

fn codecs_of(mime: &str) -> Option<String> {
    let rest = &mime[mime.find("codecs=\"")? + 8..];
    Some(rest[..rest.find('"')?].to_owned())
}

fn parse_track(f: &Value) -> Option<AudioTrack> {
    let t = f.get("audioTrack").filter(|t| t.is_object())?;
    let s = |k: &str| t.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    Some(AudioTrack {
        id: s("id"),
        display_name: s("displayName"),
        is_default: t.get("audioIsDefault").and_then(Value::as_bool).unwrap_or(false),
    })
}

fn parse_stream(f: &Value) -> Option<Stream> {
    let itag = num32(f.get("itag"))?;
    let mime_type = f.get("mimeType").and_then(Value::as_str)?.to_owned();
    let source = match f.get("url").and_then(Value::as_str) {
        Some(u) => StreamSource::Direct(u.to_owned()),
        None => parse_cipher(f.get("signatureCipher").or_else(|| f.get("cipher"))?.as_str()?)?,
    };
    let average_bitrate = num32(f.get("averageBitrate"));
    let drm = f.get("drmFamilies").and_then(Value::as_array).is_some_and(|a| !a.is_empty())
        || f.get("drmTrackType").is_some_and(|v| !v.is_null());
    Some(Stream {
        itag,
        codecs: codecs_of(&mime_type),
        mime_type,
        bitrate: num32(f.get("bitrate")).or(average_bitrate).unwrap_or(0),
        average_bitrate,
        content_length: f.get("contentLength").and_then(num).unwrap_or(0),
        audio_quality: f.get("audioQuality").and_then(Value::as_str).map(str::to_owned),
        sample_rate: num32(f.get("audioSampleRate")),
        loudness_db: f
            .get("loudnessDb")
            .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
            .filter(|d| d.is_finite()),
        track: parse_track(f),
        drm,
        source,
    })
}

/// Reads `streamingData`: `adaptiveFormats` then `formats`. Entries without itag, mime type or a usable
/// `url`/cipher are skipped. `None` when there is no `streamingData` object.
pub fn parse_streaming_data(v: &Value) -> Option<StreamingData> {
    let sd = v.get("streamingData").filter(|s| s.is_object())?;
    let streams = ["adaptiveFormats", "formats"]
        .into_iter()
        .filter_map(|k| sd.get(k).and_then(Value::as_array))
        .flatten()
        .filter_map(parse_stream)
        .take(MAX_STREAMS)
        .collect();
    Some(StreamingData { expires_in: sd.get("expiresInSeconds").and_then(num).map(Duration::from_secs), streams })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn st(status: &str, reason: &str) -> Value {
        json!({"playabilityStatus": {"status": status, "reason": reason}})
    }

    #[test]
    fn playability_table() {
        use Playability as P;
        let cases = [
            ("OK", "", P::Ok),
            ("LOGIN_REQUIRED", "Sign in to confirm you\u{2019}re not a bot", P::LoginRequired { bot_check: true }),
            ("LOGIN_REQUIRED", "Sign in to confirm you're not a bot", P::LoginRequired { bot_check: true }),
            ("LOGIN_REQUIRED", "Sign in to view this video", P::LoginRequired { bot_check: false }),
            (
                "LOGIN_REQUIRED",
                "This video may be inappropriate for some users. Sign in to confirm your age",
                P::AgeRestricted,
            ),
            ("AGE_VERIFICATION_REQUIRED", "", P::AgeRestricted),
            ("LIVE_STREAM_OFFLINE", "This live event will begin in 2 hours.", P::Live),
            (
                "UNPLAYABLE",
                "The uploader has not made this video available in your country",
                P::Unplayable { reason: UnplayableReason::Region },
            ),
            ("UNPLAYABLE", "This video is private", P::Unplayable { reason: UnplayableReason::Private }),
            ("UNPLAYABLE", "Join this channel to get access", P::Unplayable { reason: UnplayableReason::MembersOnly }),
            ("UNPLAYABLE", "Something else", P::Unplayable { reason: UnplayableReason::Other }),
            ("UNPLAYABLE", "This video is unavailable", P::Unavailable),
            ("ERROR", "Video unavailable", P::Unavailable),
            ("ERROR", "Something went wrong", P::Error),
            ("SOMETHING_NEW", "", P::Error),
        ];
        for (s, r, want) in cases {
            assert_eq!(parse_playability(&st(s, r)), Ok(want), "{s} {r}");
        }
    }

    #[test]
    fn bot_text_wins_over_status() {
        assert_eq!(
            parse_playability(&st("UNPLAYABLE", "Sign in to confirm you're not a bot")),
            Ok(Playability::LoginRequired { bot_check: true })
        );
    }

    #[test]
    fn live_flag_on_ok_status() {
        let v = json!({"playabilityStatus": {"status": "OK"}, "videoDetails": {"isLive": true}});
        assert_eq!(parse_playability(&v), Ok(Playability::Live));
    }

    #[test]
    fn playability_malformed() {
        for v in [
            json!(null),
            json!([]),
            json!({}),
            json!({"playabilityStatus": 3}),
            json!({"playabilityStatus": {}}),
            json!({"playabilityStatus": {"status": 1}}),
        ] {
            assert_eq!(parse_playability(&v), Err(PlayerError::Parse), "{v}");
        }
    }

    #[test]
    fn cipher_split() {
        let src =
            parse_cipher("s=AB%3DC%2B&sp=sig&url=https%3A%2F%2Fa.googlevideo.com%2Fv%3Fitag%3D1%26n%3Dx").unwrap();
        assert_eq!(
            src,
            StreamSource::Ciphered {
                url: "https://a.googlevideo.com/v?itag=1&n=x".into(),
                s: "AB=C+".into(),
                sp: "sig".into()
            }
        );
        let StreamSource::Ciphered { sp, .. } = parse_cipher("url=https%3A%2F%2Fa&s=x").unwrap() else { panic!() };
        assert_eq!(sp, "signature");
        for bad in ["", "s=x", "url=u", "url=u&s=", "url=a\nb&s=x"] {
            assert!(parse_cipher(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn stream_fields() {
        let f = json!({"itag": 251, "mimeType": "audio/webm; codecs=\"opus\"", "url": "https://x", "bitrate": 138000,
            "averageBitrate": 130000, "contentLength": "1234", "audioQuality": "AUDIO_QUALITY_MEDIUM",
            "audioSampleRate": "48000", "loudnessDb": -3.5,
            "audioTrack": {"id": "en.4", "displayName": "English", "audioIsDefault": true},
            "drmFamilies": ["WIDEVINE"]});
        let s = parse_stream(&f).unwrap();
        assert_eq!((s.itag, s.bitrate, s.content_length, s.sample_rate), (251, 138000, 1234, Some(48000)));
        assert_eq!(s.codecs.as_deref(), Some("opus"));
        assert!(s.is_audio() && s.is_opus() && s.drm);
        assert_eq!(s.track.unwrap().id, "en.4");
        assert!(parse_stream(&json!({"itag": 1, "mimeType": "audio/mp4"})).is_none());
        assert!(parse_stream(&json!({"mimeType": "audio/mp4", "url": "u"})).is_none());
    }
}
