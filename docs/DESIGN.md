# innerlight: design

Shared YouTube / YouTube Music client for LunoRX desktop and mobile. MIT. No UI, no Tauri, no audio output.

## Clean-room rule (why this repo exists)

`rustypipe` is GPL-3.0 and is what forces the desktop app to be GPL-3.0. This crate replaces it so the apps can return to MIT.
That only works if nothing here is copied or mechanically translated from rustypipe.

- Nobody who writes this crate copies from `desktop/src-tauri/vendor/rustypipe`. Do not paste from it, port it function by function, or keep it open while writing.
- Behaviour comes from observing the service's own responses (saved samples in `tests/fixtures/`, recorded by us) and from public documentation.
- If a design choice looks like rustypipe's, write down in the commit why it follows from the service and not from the source.
- Dependencies must be MIT, Apache-2.0, BSD or similar. `cargo deny` should run in CI. The accepted set today is: `reqwest` (features `rustls`,
  `gzip`; its re-exported `Url` type is what parses URLs, there is no separate `url` dependency), `serde`, `serde_json` (`preserve_order`),
  `thiserror`, `sha1`, `tokio` (features `time`, `sync`, plus `rt` and `macros` for tests), `futures-util` (default features off, `std` only)
  and `boa_engine` (default features off, `annex-b`) for the sandboxed deciphering. Adding one needs a licence check, not just a reason.
  `cargo-deny` is not installed in the current dev environment, so that check has not been run as a command; the accepted set above was read
  from each locked crate's own `license` field instead (`tokio` MIT, `futures-util` / `reqwest` / `serde` / `serde_json` / `sha1` / `thiserror`
  MIT OR Apache-2.0, `boa_engine` Unlicense OR MIT). Wire `cargo deny check licenses` up in CI and keep it green there.
  `tokio/sync` and `futures-util` came in with the politeness gate and the provider fan-out; neither adds a new kind of capability, one only
  waits and the other only runs several offline-safe requests concurrently.

## What the apps use today (the target surface)

Music: search tracks, search suggestions, track details / radio, album, artist, playlist, charts, genres, new albums, new videos,
lyrics, and the signed-in library (saved tracks, saved playlists, history).
YouTube: playlist, liked videos, watch later, history, saved playlists.
Account: session cookie set / remove, like / unlike (`like/like`, `like/removelike`).
Playback: resolve a playable audio stream for a video id.

## Phases

1. Transport and plumbing: client context, visitor data, request signing for the cookie session, timeouts, typed errors, fixture-based tests.
2. Read API, anonymous: search, suggestions, details, album, artist, playlist, charts, genres, lyrics.
3. Read API, signed in: library, history, liked videos, watch later, like / unlike.
4. Playback: stream resolution. See "Open problem".
5. Switch desktop to this crate, delete `vendor/rustypipe`, relicense the app to MIT, ship mobile on the same crate.

## Being polite is in scope; being evasive is not

The crate may reduce how often the service pushes back, but only by behaving well: honouring `Retry-After`, waiting with a capped backoff on
429 / 5xx / timeout / connect failure, capping concurrency and requests per second, and naming a challenge as a typed error instead of
guessing at it. See `src/policy.rs` and `Policy::polite`, which is the default.

The crate may not try to look like something it is not. No client-identity rotation (Android, iOS, TV), no forged proof-of-origin token, no
retry or client-switch aimed at a bot or login challenge, one fixed `User-Agent`. If the project ever decides otherwise, that is a policy
decision recorded here first, not a code change slipped into a feature branch. The distinction is testable and tested: a 403 carrying
challenge wording produces exactly one request and zero waiting.

The politeness work and the provider layer below are additions to phase 1 and phase 5, not new phases and not a relaxation of the clean-room
rule: both are written from the services' own public documentation and their own responses.

## More than YouTube

`src/provider/` is a plug-in seam for sources that are not YouTube. A provider owns its own `Arc<dyn Transport>`, so the YouTube session
cookie can never reach a third-party host, and it gets the same `Policy` / `Gate` / `Clock` treatment as the YouTube client. The first one is
Internet Archive (public, no key, mostly CC or public domain, and it documents its own search and metadata endpoints). Bandcamp is not
planned: it publishes no API, so reaching its data means scraping. SoundCloud is deferred rather than refused: its API needs an app-issued
`client_id` that belongs to the caller, and stream resolution there is not settled. `docs/PROVIDERS.md` holds the rules.

## Open problem: playback

YouTube answers a plain player request with "Sign in to confirm you're not a bot". Resolving a stream therefore needs a way to pass the
service's bot check. rustypipe's upstream keeps up with this; a rewrite takes that work over. The options must be decided before phase 4
(see the notes in the conversation that created this repo). Nothing in this crate may pretend a stream resolved when it did not.
