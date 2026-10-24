# innerlight

A Rust client for YouTube and YouTube Music: search, browse, library, account actions, and stream resolution.
Shared by the LunoRX desktop and mobile apps. Licensed under MIT.

> **Status: unverified against the live service.** Every parser and request body is tested only against
> hand-written fixtures. Nothing here has been checked against a real YouTube response yet. Stream playback
> may be refused by YouTube's bot check; this crate reports that as a typed error and never works around it.

## What it does

| Module | Covers |
|---|---|
| `music` | search by kind (tracks, albums, artists, playlists) and suggestions, home feed, radio, album, artist, playlist, charts, genres, new releases, lyrics, signed-in library (liked songs, saved playlists, history) |
| `youtube` | signed-in liked videos, watch later, history, saved playlists, public playlists |
| `account` | like / unlike / dislike, subscribe / unsubscribe, add / remove playlist items, create / delete playlists |
| `player` | playability check and audio stream selection; refuses to return a stream unless YouTube says it is playable |
| `decipher` | reads the public player script to derive the signature and throttling transforms, run in a sandboxed JavaScript engine |
| `provider` | catalogues that are not YouTube behind one trait: `Internet Archive` search, album and stream URL, plus `Catalog` to ask several sources at once |
| `policy` | the politeness limits one client obeys: retry with a bounded backoff, a concurrency ceiling and a request-rate ceiling |
| `client`, `transport`, `session`, `error`, `jsonx`, `ids` | HTTP transport, request context, cookie session (`SAPISIDHASH`), typed errors, lenient JSON helpers, id validation |

## Use

```rust
use innerlight::{Client, ClientKind};

let client = Client::builder().language("en").country("US").build()?;
let v = client.post(ClientKind::WebMusic, "search", serde_json::json!({ "query": "song" })).await?;
```

Signed-in calls take a cookie string that the caller supplies (`client.set_cookie(...)`). The cookie is never logged
and never sent by anonymous calls.

Every call is already polite: at most four sends in flight, two starts per second, a `429` or `5xx` waited out with a
bounded backoff. A `Retry-After` the service sent is the wait, exactly — never shortened, and if it does not fit the
call's remaining budget the client stops retrying and hands you the seconds instead of sending early.
`Policy::polite()` is the default; `.retry_attempts(1)` turns resending off and `.requests_per_second(n)` moves the ceiling.

```rust
use innerlight::provider::{Archive, Catalog, Fetcher};
use innerlight::{Policy, ReqwestTransport, TokioClock};
use std::{sync::Arc, time::Duration};

let transport = Arc::new(ReqwestTransport::new(Duration::from_secs(10), 1 << 20)?);
let archive = Archive::new(
    Fetcher::new(transport, Policy::polite(), Arc::new(TokioClock::new()), Duration::from_secs(10), 1 << 20)?,
    Archive::DEFAULT_ROWS,
)?;
let mut catalog = Catalog::new();
catalog.register(Arc::new(archive));
let hits = catalog.search_all("pile of songs", 10).await?; // items, per-source failures, duplicates dropped
```

Providers fetch anonymously through their own transport, so a third-party host never sees a YouTube cookie.

Tests need no network. Use `FixtureTransport` to answer requests from files; see `docs/API.md`.

## Limits and non-goals

What this crate does when a service pushes back: it slows down and it says why. A `429` or `5xx` or a timeout is
retried a bounded number of times with a jittered backoff, `Retry-After` is honoured, concurrency and request rate
are capped, and a challenge is reported as `Error::BotCheck` and a quota as `Error::QuotaExceeded`.

What it never does:

- No bot-check evasion: no client-identity rotation (`Android`, `iOS`, `TV` are not offered), no forged
  proof-of-origin tokens, no retry loop against a challenge. `Error::BotCheck` stops the call; signing in with your
  own cookie is the way past it.
- The `User-Agent` is one fixed value for every request and never varies.
- No DRM, no live or HLS/DASH streams, no audio decoding or playback — for YouTube or for any provider. A
  `Resolved` stream URL is handed to the caller's player and nothing here fetches it.
- Radio queues page with `Music::radio_more` when the answer carries a `Radio::continuation`; an answer without one is
  the whole queue. The position of that handle in a `next` response is not yet verified against the live site.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Everything above runs with no network: the tests answer requests from `tests/fixtures/` through `FixtureTransport`, and the
waits happen on `FakeClock`, so the suite costs no real seconds. `cargo deny check licenses` is the fourth gate and belongs
in CI; `cargo-deny` is not installed in this dev environment, so the accepted licences were verified by reading each locked
crate's own `license` field (`docs/DESIGN.md`).

Live tests are `#[ignore]`d and run only by hand with `INNERLIGHT_LIVE=1` (see `tests/live_player.rs` and `tests/decipher_live.rs`).

## Docs

- `docs/DESIGN.md`: scope and the clean-room rules
- `docs/API.md`: transport, client, politeness and error API
- `docs/MUSIC.md`, `docs/YOUTUBE.md`, `docs/PLAYER.md`, `docs/DECIPHER.md`: module APIs and the list of wire shapes still unverified
- `docs/PROVIDERS.md`: the non-YouTube catalogues, how they are merged, and which sources are refused on purpose

## Licence

MIT. See `LICENSE`.
