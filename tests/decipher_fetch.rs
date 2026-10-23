//! Fetching the loader and the script through the fixture transport: what is requested, what is refused, caching.

mod decipher_common;
use decipher_common::*;
use innerlight::decipher::{DecipherError, MAX_SCRIPT_BYTES, PlayerScript, ScriptCache, Which};
use innerlight::{Client, Error, FixtureTransport};
use std::sync::Arc;
use std::time::Duration;

const LOADER: &str =
    "var scriptUrl = 'https:\\/\\/www.youtube.com\\/s\\/player\\/test1234\\/www-widgetapi.vflset\\/www-widgetapi.js';";
const SCRIPT_PATH: &str = "s/player/test1234/player_ias.vflset/en_US/base.js";

fn client(t: &Arc<FixtureTransport>) -> Client {
    Client::builder().transport(t.clone()).build().unwrap()
}

fn transport() -> Arc<FixtureTransport> {
    Arc::new(FixtureTransport::new().with_raw("iframe_api", 200, LOADER.as_bytes()).with_raw(
        SCRIPT_PATH,
        200,
        SCRIPT_A.as_bytes(),
    ))
}

#[tokio::test]
async fn fetch_current_makes_two_anonymous_gets() {
    let t = transport();
    let c = client(&t);
    c.set_cookie("SAPISID=secret; __Secure-3PAPISID=secret").unwrap();
    let s = PlayerScript::fetch_current(&c).await.unwrap();
    assert_eq!(s.version(), "test1234");
    assert_eq!(s.signature_timestamp(), Ok(19900));
    let r = t.requests();
    assert_eq!(r.len(), 2);
    assert_eq!((r[0].host.as_str(), r[0].endpoint.as_str()), ("www.youtube.com", "iframe_api"));
    assert_eq!((r[1].host.as_str(), r[1].endpoint.as_str()), ("www.youtube.com", SCRIPT_PATH));
    assert!(r.iter().all(|q| q.headers.is_empty()), "a GET carries no cookie or account header");
}

#[tokio::test]
async fn refusals() {
    // loader without a version
    let t = Arc::new(FixtureTransport::new().with_raw("iframe_api", 200, b"nothing here"));
    assert_eq!(
        PlayerScript::fetch_current(&client(&t)).await.unwrap_err(),
        DecipherError::ScriptShapeChanged { which: Which::Version }
    );
    // loader that is not UTF-8
    let t = Arc::new(FixtureTransport::new().with_raw("iframe_api", 200, &[0xff, 0xfe, 0xfd]));
    assert_eq!(PlayerScript::fetch_current(&client(&t)).await.unwrap_err(), DecipherError::BadScript);
    // HTTP failure
    let t = Arc::new(FixtureTransport::new().with_raw("iframe_api", 403, b""));
    assert_eq!(PlayerScript::fetch_current(&client(&t)).await.unwrap_err(), DecipherError::Transport(Error::Refused));
    // oversized script (still below the client's own 8 MiB limit)
    let big = vec![b'a'; MAX_SCRIPT_BYTES + 1];
    let t = Arc::new(FixtureTransport::new().with_raw("iframe_api", 200, LOADER.as_bytes()).with_raw(
        SCRIPT_PATH,
        200,
        &big,
    ));
    assert_eq!(PlayerScript::fetch_current(&client(&t)).await.unwrap_err(), DecipherError::BadScript);
    // empty script, non-UTF-8 script
    for body in [&b""[..], &[0xff, 0xfe][..]] {
        let t = Arc::new(FixtureTransport::new().with_raw("iframe_api", 200, LOADER.as_bytes()).with_raw(
            SCRIPT_PATH,
            200,
            body,
        ));
        assert_eq!(PlayerScript::fetch_current(&client(&t)).await.unwrap_err(), DecipherError::BadScript);
    }
    // a malicious version in the loader is not turned into a URL
    let evil = "'https:\\/\\/www.youtube.com\\/s\\/player\\/..%2F..%2Fx\\/a.js'";
    let t = Arc::new(FixtureTransport::new().with_raw("iframe_api", 200, evil.as_bytes()));
    assert!(PlayerScript::fetch_current(&client(&t)).await.is_err());
    assert_eq!(t.requests().len(), 1);
}

#[tokio::test]
async fn get_static_only_goes_to_the_site() {
    // the transport has no rules: any request that got through would panic
    let t = Arc::new(FixtureTransport::new());
    let c = client(&t);
    for url in [
        "http://www.youtube.com/iframe_api",
        "https://evil.example/s/player/x/base.js",
        "https://www.youtube.com.evil.example/iframe_api",
        "https://user@www.youtube.com/iframe_api",
        "https://www.youtube.com:8443/iframe_api",
        "https://www.youtube.com/iframe_api#frag",
        "ftp://www.youtube.com/x",
        "//www.youtube.com/x",
        "",
        "not a url",
    ] {
        assert_eq!(c.get_static(url).await.unwrap_err(), Error::InvalidRequest, "{url}");
    }
    assert!(t.requests().is_empty());
}

#[tokio::test]
async fn cache_reuses_per_version_and_refetches_on_demand() {
    let t = transport();
    let c = client(&t);
    let cache = ScriptCache::new();
    assert_eq!(cache.cached_version(), None);
    let d1 = cache.get(&c).await.unwrap();
    assert_eq!(cache.cached_version().as_deref(), Some("test1234"));
    let d2 = cache.get(&c).await.unwrap();
    assert!(Arc::ptr_eq(&d1, &d2));
    assert_eq!(t.requests().len(), 2, "second get is served from memory");

    // recheck interval elapsed: only the small loader is requested, the script is kept for the same version
    let cache = ScriptCache::new().with_recheck_interval(Duration::ZERO);
    let a = cache.get(&c).await.unwrap();
    let b = cache.get(&c).await.unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!(t.requests().len(), 2 + 2 + 1);

    // invalidate (after a DecipherFailed): everything is fetched and analysed again
    cache.invalidate();
    let n = t.requests().len();
    let again = cache.get(&c).await.unwrap();
    assert!(!Arc::ptr_eq(&a, &again));
    assert_eq!(t.requests().len(), n + 2);
    assert_eq!(again.signature(SIG_IN).unwrap(), ref_sig_a(SIG_IN));
}

#[tokio::test]
async fn cache_follows_a_new_version() {
    let loader2 = LOADER.replace("test1234", "next5678");
    let t = Arc::new(
        FixtureTransport::new()
            .with_raw("iframe_api", 200, LOADER.as_bytes())
            .with_raw("iframe_api", 200, loader2.as_bytes())
            .with_raw(SCRIPT_PATH, 200, SCRIPT_A.as_bytes())
            .with_raw("s/player/next5678/player_ias.vflset/en_US/base.js", 200, SCRIPT_B.as_bytes()),
    );
    let c = client(&t);
    let cache = ScriptCache::new().with_recheck_interval(Duration::ZERO);
    assert_eq!(cache.get(&c).await.unwrap().version(), "test1234");
    let d = cache.get(&c).await.unwrap();
    assert_eq!(d.version(), "next5678");
    assert_eq!(d.signature(SIG_IN).unwrap(), ref_sig_b(SIG_IN));
}

#[tokio::test]
async fn cache_does_not_keep_a_broken_script() {
    let t = Arc::new(FixtureTransport::new().with_raw("iframe_api", 200, LOADER.as_bytes()).with_raw(
        SCRIPT_PATH,
        200,
        b"var a=1;",
    ));
    let cache = ScriptCache::new();
    let e = cache.get(&client(&t)).await.unwrap_err();
    assert!(matches!(e, DecipherError::ScriptShapeChanged { .. }), "{e:?}");
    assert_eq!(cache.cached_version(), None);
}
