# innerlight: `player` module (phase 4, partial)

`innerlight::player` asks the service for the audio streams of one video and picks one. It is **not** a complete playback
solution: see "What is not possible yet".

**Scope, on purpose.** One standard `POST player` with one of the two existing client kinds (`Web`, `WebMusic`), exactly as the
site's own pages send it, signed with the user's own session cookie only when the caller sets `authed`. No user-agent or
client-identity rotation, no fallback chain, no spoofed device clients, no PO-token forging, no workaround for "Sign in to confirm
you're not a bot". A login, bot or age challenge becomes a typed `PlayerError` and the call stops after that one request.
Clean-room: nothing here was read or ported from `rustypipe`.

**The fixtures are hand-authored, not recordings** (`tests/fixtures/player_handwritten_*.json`; invented hosts, ids, signatures).
They prove the code is consistent with the author's knowledge of the JSON and never panics; they do not prove the live shapes.

## API

```rust
use innerlight::player::{Player, ResolveOptions, Quality};
let client = innerlight::Client::builder().build()?;
let player = Player::new(&client);                       // borrows the client; Player::with_decipher(Arc<dyn Decipher>) optional
let r = player.resolve("dQw4w9WgXcQ", ResolveOptions {
    kind: innerlight::ClientKind::Web,                   // or WebMusic
    authed: false,                                       // true = post_authed (Unauthenticated, nothing sent, without a cookie)
    quality: Quality::High,                              // Low <= 64 kbit/s, Normal <= 128 kbit/s, High = best
    signature_timestamp: None,                           // caller-supplied; omitted from the request when None
}).await?;                                               // Result<Resolved, PlayerError>
```

`ResolveOptions::default()` is `Web`, anonymous, `High`, no timestamp.

- **Request** (`player::request_body`): `{videoId, contentCheckOk: true, racyCheckOk: true, playbackContext.contentPlaybackContext:
  {html5Preference: "HTML5_PREF_WANTS", signatureTimestamp?}}` plus the client's `context`. A bad id is `InvalidId`, nothing sent.
- **`Resolved`**: `url` (validated, never logged; `Debug` hides it), `itag`, `mime_type`, `codecs`, `bitrate`, `size`, `sample_rate`,
  `loudness_db`, `track: Option<AudioTrack>`, `expires_in` (relative to the answer), `needs_throttle_fix`.
- **`Playability`** (`player::parse::parse_playability`): `Ok`, `LoginRequired{bot_check}`, `AgeRestricted`, `Unplayable{reason:
  Region|Private|MembersOnly|Other}`, `Live`, `Unavailable`, `Error`. The bot challenge is recognised by "not a bot" in the reason
  text (curly quotes folded), whatever status carries it. `OK` with `videoDetails.isLive` is `Live`. Anything but `Ok` returns
  no stream, even if the same answer carries `streamingData`. The reason text itself is never kept.
- **`PlayerError`** (`Copy`, fixed messages, never a URL, cookie, signature or response text): `InvalidId`, `Transport(Error)`,
  `LoginRequired{bot_check}`, `AgeRestricted`, `Unplayable{reason}`, `Live`, `Unavailable`, `ServiceError`, `Parse`, `NoAudio`,
  `NeedsDecipher`, `DecipherFailed`, `UnsafeUrl`. `From<Error>` and `From<PlayerError> for Error` (lossy) exist.
- **Parsing** (`player::parse`): `parse_streaming_data` reads `adaptiveFormats` then `formats`: itag, mime/codecs, bitrate,
  averageBitrate, contentLength, audioQuality, audioSampleRate, loudnessDb, `audioTrack{id,displayName,audioIsDefault}`, DRM
  (`drmFamilies` non-empty or `drmTrackType`), and `url` or `signatureCipher`/`cipher` -> `StreamSource::Direct(url)` |
  `Ciphered{url,s,sp}` (`sp` defaults to `signature`). Entries without itag, mime or a usable url/cipher are skipped; at most 512.
- **`select_audio(&[Stream], Quality)`**: audio only, no DRM, `content_length > 0`; if any candidate is in a default track (or has no
  track info) only those compete; highest bitrate within the limit, else the lowest above it; ties prefer Opus over AAC. Same
  semantics as the desktop app's current `best_audio`. Without a `Decipher`, a ciphered pick falls back to the best *direct* stream
  (quality is chosen among directs then); no direct stream gives `NeedsDecipher`.
- **`check_stream_url`**: https only, host is `<label>.googlevideo.com`, no userinfo/port/fragment, no control characters, whitespace,
  quote or backslash, at most 16 KiB. Applied to every URL before it is returned (also to the base URL of a cipher and to
  every `Decipher` output). Returns the normalised URL.
- **`ranged_reader_plan(size, chunk)`**: lazy iterator of inclusive `ByteRange{start,end}` (`header_value()` = `bytes=a-b`). The
  media host wants ranged requests, so a downloader/reader should walk this plan. Empty for size 0 or chunk 0.
- **`Decipher`** (`decipher(&self, s) -> Result<String, PlayerError>`, `throttle_fix(&self, url) -> Result<String, PlayerError>`):
  for ciphered streams the result is appended as `&{sp}=<percent-encoded result>`; `throttle_fix` runs on every URL that has an `n`
  parameter. With no `Decipher`, a direct URL with `n` is still returned, flagged `needs_throttle_fix = true` (the server may
  serve it slowly), and a ciphered-only answer is `NeedsDecipher`.

## Live test

`tests/live_player.rs` is `#[ignore]` and does nothing unless `INNERLIGHT_LIVE=1`. **A human runs it deliberately**:

```text
INNERLIGHT_LIVE=1 INNERLIGHT_VIDEO=<public video id> [INNERLIGHT_COOKIE='<Cookie header>'] [INNERLIGHT_KIND=music] \
  cargo test --test live_player -- --ignored --nocapture
```

It makes one standard request, no retry, no client switch, and prints the typed outcome (never a URL or the cookie). `cargo test`
alone never touches the network.

## What is not possible yet

Playback produces sound end to end only when all of this exists; the first three are outside this crate or open:

1. **A way past the bot check.** A plain anonymous `player` request may be answered with "Sign in to confirm you're not a bot".
   This crate reports that (`LoginRequired{bot_check: true}`) and stops. Whether a user's own signed-in cookie
   (`authed: true`) is enough is **unverified** (no live request was made while writing this); the player may still demand
   proof-of-origin tokens that this crate does not and will not forge. Without a resolved answer there is nothing to play.
2. **Signature deciphering and `n` (throttle) fixing.** Open item. No `Decipher` implementation ships. Many answers carry
   `signatureCipher`; without a `Decipher` these resolve to `NeedsDecipher`. An implementation must: fetch the site's player
   script (the URL changes with `signatureTimestamp`'s player version), locate and execute the signature-transform and the
   `n`-transform functions in a sandboxed JS evaluation (or a faithful reimplementation), cache them per player version, survive
   script changes with clear errors (never a silent wrong URL), obey the same no-evasion scope, and return only what
   `check_stream_url` accepts. The caller must also supply a current `signatureTimestamp` taken from that same script.
3. **Client versions** in `client::versions` are unverified against the live site; a stale version can make the service answer
   differently.
4. **A ranged HTTP reader and a decoder/audio output.** This crate returns a URL and a range plan. Fetching bytes
   (ranged GETs with the same fixed user agent, refresh when `expires_in` runs out or a 403 comes back), demuxing WebM/MP4,
   decoding Opus/AAC and playing PCM are the application's job (the desktop app has them).
5. Not handled: live/HLS/DASH manifests (`Live`), DRM streams (excluded), SABR-only responses (they would arrive as no usable
   audio: `NoAudio`), retries and re-resolution on expiry.
6. All shapes and challenge texts above come from hand-written fixtures. Record real answers (the live test can be extended to
   save redacted ones) before trusting them.
