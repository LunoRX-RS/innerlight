# innerlight

A Rust client for YouTube and YouTube Music: search, browse, library, account actions, and stream resolution.
Shared by the LunoRX desktop and mobile apps. Licensed under MIT.

> **Status: unverified against the live service.** Every parser and request body is tested only against
> hand-written fixtures. Nothing here has been checked against a real YouTube response yet. Stream playback
> may be refused by YouTube's bot check; this crate reports that as a typed error and never works around it.

## What it does

| Module | Covers |
|---|---|
| `music` | search and suggestions, radio, album, artist, playlist, charts, genres, new releases, lyrics, signed-in library (liked songs, saved playlists, history) |
| `youtube` | signed-in liked videos, watch later, history, saved playlists, public playlists |
| `account` | like / unlike / dislike, subscribe / unsubscribe, add / remove playlist items, create / delete playlists |
| `player` | playability check and audio stream selection; refuses to return a stream unless YouTube says it is playable |
| `decipher` | reads the public player script to derive the signature and throttling transforms, run in a sandboxed JavaScript engine |
| `client`, `transport`, `session`, `error`, `jsonx`, `ids` | HTTP transport, request context, cookie session (`SAPISIDHASH`), typed errors, lenient JSON helpers, id validation |

## Use

```rust
use innerlight::{Client, ClientKind};

let client = Client::builder().language("en").country("US").build()?;
let v = client.post(ClientKind::WebMusic, "search", serde_json::json!({ "query": "song" })).await?;
```

Signed-in calls take a cookie string that the caller supplies (`client.set_cookie(...)`). The cookie is never logged
and never sent by anonymous calls.

Tests need no network. Use `FixtureTransport` to answer requests from files; see `docs/API.md`.

## Limits and non-goals

- No bot-check evasion: no client-identity rotation, no forged proof-of-origin tokens, no retry loops against a challenge.
- No DRM, no live or HLS/DASH streams, no audio decoding or playback. The caller hands the resolved URL to a player.
- Radio has no continuation token; a caller that needs more must re-seed.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Live tests are `#[ignore]`d and run only by hand with `INNERLIGHT_LIVE=1` (see `tests/live_player.rs` and `tests/decipher_live.rs`).

## Docs

- `docs/DESIGN.md`: scope and the clean-room rules
- `docs/API.md`: transport and client API
- `docs/MUSIC.md`, `docs/YOUTUBE.md`, `docs/PLAYER.md`, `docs/DECIPHER.md`: module APIs and the list of wire shapes still unverified

## Licence

MIT. See `LICENSE`.
