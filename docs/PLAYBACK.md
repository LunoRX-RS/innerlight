# innerlight: playback decision record

Required by `docs/DESIGN.md` §8 and the PO Token section before any further playback work.

## Decision

Playback stays **partial**. The crate sends one ordinary `player` request per call (`docs/PLAYER.md`), signed with the
caller's own cookie only when asked, and unscrambles `signatureCipher` / `n` with the public script (`docs/DECIPHER.md`).
It does **not** obtain, generate or forge a PO Token, rotate or impersonate clients, or retry past a challenge.

| Outcome | Status |
|---|---|
| Stream resolved | `Resolved` |
| Login or bot challenge | typed `PlayerError`, exactly one request, no wait |
| Token required but unavailable | reported as the challenge it appears as; no separate "token" variant, because the service response does not name one |
| Authorised token source | `player::PoTokenSource`, implemented by the host app (e.g. a token read from the user's own real browser session). This crate never generates, stores or logs a token, sends it on the one `player` request only, and still stops at a challenge. Live check 2026-10-28 without a token: `UNPLAYABLE` / "The page needs to be reloaded." |

## Open questions (not decided)

- Whether an officially supported interface exists that meets the project's terms. Not evaluated.
- Whether script-based deciphering is acceptable long term. It runs in the `boa_engine` sandbox with time limits, but §9 asks
  for its own security review, which has not been done.

## Verification status

- `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`: run locally.
- `cargo deny check`: **not run** (`cargo-deny` is not installed here). `deny.toml`'s allow-list is a first guess from the
  dependency tree, not a verified result. The CI `deny` job is the first real check.
