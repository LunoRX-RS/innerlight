//! Offline tests for the transport layer. Fixtures are hand-written; no network is used.

use innerlight::transport::{BoxFuture, Request, Response};
use innerlight::{Client, ClientKind, Error, FixtureTransport, Transport};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const SEARCH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/handwritten_web_music_search_minimal.json");

fn client_with(fx: FixtureTransport) -> (Client, Arc<FixtureTransport>) {
    let fx = Arc::new(fx);
    let client = Client::builder().language("en").country("US").transport(fx.clone()).build().unwrap();
    (client, fx)
}

#[tokio::test]
async fn post_adds_context_and_targets_the_right_host() {
    let (c, fx) = client_with(FixtureTransport::new().with_file("search", SEARCH).with("next", json!({})));
    let out = c.post(ClientKind::WebMusic, "search", json!({"query": "x"})).await.unwrap();
    assert!(out.pointer("/contents/tabbedSearchResultsRenderer").is_some());
    c.post(ClientKind::Web, "next", json!({})).await.unwrap();

    let reqs = fx.requests();
    assert_eq!((reqs[0].host.as_str(), reqs[0].endpoint.as_str()), ("music.youtube.com", "search"));
    assert_eq!((reqs[1].host.as_str(), reqs[1].endpoint.as_str()), ("www.youtube.com", "next"));
    let ctx = &reqs[0].body["context"]["client"];
    assert_eq!(ctx["clientName"], "WEB_REMIX");
    assert_eq!((ctx["hl"].as_str(), ctx["gl"].as_str()), (Some("en"), Some("US")));
    assert!(ctx["clientVersion"].as_str().is_some_and(|v| !v.is_empty()));
    assert_eq!(reqs[0].body["query"], "x");
    assert_eq!(reqs[1].body["context"]["client"]["clientName"], "WEB");
    assert_eq!(reqs[0].header("origin"), Some("https://music.youtube.com"));
}

#[tokio::test]
async fn anonymous_post_never_sends_cookie_even_when_set() {
    let (c, fx) = client_with(FixtureTransport::new().with("browse", json!({})));
    c.set_cookie("SAPISID=secret; SID=other").unwrap();
    c.post(ClientKind::Web, "browse", json!({})).await.unwrap();
    let r = &fx.requests()[0];
    assert!(r.header("cookie").is_none() && r.header("authorization").is_none());
}

#[tokio::test]
async fn post_authed_signs_the_request() {
    let (c, fx) = client_with(FixtureTransport::new().with("browse", json!({})));
    c.set_cookie("SAPISID=abc123; SID=x").unwrap();
    assert!(c.is_signed_in());
    c.post_authed(ClientKind::WebMusic, "browse", json!({})).await.unwrap();
    let r = &fx.requests()[0];
    let auth = r.header("authorization").unwrap();
    let rest = auth.strip_prefix("SAPISIDHASH ").unwrap();
    let (ts, hash) = rest.split_once('_').unwrap();
    assert!(ts.parse::<u64>().unwrap() > 1_700_000_000);
    assert_eq!(hash.len(), 40);
    assert_eq!(r.header("x-goog-authuser"), Some("0"));
    assert_eq!(r.header("cookie"), Some("SAPISID=abc123; SID=x"));
    assert_eq!(r.header("origin"), Some("https://music.youtube.com"));
}

#[tokio::test]
async fn post_authed_without_cookie_fails_before_sending() {
    let (c, fx) = client_with(FixtureTransport::new().with("browse", json!({})));
    assert_eq!(c.post_authed(ClientKind::Web, "browse", json!({})).await, Err(Error::Unauthenticated));
    c.set_cookie("SID=only").unwrap(); // no SAPISID: cannot sign
    assert_eq!(c.post_authed(ClientKind::Web, "browse", json!({})).await, Err(Error::Unauthenticated));
    c.set_cookie("SAPISID=a").unwrap();
    c.clear_session();
    assert_eq!(c.post_authed(ClientKind::Web, "browse", json!({})).await, Err(Error::Unauthenticated));
    assert!(fx.requests().is_empty());
}

#[tokio::test]
async fn bad_endpoints_and_bodies_are_rejected_before_sending() {
    let (c, fx) = client_with(FixtureTransport::new().with("browse", json!({})));
    for e in
        ["", "..", "a/../b", "/browse", "browse/", "a//b", "Browse", "browse?x=1", "br owse", "browse\n", "a.b", "b1"]
    {
        assert_eq!(c.post(ClientKind::Web, e, json!({})).await, Err(Error::InvalidRequest), "{e:?}");
    }
    for body in [json!(null), json!([1]), json!("s")] {
        assert_eq!(c.post(ClientKind::Web, "browse", body).await, Err(Error::InvalidRequest));
    }
    assert!(fx.requests().is_empty());
    for ok in ["browse", "like/like", "music/get_search_suggestions"] {
        assert!(innerlight::client::validate_endpoint(ok).is_ok(), "{ok}");
    }
}

#[tokio::test]
async fn http_statuses_map_to_errors() {
    let fx = FixtureTransport::new()
        .with_raw("unauth", 401, b"{}")
        .with_raw("forbidden", 403, b"{}")
        .with_raw("limited", 429, b"{}")
        .with_raw("err_a", 500, b"oops")
        .with_raw("err_b", 503, b"oops")
        .with_raw("missing", 404, b"")
        .with_raw("bad", 200, b"<html>not json")
        .with_raw("empty", 200, b"");
    let (c, _) = client_with(fx);
    let post = |e: &'static str| {
        let c = c.clone();
        async move { c.post(ClientKind::Web, e, json!({})).await }
    };
    assert_eq!(post("unauth").await, Err(Error::Unauthenticated));
    assert_eq!(post("forbidden").await, Err(Error::Refused));
    assert_eq!(post("limited").await, Err(Error::RateLimited { retry_after: None }));
    assert_eq!(post("err_a").await, Err(Error::Network));
    assert_eq!(post("err_b").await, Err(Error::Network));
    assert_eq!(post("missing").await, Err(Error::Refused));
    assert_eq!(post("bad").await, Err(Error::Parse));
    assert_eq!(post("empty").await, Err(Error::Parse));
}

struct Canned(Response);
impl Transport for Canned {
    fn send(&self, _: Request) -> BoxFuture<'_, Result<Response, Error>> {
        let r = Response { status: self.0.status, retry_after: self.0.retry_after, body: self.0.body.clone() };
        Box::pin(async move { Ok(r) })
    }
}

struct Never;
impl Transport for Never {
    fn send(&self, _: Request) -> BoxFuture<'_, Result<Response, Error>> {
        Box::pin(std::future::pending())
    }
}

#[tokio::test]
async fn retry_after_is_passed_through() {
    let t = Canned(Response { status: 429, retry_after: Some(Duration::from_secs(30)), body: vec![] });
    let c = Client::builder().transport(Arc::new(t)).build().unwrap();
    assert_eq!(
        c.post(ClientKind::Web, "browse", json!({})).await,
        Err(Error::RateLimited { retry_after: Some(Duration::from_secs(30)) })
    );
}

#[tokio::test]
async fn oversized_body_is_rejected() {
    let big = format!("{{\"x\":\"{}\"}}", "a".repeat(2000));
    let t = Canned(Response { status: 200, retry_after: None, body: big.into_bytes() });
    let c = Client::builder().max_response_bytes(1000).transport(Arc::new(t)).build().unwrap();
    assert_eq!(c.post(ClientKind::Web, "browse", json!({})).await, Err(Error::TooLarge));
}

#[tokio::test]
async fn timeout_maps_to_network() {
    let c = Client::builder().timeout(Duration::from_millis(30)).transport(Arc::new(Never)).build().unwrap();
    assert_eq!(c.post(ClientKind::Web, "browse", json!({})).await, Err(Error::Network));
}

#[tokio::test]
async fn visitor_data_is_learned_then_sent() {
    let (c, fx) = client_with(FixtureTransport::new().with_file("search", SEARCH));
    assert_eq!(c.visitor_data(), None);
    c.post(ClientKind::WebMusic, "search", json!({})).await.unwrap();
    assert_eq!(c.visitor_data().as_deref(), Some("CgtIQU5ETFdSSVRURU4%3D"));
    c.post(ClientKind::WebMusic, "search", json!({})).await.unwrap();
    let second = &fx.requests()[1];
    assert_eq!(second.header("x-goog-visitor-id"), Some("CgtIQU5ETFdSSVRURU4%3D"));
    assert_eq!(second.body["context"]["client"]["visitorData"], "CgtIQU5ETFdSSVRURU4%3D");
    c.set_visitor_data(None);
    assert_eq!(c.visitor_data(), None);
}

#[tokio::test]
async fn fixture_sequences_and_body_matching() {
    let fx = FixtureTransport::new()
        .with_match("browse", "TOKEN2", json!({"page": 2}))
        .with("browse", json!({"page": 1}))
        .with("browse", json!({"page": "later"}));
    let (c, fx) = client_with(fx);
    let post = |b: Value| c.post(ClientKind::Web, "browse", b);
    assert_eq!(post(json!({"continuation": "TOKEN2"})).await.unwrap()["page"], 2);
    assert_eq!(post(json!({})).await.unwrap()["page"], 1);
    assert_eq!(post(json!({})).await.unwrap()["page"], "later");
    assert_eq!(post(json!({})).await.unwrap()["page"], "later"); // last response repeats
    assert_eq!(fx.requests().len(), 4);
}

#[test]
fn builder_validates_locale_and_debug_is_opaque() {
    assert!(Client::builder().language("en-GB").country("GB").build().is_ok());
    assert_eq!(Client::builder().language("e n").build().unwrap_err(), Error::InvalidRequest);
    assert_eq!(Client::builder().country("USA").build().unwrap_err(), Error::InvalidRequest);
    let c = Client::builder().build().unwrap();
    c.set_cookie("SAPISID=topsecret").unwrap();
    assert_eq!(format!("{c:?}"), "Client(..)");
    assert!(c.set_cookie("a\nb").is_err());
}
