//! Lenient parsers for InnerTube `browse` responses (client WEB).
//!
//! The walker looks for the renderer objects wherever they sit instead of following one fixed path, because the
//! service moves them between layouts (`playlistVideoListRenderer`, `richGridRenderer`, `sectionListRenderer`, continuation
//! actions). Anything it does not recognise is skipped; nothing here panics or returns an error for a missing field.

use super::model::{ChannelRef, Page, PlaylistSummary, Thumbnail, Verification, Video};
use crate::{Error, ids, jsonx};
use serde_json::Value;

const MAX_DEPTH: usize = 40;

#[derive(Default)]
struct Found {
    videos: Vec<Video>,
    playlists: Vec<PlaylistSummary>,
    continuation: Option<String>,
}

/// Parses a `browse` response (first page or continuation) into videos.
/// `Err(Parse)` only when the root is not a JSON object; `Err(Refused)` for a top-level service `error`.
pub fn parse_video_page(root: &Value) -> Result<Page<Video>, Error> {
    let f = scan(root)?;
    Ok(Page { items: f.videos, continuation: f.continuation })
}

/// Same as [`parse_video_page`] for a listing of playlists.
pub fn parse_playlist_page(root: &Value) -> Result<Page<PlaylistSummary>, Error> {
    let f = scan(root)?;
    Ok(Page { items: f.playlists, continuation: f.continuation })
}

fn scan(root: &Value) -> Result<Found, Error> {
    let obj = root.as_object().ok_or(Error::Parse)?;
    if obj.contains_key("error") {
        return Err(Error::Refused);
    }
    let mut f = Found::default();
    visit(root, 0, &mut f);
    Ok(f)
}

fn visit(v: &Value, depth: usize, f: &mut Found) {
    if depth > MAX_DEPTH {
        return;
    }
    match v {
        Value::Array(items) => items.iter().for_each(|c| visit(c, depth + 1, f)),
        Value::Object(map) => {
            for (key, child) in map {
                match key.as_str() {
                    "playlistVideoRenderer" | "videoRenderer" | "gridVideoRenderer" => {
                        f.videos.extend(video_renderer(child));
                    }
                    "reelItemRenderer" => f.videos.extend(reel_item(child)),
                    "shortsLockupViewModel" => f.videos.extend(shorts_lockup(child)),
                    "gridPlaylistRenderer" | "playlistRenderer" => f.playlists.extend(playlist_renderer(child)),
                    "lockupViewModel" => lockup(child, f),
                    "continuationItemRenderer" => {
                        if let Some(t) = continuation_token(child) {
                            f.continuation = Some(t);
                        }
                    }
                    _ => visit(child, depth + 1, f),
                }
            }
        }
        _ => {}
    }
}

fn continuation_token(item: &Value) -> Option<String> {
    let cmd = jsonx::find_all(item, "continuationCommand").into_iter().next()?;
    let t = cmd.get("token")?.as_str()?;
    (!t.is_empty()).then(|| t.to_owned())
}

fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|x| !x.is_empty()).map(str::to_owned)
}

fn thumbs(list: &Value) -> Vec<Thumbnail> {
    let arr = list.as_array().or_else(|| list.get("thumbnails").and_then(Value::as_array));
    let Some(arr) = arr else { return Vec::new() };
    arr.iter()
        .filter_map(|t| {
            let url = t.get("url")?.as_str().filter(|u| !u.is_empty())?;
            let url = if url.starts_with("//") { format!("https:{url}") } else { url.to_owned() };
            let n = |k: &str| t.get(k).and_then(Value::as_u64).map_or(0, |n| n.min(u32::MAX as u64) as u32);
            Some(Thumbnail { url, width: n("width"), height: n("height") })
        })
        .collect()
}

/// "1:02:03" / "12:34" / "45" to milliseconds. Anything else (e.g. "LIVE", "") is `None`.
pub(crate) fn duration_from_clock(text: &str) -> Option<u64> {
    let mut secs: u64 = 0;
    let mut parts = 0;
    for p in text.trim().split(':') {
        if p.is_empty() || p.len() > 6 || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        secs = secs.checked_mul(60)?.checked_add(p.parse::<u64>().ok()?)?;
        parts += 1;
    }
    (1..=3).contains(&parts).then(|| secs.saturating_mul(1000))
}

/// Digits of a text such as "1,234 videos" as a number. `None` when there are no digits ("No videos").
fn count_from_text(text: &str) -> Option<u64> {
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() || digits.len() > 15 { None } else { digits.parse().ok() }
}

fn channel_from(r: &Value) -> Option<ChannelRef> {
    for key in ["shortBylineText", "longBylineText", "ownerText"] {
        let Some(node) = r.get(key) else { continue };
        let Some(name) = jsonx::text(node) else { continue };
        let id = jsonx::get(node, "runs[0].navigationEndpoint.browseEndpoint.browseId")
            .and_then(Value::as_str)
            .filter(|i| ids::is_channel_id(i))
            .map(str::to_owned);
        return Some(ChannelRef { id, name, verification: verification_from(r) });
    }
    None
}

/// The badge on a video row's channel: `ownerBadges` entries carry the icon type (`CHECK_CIRCLE_THICK` for a verified
/// channel, `OFFICIAL_ARTIST_BADGE` for a verified artist). Anything else, including no badge at all, is `None`.
fn verification_from(r: &Value) -> Verification {
    let icons: Vec<&str> = r
        .get("ownerBadges")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|b| jsonx::get(b, "metadataBadgeRenderer.icon.iconType").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    if icons.contains(&"OFFICIAL_ARTIST_BADGE") {
        Verification::Artist
    } else if icons.contains(&"CHECK_CIRCLE_THICK") {
        Verification::Verified
    } else {
        Verification::None
    }
}

fn video_renderer(r: &Value) -> Option<Video> {
    let id = s(r, "videoId").filter(|i| ids::is_video_id(i))?;
    let title = jsonx::text(r.get("title")?)?;

    let overlay = r
        .get("thumbnailOverlays")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find_map(|o| o.get("thumbnailOverlayTimeStatusRenderer")));
    let style = overlay.and_then(|o| o.get("style")).and_then(Value::as_str).unwrap_or("");
    let live_badge = r.get("badges").and_then(Value::as_array).is_some_and(|a| {
        a.iter().any(|b| {
            jsonx::get(b, "metadataBadgeRenderer.style").and_then(Value::as_str) == Some("BADGE_STYLE_TYPE_LIVE_NOW")
        })
    });
    let is_live = style == "LIVE" || live_badge;
    let url = jsonx::get(r, "navigationEndpoint.commandMetadata.webCommandMetadata.url")
        .and_then(Value::as_str)
        .unwrap_or("");
    let is_short = style == "SHORTS"
        || url.starts_with("/shorts/")
        || jsonx::get(r, "navigationEndpoint.reelWatchEndpoint").is_some();

    let duration_ms = (!is_live)
        .then(|| {
            // lengthSeconds is a string in the real payload; accept a number too.
            let secs = r
                .get("lengthSeconds")
                .and_then(|v| v.as_str().and_then(|x| x.parse::<u64>().ok()).or_else(|| v.as_u64()));
            secs.map(|x| x.saturating_mul(1000))
                .or_else(|| r.get("lengthText").and_then(jsonx::text).and_then(|t| duration_from_clock(&t)))
                .or_else(|| {
                    overlay.and_then(|o| o.get("text")).and_then(jsonx::text).and_then(|t| duration_from_clock(&t))
                })
        })
        .flatten();

    Some(Video {
        id,
        title,
        channel: channel_from(r),
        duration_ms,
        thumbnails: r.get("thumbnail").map(thumbs).unwrap_or_default(),
        is_live,
        is_short,
        set_video_id: s(r, "setVideoId"),
    })
}

fn reel_item(r: &Value) -> Option<Video> {
    let id = s(r, "videoId").filter(|i| ids::is_video_id(i))?;
    let title = jsonx::text(r.get("headline")?)?;
    Some(Video {
        id,
        title,
        channel: None,
        duration_ms: None,
        thumbnails: r.get("thumbnail").map(thumbs).unwrap_or_default(),
        is_live: false,
        is_short: true,
        set_video_id: None,
    })
}

fn shorts_lockup(r: &Value) -> Option<Video> {
    let id = jsonx::get(r, "onTap.innertubeCommand.reelWatchEndpoint.videoId")
        .and_then(Value::as_str)
        .filter(|i| ids::is_video_id(i))?
        .to_owned();
    let title = jsonx::get(r, "overlayMetadata.primaryText.content")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())?
        .to_owned();
    Some(Video {
        id,
        title,
        channel: None,
        duration_ms: None,
        thumbnails: jsonx::get(r, "onTap.innertubeCommand.reelWatchEndpoint.thumbnail").map(thumbs).unwrap_or_default(),
        is_live: false,
        is_short: true,
        set_video_id: None,
    })
}

fn playlist_renderer(r: &Value) -> Option<PlaylistSummary> {
    let id = s(r, "playlistId").filter(|i| ids::is_playlist_id(i))?;
    let title = jsonx::text(r.get("title")?)?;
    let video_count = ["videoCountText", "videoCountShortText"]
        .iter()
        .find_map(|k| r.get(*k).and_then(jsonx::text))
        .and_then(|t| count_from_text(&t))
        .or_else(|| r.get("videoCount").and_then(Value::as_str).and_then(count_from_text));
    let thumbnails = r
        .get("thumbnail")
        .map(thumbs)
        .filter(|t| !t.is_empty())
        .or_else(|| r.get("thumbnails").and_then(|a| a.get(0)).map(thumbs))
        .or_else(|| r.get("sidebarThumbnails").and_then(|a| a.get(0)).map(thumbs))
        .unwrap_or_default();
    Some(PlaylistSummary { id, title, video_count, thumbnails })
}

/// Newer layout: one `lockupViewModel` for videos and playlists, told apart by `contentType`.
fn lockup(r: &Value, f: &mut Found) {
    let Some(id) = s(r, "contentId") else { return };
    let kind = r.get("contentType").and_then(Value::as_str).unwrap_or("");
    let Some(title) = jsonx::get(r, "metadata.lockupMetadataViewModel.title.content")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
    else {
        return;
    };
    let image = r.get("contentImage");
    let sources = image.and_then(|i| jsonx::find_all(i, "sources").into_iter().next()).map(thumbs).unwrap_or_default();
    let badges: Vec<String> = image
        .map(|i| jsonx::find_all(i, "thumbnailBadgeViewModel"))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|b| b.get("text").and_then(Value::as_str).map(str::to_owned))
        .collect();

    match kind {
        "LOCKUP_CONTENT_TYPE_PLAYLIST" if ids::is_playlist_id(&id) => f.playlists.push(PlaylistSummary {
            id,
            title,
            video_count: badges.iter().find_map(|b| count_from_text(b)),
            thumbnails: sources,
        }),
        "LOCKUP_CONTENT_TYPE_VIDEO" if ids::is_video_id(&id) => {
            let is_live = badges.iter().any(|b| b.eq_ignore_ascii_case("live"));
            let duration_ms = if is_live { None } else { badges.iter().find_map(|b| duration_from_clock(b)) };
            let meta = r.get("metadata").and_then(|m| jsonx::find_all(m, "metadataParts").into_iter().next());
            let name = meta.and_then(|p| p.get(0)).and_then(|p| jsonx::get(p, "text.content")).and_then(Value::as_str);
            let cid = r
                .get("metadata")
                .map(|m| jsonx::find_all(m, "browseId"))
                .unwrap_or_default()
                .into_iter()
                .filter_map(Value::as_str)
                .find(|i| ids::is_channel_id(i))
                .map(str::to_owned);
            f.videos.push(Video {
                id,
                title,
                channel: name.filter(|n| !n.is_empty()).map(|n| ChannelRef {
                    id: cid,
                    name: n.to_owned(),
                    verification: Verification::None,
                }),
                duration_ms,
                thumbnails: sources,
                is_live,
                is_short: false,
                set_video_id: None,
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    #[test]
    fn clock() {
        assert_eq!(duration_from_clock("3:25"), Some(205_000));
        assert_eq!(duration_from_clock("1:02:03"), Some(3_723_000));
        assert_eq!(duration_from_clock("45"), Some(45_000));
        for bad in ["", "LIVE", "1:2:3:4", "a:b", ":", "1::2", "-1:00"] {
            assert_eq!(duration_from_clock(bad), None, "{bad}");
        }
    }

    #[test]
    fn counts() {
        assert_eq!(count_from_text("1,234 videos"), Some(1234));
        assert_eq!(count_from_text("No videos"), None);
    }

    #[test]
    fn channel_badges_map_to_verification() {
        let row = |badges: Value| json!({"ownerText": {"runs": [{"text": "Band"}]}, "ownerBadges": badges});
        let icon = |t: &str| json!({"metadataBadgeRenderer": {"icon": {"iconType": t}}});
        assert_eq!(verification_from(&row(json!([icon("OFFICIAL_ARTIST_BADGE")]))), Verification::Artist);
        assert_eq!(verification_from(&row(json!([icon("CHECK_CIRCLE_THICK")]))), Verification::Verified);
        assert_eq!(verification_from(&row(json!([icon("SOMETHING_ELSE")]))), Verification::None);
        assert_eq!(verification_from(&row(json!(5))), Verification::None);
        assert_eq!(verification_from(&json!({"ownerText": {}})), Verification::None);
    }
}
