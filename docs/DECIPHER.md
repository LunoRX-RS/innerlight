# innerlight: `decipher` module

`innerlight::decipher` is the real `player::Decipher`: it unscrambles `signatureCipher` values and rewrites the throttling
parameter `n` of stream URLs, by running the site's own public player script in a sandbox.

**Scope, on purpose.** Signature-cipher and throttle-parameter transform only. It does not call the `player` endpoint, does not
touch "Sign in to confirm you're not a bot", does not rotate user agents or client identities and does not make or forge
proof-of-origin tokens. Network use is two anonymous `GET`s: the public loader (`https://www.youtube.com/iframe_api`, only to
learn the current player version) and the public script (`https://www.youtube.com/s/player/<version>/player_ias.vflset/en_US/base.js`).
No cookie or account header is ever sent on a `GET`. Clean-room: written from the public script itself and general JavaScript
knowledge; nothing was read or ported from `rustypipe`.

## API

```rust
use innerlight::decipher::{PlayerScript, ScriptCache, ScriptDecipher};
use innerlight::player::{Player, ResolveOptions};

let cache = ScriptCache::new();                         // per-version cache (Arc), rechecks the loader hourly
let d = cache.get(&client).await?;                      // Arc<ScriptDecipher>: fetch + analyse once per version
let opts = ResolveOptions { signature_timestamp: Some(d.signature_timestamp()?), ..Default::default() };
match Player::new(&client).with_decipher(d).resolve(id, opts).await {
    Err(PlayerError::DecipherFailed) => cache.invalidate(),   // script moved on: the next get() refetches
    other => { /* ... */ }
}
```

- `PlayerScript::fetch_current(&Client)` (loader, then script; both validated: host `www.youtube.com`/`s.ytimg.com`, path
  `/s/player/<version>/...js`, UTF-8, non-empty, at most `MAX_SCRIPT_BYTES` = 4 MiB), `fetch_version`, `current_version`, and
  offline `PlayerScript::from_source(version, js)`. `signature_timestamp()` reads `signatureTimestamp:<n>` (also quoted /
  `=`), falling back to a bare `sts:<5 digits>`.
- `ScriptDecipher::new(&PlayerScript)` finds and **verifies** both transforms or returns `ScriptShapeChanged { which }`.
  `signature(s)`, `throttle_value(n)`, `throttle_url(url)` return `DecipherError`; the `Decipher` impl maps it to
  `PlayerError` (`DecipherFailed`, `UnsafeUrl`, transport errors pass through). `found_by()` names the heuristics that matched.
  `Send + Sync`; each transform has its own sandbox thread and concurrent calls are serialised.
- `ScriptCache`: `get(&client)`, `invalidate()`, `cached_version()`, `with_recheck_interval`, `with_limits`. Within the interval
  no request is made; after it one loader `GET` confirms the version and the script is downloaded again only if the version
  changed. `invalidate()` (call it on `DecipherFailed`) forces a full refetch and re-analysis.
- `ScriptDecipher::new` is blocking CPU work (about one second in a debug build for a 3 MB script: lexing dominates). Call it from
  a blocking context if the executor has latency-sensitive tasks. `Client::get_static` / `Transport::get` are the additive GET
  support; `Transport::get` defaults to `Err(Refused)` so other transports keep compiling.
- Outputs are checked: the signature is non-empty, no longer than the input and made only of the input's characters; the `n`
  value is non-empty, at most 1024 characters of `[A-Za-z0-9_.~-]`, not equal to the input and not the script's failure fallback
  (`<marker>` + input). `throttle_url` rewrites only the `n=` segment (other parameters keep their exact bytes and order), returns
  a URL without `n` unchanged, and every result passes `player::check_stream_url`.
- Errors (`DecipherError`, `Copy`, fixed messages, never script text or values): `Transport`, `BadScript`,
  `ScriptShapeChanged{which: Signature|Throttle|SignatureTimestamp|Version}`, `SandboxTimeout`, `SandboxBudget`, `SandboxFailed`,
  `BadInput`, `BadOutput`, `UnsafeUrl`. A transform that cannot be found, or does not behave, is **never** replaced by the input.

## How the transforms are found (no hard-coded names)

`lexer.rs` tokenizes the script (strings, regexes, comments, brackets). `index.rs` splits the wrapper into named top-level
**units** (`name=function..`, `var name=..`, `function name(){}`, `g.name=..`, comma-joined definitions split) and can build a
**closure program**: chosen root names plus everything they refer to within N hops, in script order, each unit in its own
`try{}`, evaluated in strict mode. Only that program is ever executed, never the whole script (for the saved script: 30 units /
9 KB for the signature, 105 units / 37 KB for `n`).

`locate.rs` only **proposes** candidates `(roots, call template)`; `plan.rs` loads the closure at depth 0, 1, 2, ... into a
sandbox and **probes** each candidate. The first candidate that behaves like the transform wins:

- Signature probe: 94 distinct characters in; the output must be non-empty, shorter or equal, made of distinct input characters,
  different from the input, deterministic, and **one fixed position mapping** (the same mapping applied to the reversed input
  must give the predicted result). Splice operations drop characters, so the length is *not* preserved in general (the saved
  script maps 100 characters to 94).
- `n` probe: two inputs; each output must pass the `n` checks above, be deterministic, and the two outputs must differ.

Heuristics today (a candidate may come from any of them):

| Target | Heuristic | Looks for |
|---|---|---|
| signature | `helper-function`, `helper-dispatcher` | an object literal of 2 to 6 members that each reverse, splice (`a.splice(0,b)`, also `a[expr](0,b)`) or swap (`a[0]` with `a[b%a.length]`) their first argument, at least two kinds; then every unit using that object at least twice, called as `F(__a)`, or as `F(A,B,__a)` for each `F(NUM,NUM,` call site (multiplexed dispatcher) |
| signature | `decode-call-site` | `F(decodeURIComponent(` / `F[N](decodeURIComponent(` |
| signature | `sp-call-site` | after a `.sp` read, the call `F(A,B,G(C,D,` |
| `n` | `marker-in-function` | a function unit holding the failure-marker string (`enhanced_except_..._`, or `..._w8_`) |
| `n` | `marker-in-string-table` | the marker inside a `T="a;b;c".split(";")` table and a read `T[e^K]+x`: `e = K ^ index(marker)`, then `D(A, A^e, __a)` for every first argument `A` seen at `D(` call sites. When the read sits in a `catch` block (the failure path), every selector `A` in `0..64` is proposed too, after the ones the script uses: the player of 2026-10-30 reaches the `n` branch only through computed arguments, so no call site names the selector |
| `n` | `get-n-call-site` | `.get("n")` / `.get(String.fromCharCode(110))` followed by `F(x)` or `F[N](x)` |
| `n` | `split-join-try-shape` | the six biggest function units containing `split`, `join`, `try`, `catch` |
| `n` | `brute-force` (last resort) | dispatchers shaped `function(a,b,..){var e=b^a;..try..}`: every selector seen at call sites x `b` in `0..4096`, 20 s budget |

Two shapes of script exist and both are covered. **Classic**: `NAME=function(a){a=a.split("");H.xx(a,3);...;return a.join("")}`
with helper `H`. **Multiplexed** (the saved 2026 script): one dispatcher `D(a,b,c,d)` computes `e=b^a`, picks a branch by `a`, and
reaches the helper and a string table through `T[e^K]` lookups, with the signature as `D(16,1453,s)` and `n` as `D(1,<b>,n)`.

## What the saved real script proved and did not prove

The real player script (player `cbcfbf2f`, `signatureTimestamp` 20746, fetched 2026-10-23) was used for these checks. It is
not committed (Google owns it). Point `INNERLIGHT_PLAYER_JS` at a copy to rerun `decipher_real` and the real-script fuzz test. Proved: both structures are located by heuristics (`helper-dispatcher`, `marker-in-string-table`), the closure
loads in the pure-Rust engine, the signature is a stable position mapping of the input (drops characters, never adds),
and the `n` transform returns a deterministic, changed, valid value for every input tried without taking the script's failure
path. **Not proved:** that the service accepts a URL deciphered this way. That needs a live `player` answer, which this crate
does not obtain (see `PLAYER.md`, bot check). Also not proved: output equality with a browser engine; the script is run by
`boa_engine`, whose `Date`, regex and numeric edge cases may differ from V8. A human can run the `#[ignore]`d live test:

```text
INNERLIGHT_LIVE=1 cargo test --test decipher_live -- --ignored --nocapture
```

## Sandbox

`boa_engine` (MIT or Unlicense, pure Rust, no `boa_runtime`): the context has **no host functions** (`fetch`, `require`,
`process`, `console`, timers, `XMLHttpRequest`, ... are all undefined), runs on its own thread, and is reached only through a
channel. The argument is bound to a global (`__a`) as data, never spliced into code. Limits (`SandboxLimits`): 250 ms wall clock
per call (a stuck context is abandoned and rebuilt from the program; its thread ends when the engine's loop budget runs out),
3 000 000 loop iterations per call frame, call depth 256, results at most 8192 UTF-16 units. `String#repeat`, `padStart`, `padEnd`
and `Array#join` are capped at 1 Mi so one call cannot allocate without bound. **Boa has no memory ceiling**; those caps are
best effort, not a guarantee. Boa also does not give back every stack slot of a call that ends in a thrown error, so after 8
failed calls a sandbox rebuilds its context from the program before the next call (probing tries hundreds of wrong candidates;
without this, a correct candidate late in the list fails with "maximum stack size"). Tests: `tests/decipher_sandbox.rs`.

A multiplexed dispatcher can hold several guarded branches that run one after another (`if(guard1){..}guard2&&(..);if(guard3){n branch}`).
A wrong selector then runs a neighbouring branch and throws on an undefined argument; the right one satisfies only its own guard
(the 2026-10-30 player: selector 27 to 31). The probe tells them apart by behaviour, never by reading the guards.

## What breaks when the script changes

The player is replaced every few weeks and its obfuscation is regenerated. Expect, in rising order of pain:

1. **Constants change** (indices, selectors, the marker string): handled, nothing is hard-coded.
2. **Renamed identifiers**: handled.
3. **A heuristic misses** (new helper member shape, marker no longer a `_w8_` string, call site rewritten): `ScriptDecipher::new`
   returns `ScriptShapeChanged { which }`. `brute-force` may still rescue `n`. Add a heuristic (below).
4. **A candidate is found but the probe rejects it** (the code now throws in the sandbox because of an environment check, a
   dependency the closure does not reach within `MAX_DEPTH` = 12 hops or 3 MiB, or a language feature the engine lacks):
   also `ScriptShapeChanged`. Run with `INNERLIGHT_DECIPHER_DEBUG=1` to print each depth tried and the JS error behind every
   rejected candidate.
5. **Output looks right but the service rejects it**: nothing in this crate can see that. The caller sees a 403 on the media
   request; the application should `invalidate()` and retry once.

## Adding a heuristic

1. Save the new script as a fixture and run `INNERLIGHT_DECIPHER_DEBUG=1 cargo test --test decipher_real -- --nocapture`.
2. Write a function in `src/decipher/locate.rs` returning `Vec<Candidate>` (`roots` = unit names to load, `template` = call
   expression with `__a` for the argument, `how` = a short label) and call it from `signature_candidates` or
   `throttle_candidates`. Use the `Index` helpers (`is`, `text`, `close_of`, `unit_at`, `defs`); guard every token index with
   `idx.is(..)` rather than `idx.text(..)` (the tokens may end early).
3. Do not weaken the probes in `plan.rs`; they are what makes "fail loudly" true.
4. Add a hand-authored mini script of the new shape to `tests/decipher_common/mod.rs` with a Rust reference implementation, a
   test in `tests/decipher_synthetic.rs`, and add the new real script as a fixture test.
