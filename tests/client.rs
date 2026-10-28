//! Offline tests for the transport layer. Fixtures are hand-written; no network is used.

mod common;

use common::{builder, builder_with_clock};
use innerlight::transport::{BoxFuture, Request, Response};
use innerlight::{Client, ClientKind, Error, FixtureTransport, Policy, Transport};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const SEARCH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/handwritten_web_music_search_minimal.json");

fn client_with(fx: FixtureTransport) -> (Client, Arc<FixtureTransport>) {
    let fx = Arc::new(fx);
    let client = builder().language("en").country("US").transport(fx.clone()).build().unwrap();
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
        .with_raw("challenge", 403, br#"{"error":{"message":"Sign in to confirm you're not a bot"}}"#)
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
    assert_eq!(post("err_a").await, Err(Error::ServerError));
    assert_eq!(post("err_b").await, Err(Error::ServerError));
    assert_eq!(post("missing").await, Err(Error::Refused));
    assert_eq!(post("bad").await, Err(Error::Parse));
    assert_eq!(post("empty").await, Err(Error::Parse));
    // The bot challenge is named, and naming it is where the work stops: no retry, no other client identity.
    assert_eq!(post("challenge").await, Err(Error::BotCheck));
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
    let c = builder().retry_attempts(1).transport(Arc::new(t)).build().unwrap();
    assert_eq!(
        c.post(ClientKind::Web, "browse", json!({})).await,
        Err(Error::RateLimited { retry_after: Some(Duration::from_secs(30)) })
    );
}

#[tokio::test]
async fn a_transient_failure_is_resent_and_a_rate_limit_waits_for_what_was_asked() {
    let fx = Arc::new(
        FixtureTransport::new()
            .with_retry_after("browse", 429, Duration::from_secs(7), b"{}")
            .with("browse", json!({"ok": true})),
    );
    let (b, clock) = builder_with_clock();
    let c = b.language("en").country("US").transport(fx.clone()).build().unwrap();
    assert_eq!(c.post(ClientKind::Web, "browse", json!({})).await.unwrap()["ok"], true);
    assert_eq!(fx.requests().len(), 2, "the second send is the retry");
    // The asked-for wait is kept exactly: not shortened, and no spread added to a delay the service named.
    assert_eq!(clock.elapsed_ms(), 7_000, "{} ms waited", clock.elapsed_ms());
}

#[tokio::test]
async fn a_retry_after_too_long_for_the_budget_stops_instead_of_sending_early() {
    let fx = Arc::new(
        FixtureTransport::new()
            .with_retry_after("browse", 429, Duration::from_secs(30), b"{}")
            .with("browse", json!({"ok": true})),
    );
    let (b, clock) = builder_with_clock();
    let c = b
        .transport(fx.clone())
        .retry_attempts(3)
        .retry_max_delay(Duration::from_secs(10))
        .retry_total_budget(Duration::from_secs(20))
        .build()
        .unwrap();
    assert_eq!(
        c.post(ClientKind::Web, "browse", json!({})).await,
        Err(Error::RateLimited { retry_after: Some(Duration::from_secs(30)) })
    );
    assert_eq!(fx.requests().len(), 1, "30 s was asked and only 20 s of budget is left: waiting 20 would be early");
    assert_eq!(clock.elapsed_ms(), 0, "nothing is parked partway: the caller gets the seconds to wait itself");
}

#[tokio::test]
async fn a_challenge_is_named_and_never_resent() {
    for (status, body, want) in [
        (403u16, br#"{"error":{"message":"Sign in to confirm you're not a bot"}}"#.to_vec(), Error::BotCheck),
        (403, b"{\"error\":{\"message\":\"quota exceeded\"}}".to_vec(), Error::QuotaExceeded),
        (401, b"{}".to_vec(), Error::Unauthenticated),
        (404, b"{}".to_vec(), Error::Refused),
    ] {
        let fx = Arc::new(FixtureTransport::new().with_raw("browse", status, &body));
        let c = builder().transport(fx.clone()).build().unwrap();
        assert_eq!(c.post(ClientKind::Web, "browse", json!({})).await, Err(want), "{status}");
        assert_eq!(fx.requests().len(), 1, "{want}: no retry, no other client identity, nothing to work around");
    }
}

#[tokio::test]
async fn retries_stop_at_the_attempt_ceiling_and_at_the_budget() {
    let fx = Arc::new(FixtureTransport::new().with_raw("browse", 500, b"oops"));
    let (b, clock) = builder_with_clock();
    let c = b.transport(fx.clone()).retry_attempts(3).build().unwrap();
    assert_eq!(c.post(ClientKind::Web, "browse", json!({})).await, Err(Error::ServerError));
    assert_eq!(fx.requests().len(), 3, "three sends, then the answer is reported");

    let fx = Arc::new(FixtureTransport::new().with_raw("browse", 500, b"oops"));
    let (b, clock2) = builder_with_clock();
    let c = b
        .transport(fx.clone())
        .retry_attempts(8)
        .requests_per_second(50) // so the only waiting here is the backoff, never the pacing
        .retry_base_delay(Duration::from_millis(200))
        .retry_max_delay(Duration::from_millis(400))
        .retry_total_budget(Duration::from_secs(1))
        .build()
        .unwrap();
    assert_eq!(c.post(ClientKind::Web, "browse", json!({})).await, Err(Error::ServerError));
    assert!(fx.requests().len() < 8, "{} sends past a 1 s budget", fx.requests().len());
    assert!(clock2.elapsed_ms() <= 1_000, "no wait is scheduled past the budget");
    assert!(clock.elapsed_ms() > 0);
}

#[tokio::test]
async fn a_backing_off_call_frees_its_concurrency_slot() {
    // One slot: if the wait held it, the second request could never start and this would hang.
    let fx = Arc::new(
        FixtureTransport::new()
            .with_retry_after("browse", 429, Duration::from_secs(20), b"{}")
            .with("browse", json!({})),
    );
    let c = builder().transport(fx.clone()).max_concurrent_requests(1).build().unwrap();
    let one = c.post(ClientKind::Web, "browse", json!({}));
    let two = c.post(ClientKind::Web, "browse", json!({}));
    let (a, b) = tokio::join!(one, two);
    assert!(a.is_ok() && b.is_ok(), "{a:?} {b:?}");
    assert_eq!(fx.requests().len(), 3, "one 429, one unperturbed call, one retry");
}

#[tokio::test]
async fn fixture_can_carry_retry_after() {
    let (c, _) = client_with(FixtureTransport::new().with_retry_after("browse", 429, Duration::from_secs(11), b"{}"));
    assert_eq!(
        c.post(ClientKind::Web, "browse", json!({})).await,
        Err(Error::RateLimited { retry_after: Some(Duration::from_secs(11)) })
    );
}

#[tokio::test]
async fn oversized_body_is_rejected() {
    let big = format!("{{\"x\":\"{}\"}}", "a".repeat(2000));
    let t = Canned(Response { status: 200, retry_after: None, body: big.into_bytes() });
    let c = builder().max_response_bytes(1000).transport(Arc::new(t)).build().unwrap();
    assert_eq!(c.post(ClientKind::Web, "browse", json!({})).await, Err(Error::TooLarge));
}

#[tokio::test]
async fn timeout_maps_to_its_own_error() {
    let c = builder().retry_attempts(1).timeout(Duration::from_millis(30)).transport(Arc::new(Never)).build().unwrap();
    assert_eq!(c.post(ClientKind::Web, "browse", json!({})).await, Err(Error::Timeout));
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
    assert_eq!(c.policy(), Policy::polite(), "politeness is the default, not opt-in");
    c.set_cookie("SAPISID=topsecret").unwrap();
    assert_eq!(format!("{c:?}"), "Client(..)");
    assert!(c.set_cookie("a\nb").is_err());
}

#[test]
fn builder_rejects_politeness_settings_that_could_deadlock_or_spin() {
    for bad in [
        Client::builder().retry_attempts(0),
        Client::builder().retry_attempts(9),
        Client::builder().requests_per_second(0),
        Client::builder().max_concurrent_requests(0),
        Client::builder().retry_base_delay(Duration::ZERO),
        Client::builder().retry_base_delay(Duration::from_secs(60)), // above the max delay
        Client::builder().retry_total_budget(Duration::from_secs(5)), // below the max delay
    ] {
        assert_eq!(bad.build().unwrap_err(), Error::InvalidRequest);
    }
    assert_eq!(Client::builder().policy(Policy::no_retry()).build().unwrap().policy().attempts, 1);
}

#[tokio::test]
async fn user_agent_is_fixed_and_names_this_library() {
    let (c, fx) = client_with(FixtureTransport::new().with("browse", json!({})).with("next", json!({})));
    c.post(ClientKind::Web, "browse", json!({})).await.unwrap();
    c.post(ClientKind::WebMusic, "next", json!({})).await.unwrap();
    let reqs = fx.requests();
    let ua = reqs[0].header("user-agent").unwrap();
    assert!(ua.starts_with("innerlight/") && !ua.contains("Mozilla"), "{ua}");
    assert_eq!(reqs[1].header("user-agent"), Some(ua), "never rotated");
}

#[tokio::test]
async fn no_proof_of_origin_token_is_ever_sent_and_a_challenge_waits_zero() {
    let fx = Arc::new(FixtureTransport::new().with_raw(
        "player",
        403,
        br#"{"error":{"message":"Sign in to confirm you're not a bot"}}"#,
    ));
    let (b, clock) = builder_with_clock();
    let c = b.transport(fx.clone()).build().unwrap();
    assert_eq!(c.post(ClientKind::Web, "player", json!({"videoId": "dQw4w9WgXcQ"})).await, Err(Error::BotCheck));
    let reqs = fx.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(clock.elapsed_ms(), 0, "a challenge is not waited out");
    let wire = format!("{:?}{}", reqs[0].headers, reqs[0].body).to_ascii_lowercase();
    assert!(
        !wire.contains("potoken") && !wire.contains("po_token") && !wire.contains("serviceintegritydimensions"),
        "{wire}"
    );
}

#[tokio::test]
async fn a_caller_chosen_user_agent_is_sent_as_given_and_validated() {
    let fx = Arc::new(FixtureTransport::new().with("browse", json!({})));
    let c = builder().user_agent("LunoRX/1.4 (+https://example.org)").transport(fx.clone()).build().unwrap();
    c.post(ClientKind::Web, "browse", json!({})).await.unwrap();
    c.post(ClientKind::Web, "browse", json!({})).await.unwrap();
    for r in fx.requests() {
        assert_eq!(r.header("user-agent"), Some("LunoRX/1.4 (+https://example.org)"));
    }
    for bad in ["", "a\r\nX-Evil: 1", "caf\u{e9}", &"x".repeat(201)] {
        assert_eq!(builder().user_agent(bad).build().err(), Some(Error::InvalidRequest), "{bad:?}");
    }
}

/// A clock whose waits never finish, so a retry parked on a backoff can be cancelled by dropping the call.
struct Stuck;
impl innerlight::Clock for Stuck {
    fn now_ms(&self) -> u64 {
        0
    }
    fn sleep_ms(&self, _: u64) -> BoxFuture<'_, ()> {
        Box::pin(std::future::pending())
    }
}

#[tokio::test]
async fn a_cancelled_call_sends_nothing_more() {
    let fx = Arc::new(FixtureTransport::new().with_raw("browse", 503, b"oops"));
    let c = Client::builder().clock(Arc::new(Stuck)).transport(fx.clone()).build().unwrap();
    let call = c.post(ClientKind::Web, "browse", json!({}));
    assert!(tokio::time::timeout(Duration::from_millis(100), call).await.is_err(), "parked on the backoff");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(fx.requests().len(), 1, "dropped while waiting: the retry is never sent");
}
