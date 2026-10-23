//! Offline tests for `innerlight::player::Player`: request shape, typed outcomes, selection from fixtures,
//! the Decipher plumbing. Fixtures are hand-authored (see docs/PLAYER.md).

use innerlight::player::{
    Decipher, Player, PlayerError, Quality, ResolveOptions, Resolved, StreamSource, UnplayableReason,
};
use innerlight::{Client, ClientKind, Error, FixtureTransport};
use serde_json::json;
use std::sync::Arc;

const VID: &str = "dQw4w9WgXcQ";

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/player_handwritten_{name}.json", env!("CARGO_MANIFEST_DIR"))
}

fn client_for(name: &str) -> (Client, Arc<FixtureTransport>) {
    let t = Arc::new(FixtureTransport::new().with_file("player", fixture(name)));
    (Client::builder().transport(t.clone()).build().unwrap(), t)
}

fn opts(quality: Quality) -> ResolveOptions {
    ResolveOptions { quality, ..ResolveOptions::default() }
}

async fn outcome(name: &str) -> Result<Resolved, PlayerError> {
    let (c, _) = client_for(name);
    Player::new(&c).resolve(VID, opts(Quality::High)).await
}

/// Reverses `s`, replaces the value of `n=`. Proves the plumbing, unscrambles nothing real.
struct Stub;
impl Decipher for Stub {
    fn decipher(&self, s: &str) -> Result<String, PlayerError> {
        Ok(s.chars().rev().collect())
    }
    fn throttle_fix(&self, url: &str) -> Result<String, PlayerError> {
        Ok(url.split('&').map(|p| if p.starts_with("n=") { "n=FIXED" } else { p }).collect::<Vec<_>>().join("&"))
    }
}

struct Evil;
impl Decipher for Evil {
    fn decipher(&self, s: &str) -> Result<String, PlayerError> {
        Ok(s.to_owned())
    }
    fn throttle_fix(&self, _: &str) -> Result<String, PlayerError> {
        Ok("https://evil.example/x".into())
    }
}

struct Failing;
impl Decipher for Failing {
    fn decipher(&self, _: &str) -> Result<String, PlayerError> {
        Err(PlayerError::DecipherFailed)
    }
    fn throttle_fix(&self, u: &str) -> Result<String, PlayerError> {
        Ok(u.to_owned())
    }
}

#[tokio::test]
async fn request_shape_anonymous_web() {
    let (c, t) = client_for("ok_direct");
    Player::new(&c)
        .resolve(VID, ResolveOptions { signature_timestamp: Some(20123), ..ResolveOptions::default() })
        .await
        .unwrap();
    let reqs = t.requests();
    assert_eq!(reqs.len(), 1);
    let r = &reqs[0];
    assert_eq!((r.host.as_str(), r.endpoint.as_str()), ("www.youtube.com", "player"));
    assert_eq!(r.body["videoId"], VID);
    assert_eq!(r.body["contentCheckOk"], true);
    assert_eq!(r.body["racyCheckOk"], true);
    let cpc = &r.body["playbackContext"]["contentPlaybackContext"];
    assert_eq!(cpc["signatureTimestamp"], 20123);
    assert!(cpc["html5Preference"].is_string());
    assert_eq!(r.body["context"]["client"]["clientName"], "WEB");
    assert!(r.header("authorization").is_none() && r.header("cookie").is_none());
}

#[tokio::test]
async fn request_shape_music_authed_without_timestamp() {
    let (c, t) = client_for("ok_direct");
    c.set_cookie("SAPISID=abc123; SID=x").unwrap();
    let o = ResolveOptions { kind: ClientKind::WebMusic, authed: true, ..ResolveOptions::default() };
    Player::new(&c).resolve(VID, o).await.unwrap();
    let r = &t.requests()[0];
    assert_eq!(r.host, "music.youtube.com");
    assert_eq!(r.body["context"]["client"]["clientName"], "WEB_REMIX");
    assert!(r.body["playbackContext"]["contentPlaybackContext"].get("signatureTimestamp").is_none());
    assert!(r.header("authorization").is_some_and(|h| h.starts_with("SAPISIDHASH ")));
}

#[tokio::test]
async fn authed_without_cookie_sends_nothing() {
    let (c, t) = client_for("ok_direct");
    let o = ResolveOptions { authed: true, ..ResolveOptions::default() };
    assert_eq!(Player::new(&c).resolve(VID, o).await.unwrap_err(), PlayerError::Transport(Error::Unauthenticated));
    assert!(t.requests().is_empty());
}

#[tokio::test]
async fn bad_id_sends_nothing() {
    let (c, t) = client_for("ok_direct");
    assert_eq!(Player::new(&c).resolve("nope", opts(Quality::High)).await.unwrap_err(), PlayerError::InvalidId);
    assert!(t.requests().is_empty());
}

#[tokio::test]
async fn direct_fixture_quality_choices() {
    // 139/249 <= 64k -> 249; <= 128k -> 250 (140 is 130k); best -> the default-track 251, not the louder dub.
    for (q, itag) in [(Quality::Low, 249), (Quality::Normal, 250), (Quality::High, 251)] {
        let (c, _) = client_for("ok_direct");
        let r = Player::new(&c).resolve(VID, opts(q)).await.unwrap();
        assert_eq!(r.itag, itag, "{q:?}");
        assert!(r.url.starts_with("https://") && r.url.contains(&format!("itag={itag}")), "{q:?}");
        assert!(r.size > 0);
        assert_eq!(r.expires_in, Some(std::time::Duration::from_secs(21540)));
        assert!(r.needs_throttle_fix, "direct urls carry n= and no decipher was given");
    }
}

#[tokio::test]
async fn default_track_and_metadata() {
    let r = outcome("ok_direct").await.unwrap();
    let t = r.track.unwrap();
    assert_eq!((t.id.as_str(), t.is_default), ("en.4", true));
    assert!(r.url.contains("rr2---"));
    assert_eq!((r.mime_type.as_str(), r.codecs.as_deref()), ("audio/webm; codecs=\"opus\"", Some("opus")));
    assert_eq!((r.bitrate, r.size, r.sample_rate, r.loudness_db), (138000, 3350000, Some(48000), Some(-4.1)));
}

#[tokio::test]
async fn debug_hides_url() {
    let r = outcome("ok_direct").await.unwrap();
    let dbg = format!("{r:?}");
    assert!(!dbg.contains("googlevideo") && !dbg.contains("http"), "{dbg}");
    let s = StreamSource::Ciphered { url: "https://a.googlevideo.com/x".into(), s: "SECRET".into(), sp: "sig".into() };
    assert!(!format!("{s:?}").contains("SECRET") && !format!("{s:?}").contains("googlevideo"));
}

#[tokio::test]
async fn challenges_are_typed_and_never_return_a_stream() {
    assert_eq!(outcome("login_plain").await, Err(PlayerError::LoginRequired { bot_check: false }));
    assert_eq!(outcome("login_bot").await, Err(PlayerError::LoginRequired { bot_check: true }));
    assert_eq!(outcome("age").await, Err(PlayerError::AgeRestricted));
    assert_eq!(outcome("live").await, Err(PlayerError::Live));
    assert_eq!(outcome("unplayable").await, Err(PlayerError::Unplayable { reason: UnplayableReason::Region }));
    assert_eq!(outcome("unavailable").await, Err(PlayerError::Unavailable));
}

#[tokio::test]
async fn bot_check_means_exactly_one_request() {
    let (c, t) = client_for("login_bot");
    c.set_cookie("SAPISID=abc123").unwrap();
    let o = ResolveOptions { authed: true, ..ResolveOptions::default() };
    assert_eq!(Player::new(&c).resolve(VID, o).await, Err(PlayerError::LoginRequired { bot_check: true }));
    assert_eq!(t.requests().len(), 1, "no retry, no other client");
}

#[tokio::test]
async fn ok_status_with_streams_but_challenge_text_is_still_a_challenge() {
    // Streams present in the same answer must not leak out when the status says otherwise.
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fixture("ok_direct")).unwrap()).unwrap();
    v["playabilityStatus"] = json!({"status": "LOGIN_REQUIRED", "reason": "Sign in to confirm you're not a bot"});
    let c = Client::builder().build().unwrap();
    assert_eq!(
        Player::new(&c).resolve_response(&v, Quality::High),
        Err(PlayerError::LoginRequired { bot_check: true })
    );
}

#[tokio::test]
async fn transport_errors_pass_through() {
    let t = Arc::new(FixtureTransport::new().with_raw("player", 429, b"{}"));
    let c = Client::builder().transport(t).build().unwrap();
    let e = Player::new(&c).resolve(VID, opts(Quality::High)).await.unwrap_err();
    assert!(matches!(e, PlayerError::Transport(Error::RateLimited { .. })));
    let t = Arc::new(FixtureTransport::new().with_raw("player", 200, b"<html>"));
    let c = Client::builder().transport(t).build().unwrap();
    assert_eq!(Player::new(&c).resolve(VID, opts(Quality::High)).await, Err(PlayerError::Transport(Error::Parse)));
}

#[tokio::test]
async fn error_text_never_leaks_response_content() {
    for name in ["login_bot", "age", "unplayable", "unavailable", "live", "login_plain"] {
        let e = outcome(name).await.unwrap_err();
        let m = format!("{e} {e:?}");
        for needle in ["http", "country", "cookie", "googlevideo", "Sign in", "protect"] {
            assert!(!m.contains(needle), "{name}: {m}");
        }
    }
}

#[tokio::test]
async fn ciphered_without_decipher_is_needs_decipher() {
    assert_eq!(outcome("ok_ciphered").await, Err(PlayerError::NeedsDecipher));
}

#[tokio::test]
async fn ciphered_with_stub_decipher() {
    let (c, _) = client_for("ok_ciphered");
    let p = Player::new(&c).with_decipher(Arc::new(Stub));
    let r = p.resolve(VID, opts(Quality::High)).await.unwrap();
    assert_eq!(r.itag, 251);
    assert!(r.url.contains("&sig=JIHGFEDCBA"), "{}", r.url.len());
    assert!(r.url.contains("n=FIXED") && !r.needs_throttle_fix);
    // legacy `cipher` field with sp=signature and a quality that picks the aac stream
    let r = p.resolve(VID, opts(Quality::Low)).await.unwrap();
    assert_eq!(r.itag, 140);
    assert!(r.url.contains("&signature=TSRQPONMLK"));
}

#[tokio::test]
async fn decipher_output_is_validated() {
    let (c, _) = client_for("ok_ciphered");
    let r = Player::new(&c).with_decipher(Arc::new(Evil)).resolve(VID, opts(Quality::High)).await;
    assert_eq!(r, Err(PlayerError::UnsafeUrl));
    let r = Player::new(&c).with_decipher(Arc::new(Failing)).resolve(VID, opts(Quality::High)).await;
    assert_eq!(r, Err(PlayerError::DecipherFailed));
}

#[tokio::test]
async fn direct_stream_with_decipher_gets_throttle_fix() {
    let (c, _) = client_for("ok_direct");
    let r = Player::new(&c).with_decipher(Arc::new(Stub)).resolve(VID, opts(Quality::High)).await.unwrap();
    assert!(r.url.contains("n=FIXED") && !r.needs_throttle_fix);
}

#[tokio::test]
async fn mixed_response_without_decipher_falls_back_to_direct() {
    let c = Client::builder().build().unwrap();
    let v = json!({"playabilityStatus": {"status": "OK"}, "streamingData": {"adaptiveFormats": [
        {"itag": 251, "mimeType": "audio/webm; codecs=\"opus\"", "bitrate": 160000, "contentLength": "10",
         "signatureCipher": "s=AB&sp=sig&url=https%3A%2F%2Fa.googlevideo.com%2Fv%3Fitag%3D251"},
        {"itag": 140, "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "bitrate": 130000, "contentLength": "10",
         "url": "https://a.googlevideo.com/v?itag=140"}]}});
    let p = Player::new(&c);
    assert_eq!(p.resolve_response(&v, Quality::High).unwrap().itag, 140);
    let p = p.with_decipher(Arc::new(Stub));
    assert_eq!(p.resolve_response(&v, Quality::High).unwrap().itag, 251);
}

#[tokio::test]
async fn unsafe_urls_are_never_returned() {
    let c = Client::builder().build().unwrap();
    for url in [
        "http://a.googlevideo.com/v",
        "https://evil.example/v",
        "https://a.googlevideo.com:444/v",
        "https://u@a.googlevideo.com/v",
    ] {
        let v = json!({"playabilityStatus": {"status": "OK"}, "streamingData": {"adaptiveFormats": [
            {"itag": 140, "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "bitrate": 1, "contentLength": "10", "url": url}]}});
        assert_eq!(Player::new(&c).resolve_response(&v, Quality::High), Err(PlayerError::UnsafeUrl), "{url}");
    }
}

#[tokio::test]
async fn malformed_and_empty_answers() {
    assert_eq!(outcome("malformed").await, Err(PlayerError::NoAudio));
    let c = Client::builder().build().unwrap();
    let p = Player::new(&c);
    // OK without streamingData, DRM-only, video-only
    assert_eq!(
        p.resolve_response(&json!({"playabilityStatus": {"status": "OK"}}), Quality::High),
        Err(PlayerError::Parse)
    );
    let drm = json!({"playabilityStatus": {"status": "OK"}, "streamingData": {"adaptiveFormats": [
        {"itag": 141, "mimeType": "audio/mp4", "bitrate": 1, "contentLength": "5", "url": "https://a.googlevideo.com/v", "drmTrackType": "X"}]}});
    assert_eq!(p.resolve_response(&drm, Quality::High), Err(PlayerError::NoAudio));
}
