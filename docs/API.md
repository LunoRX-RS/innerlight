# innerlight: public API (phase 1, transport layer)

Everything below exists in the code today. Paths are relative to the crate root (`innerlight::...`).
Re-exports at the root: `Client`, `ClientBuilder`, `ClientKind`, `Error`, `Session`, `Transport`, `FixtureTransport`, `ReqwestTransport`.
Modules: `client`, `error`, `ids`, `jsonx`, `session`, `transport`.

Higher modules (music, YouTube) should only talk to the network through `Client::post` / `Client::post_authed`, and parse the returned
`serde_json::Value` with `jsonx`. Every function returns `Result<_, innerlight::Error>`.

## Error (`error.rs`)

`#[derive(Debug, Clone, Copy, PartialEq, Eq)] enum Error`, messages are fixed text (no URL, cookie or body ever).

| Variant | Meaning |
|---|---|
| `InvalidId` | an id failed `ids::is_*` |
| `InvalidRequest` | bad endpoint string, body not a JSON object, bad locale, bad cookie string |
| `Network` | connect failure, timeout, TLS failure, HTTP 5xx |
| `Unauthenticated` | HTTP 401, or `post_authed` without a signable cookie (nothing is sent) |
| `Refused` | HTTP 403, and any other non-2xx status (3xx, 400, 404, ...) |
| `RateLimited { retry_after: Option<Duration> }` | HTTP 429; `retry_after` is the `Retry-After` header in whole seconds |
| `Parse` | body is not valid JSON (higher modules also use it for "JSON valid but expected shape missing") |
| `TooLarge` | body bigger than the limit (default 8 MiB) |

`Error::from_status(status: u16, retry_after: Option<Duration>) -> Error` is the status mapping (call for non-2xx only).

## Client (`client.rs`)

```rust
let client = Client::builder()
    .language("en")                       // hl, default "en"; 2..=8 letters/'-'
    .country("US")                        // gl, default "US"; exactly 2 letters
    .timeout(Duration::from_secs(20))     // default 20 s, enforced for every transport
    .max_response_bytes(8 * 1024 * 1024)  // default 8 MiB, enforced for every transport
    .transport(Arc::new(my_transport))    // optional; default is ReqwestTransport
    .build()?;                            // Result<Client, Error>; InvalidRequest on a bad locale
```

`Client` is `Clone` (shared state, cheap) and `Debug` prints `Client(..)`.

| Method | Notes |
|---|---|
| `async fn post(&self, kind: ClientKind, endpoint: &str, body: Value) -> Result<Value, Error>` | anonymous; never sends the cookie |
| `async fn post_authed(&self, kind, endpoint, body) -> Result<Value, Error>` | signed with the stored cookie; `Err(Unauthenticated)` and no request if no `SAPISID` cookie |
| `fn set_cookie(&self, cookie: &str) -> Result<(), Error>` | whole `Cookie` header string; rejects empty/control characters |
| `fn clear_session(&self)` | forget the cookie |
| `fn is_signed_in(&self) -> bool` | a cookie with a `SAPISID` (or `__Secure-3PAPISID`) value is stored |
| `fn visitor_data(&self) -> Option<String>` / `fn set_visitor_data(&self, Option<String>)` | see below |

`post` behaviour:

- `endpoint` is the path after `/youtubei/v1/`, e.g. `"search"`, `"browse"`, `"next"`, `"like/like"`. It must match `[a-z_/]+` with no `..`,
  `//`, leading or trailing `/`. Digits are not allowed. Violations give `InvalidRequest` before anything is sent.
  (`client::validate_endpoint(&str) -> Result<(), Error>` is public.)
- `body` must be a JSON object (`json!({"query": "x"})`, `json!({})`). The client inserts `"context"` into it (overwriting any `context` you set).
- URL: `https://www.youtube.com/youtubei/v1/<endpoint>?prettyPrint=false` for `ClientKind::Web`,
  `https://music.youtube.com/...` for `ClientKind::WebMusic`. HTTPS only, redirects are not followed.
- Context sent: `{"client": {"clientName", "clientVersion", "hl", "gl", "visitorData"?}, "user": {"lockedSafetyMode": false}}`.
  `Web` = `WEB`, `WebMusic` = `WEB_REMIX`. Version strings live in `client::versions` (one place to update); they are unverified against the live site.
- Headers: `Content-Type`, one fixed `User-Agent` (`versions::USER_AGENT`, never rotated), `Accept-Language`, `X-Youtube-Client-Name`,
  `X-Youtube-Client-Version`, `Origin`, and `X-Goog-Visitor-Id` when visitor data is known. `post_authed` adds `Cookie`, `Authorization`
  (`SAPISIDHASH`), `X-Origin`, `X-Goog-AuthUser: 0`.
- Visitor data: if none is set, the first successful response's `responseContext.visitorData` is stored and sent from then on.
- Non-2xx -> `Error::from_status`. Body over the limit -> `TooLarge`. Invalid JSON -> `Parse`. Timeout -> `Network`.
- The returned `Value` keeps object key order (serde_json `preserve_order`).

`ClientKind { Web, WebMusic }` (`Copy`, `Eq`, `Hash`): `fn origin(self) -> &'static str`, `fn context(self, hl, gl, visitor_data: Option<&str>) -> Value`.

If a live call ever returns a bot or login challenge, the crate does nothing special: it comes back as `Refused`/`Unauthenticated` or as JSON
the higher module must report. There is no retry, user-agent rotation or client switching.

## Session (`session.rs`)

`Session` holds the cookie; `Debug` prints `Session(..)`. `Client` owns one, so most code only calls `Client::set_cookie`.

- `Session::new()`, `set_cookie(&mut self, &str) -> Result<(), Error>`, `clear(&mut self)`, `can_sign(&self) -> bool`
- `sapisidhash(&self, kind: ClientKind, timestamp: u64) -> Option<String>`: the header value `SAPISIDHASH {ts}_{sha1hex}`,
  where the hash is SHA-1 of `"{ts} {SAPISID} {origin}"` and `origin` is `kind.origin()`.
  Test vector: SAPISID `abc123`, ts `1700000000`, Web -> `SAPISIDHASH 1700000000_9e5071f149fc514366f78b22d1a169786d40ed32`,
  WebMusic -> `..._597a8411c1c01f9c14cea84b6ec377b76d93be6e`.
- `auth_headers(&self, kind, timestamp) -> Option<Vec<(String, String)>>`: `Cookie`, `Authorization`, `Origin`, `X-Origin`, `X-Goog-AuthUser`.

## Transport (`transport.rs`)

```rust
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub trait Transport: Send + Sync + 'static {
    fn send(&self, request: Request) -> BoxFuture<'_, Result<Response, Error>>;
}
pub struct Request  { pub url: String, pub headers: Vec<(String, String)>, pub body: Vec<u8> }   // always POST; Debug hides everything
pub struct Response { pub status: u16, pub retry_after: Option<Duration>, pub body: Vec<u8> }    // non-2xx is data, not Err
```

- `ReqwestTransport::new(timeout: Duration, max_response_bytes: usize) -> Result<Self, Error>`: the default. rustls (no OpenSSL), gzip,
  HTTPS only, no redirects, streams the body and stops at the size limit. You normally never construct it; the builder does.
- `FixtureTransport`: offline stand-in, see below. Constants `DEFAULT_TIMEOUT` (20 s), `DEFAULT_MAX_RESPONSE_BYTES` (8 MiB).

### Testing a module offline with `FixtureTransport`

```rust
use innerlight::{Client, ClientKind, FixtureTransport};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn my_parser_test() {
    let fx = Arc::new(
        FixtureTransport::new()
            .with_file("search", "tests/fixtures/my_search.json")          // 200 + file contents (path relative to the crate root)
            .with_match("browse", "TOKEN2", json!({"page": 2}))             // only when the request body text contains TOKEN2
            .with("browse", json!({"page": 1}))                             // catch-all for `browse`
            .with_raw("next", 429, b"{}"),                                  // any status/body, e.g. errors, bad JSON
    );
    let client = Client::builder().transport(fx.clone()).build().unwrap();

    let v = client.post(ClientKind::WebMusic, "search", json!({"query": "x"})).await.unwrap();
    // ... run your parser on `v` ...

    let sent = &fx.requests()[0];           // Vec<Recorded>: .host, .endpoint, .body (with "context"), .header("name")
    assert_eq!(sent.endpoint, "search");
}
```

Rules: the endpoint key is the same string you pass to `post` (`"search"`, not a URL). Rules are checked in the order added; the first match
wins, so put `with_match` before the catch-all `with`. A rule with several responses answers them in order and then repeats the last (good for
continuation pages). **A request with no matching rule panics** with the endpoint name. Put recorded or hand-written JSON in
`tests/fixtures/`; name hand-written ones `handwritten_*` so nobody mistakes them for recordings. `tokio` is a dev-dependency
(`#[tokio::test]` needs features `rt`, `macros`). Use endpoint names without digits in your own tests (`validate_endpoint` applies).

## jsonx (`jsonx.rs`)

Pure, never panic; missing or odd shapes give `None` or an empty `Vec`.

| Function | Behaviour |
|---|---|
| `get(&Value, path: &str) -> Option<&Value>` | `"a.b[0].c"`, `"[1][2]"`; `""` is the root; bad path syntax or missing step is `None` |
| `text(&Value) -> Option<String>` | a string, `{"simpleText"}`, or `{"runs":[{"text"}..]}` joined (runs without text are skipped); `None` if not text or empty |
| `find_all(&Value, key: &str) -> Vec<&Value>` | every value stored under `key`, depth first, document order, also inside matches; stops below depth `DEFAULT_MAX_DEPTH` (24) |
| `find_all_depth(&Value, key, max_depth: usize) -> Vec<&Value>` | same with an explicit limit (root is depth 0) |
| `thumbnail_best(&Value) -> Option<Thumb>` | widest entry of a thumbnail list; accepts the array, `{"thumbnails":[..]}`, or `{"thumbnail":{"thumbnails":[..]}}` |

`Thumb { url: String, width: u32, height: u32 }` (`Debug, Clone, PartialEq, Eq`). Entries without a `url` are skipped, a missing width is 0,
ties go to the later entry, `//host/x` becomes `https://host/x`.

## ids (`ids.rs`, from the scaffold)

`is_video_id`, `is_channel_id`, `is_playlist_id`: `fn(&str) -> bool`. Validate any id before it goes into a request body.

## Not covered yet

Real-network behaviour (see the report that came with this commit), retries and backoff, automatic client-version refresh, cookie-jar updates
from `Set-Cookie`, the three-hash `SAPISID1PHASH`/`SAPISID3PHASH` variants, and anything about player/stream requests.
