# lunorx-youtube: design

Shared YouTube / YouTube Music client for LunoRX desktop and mobile. MIT. No UI, no Tauri, no audio output.

## Clean-room rule (why this repo exists)

`rustypipe` is GPL-3.0 and is what forces the desktop app to be GPL-3.0. This crate replaces it so the apps can return to MIT.
That only works if nothing here is copied or mechanically translated from rustypipe.

- Nobody who writes this crate copies from `desktop/src-tauri/vendor/rustypipe`. Do not paste from it, port it function by function, or keep it open while writing.
- Behaviour comes from observing the service's own responses (saved samples in `tests/fixtures/`, recorded by us) and from public documentation.
- If a design choice looks like rustypipe's, write down in the commit why it follows from the service and not from the source.
- Dependencies must be MIT, Apache-2.0, BSD or similar. `cargo deny` should run in CI.

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

## Open problem: playback

YouTube answers a plain player request with "Sign in to confirm you're not a bot". Resolving a stream therefore needs a way to pass the
service's bot check. rustypipe's upstream keeps up with this; a rewrite takes that work over. The options must be decided before phase 4
(see the notes in the conversation that created this repo). Nothing in this crate may pretend a stream resolved when it did not.
