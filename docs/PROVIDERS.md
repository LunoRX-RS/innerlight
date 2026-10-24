# innerlight: providers (sources that are not YouTube)

`innerlight::provider`: a plug-in seam for catalogues outside YouTube, plus the one source implemented today (Internet Archive).
Read-only metadata and a URL to hand to the app's own player. Nothing here decodes audio, and nothing here talks to YouTube.

**Fixtures are hand-authored.** `tests/fixtures/archive_handwritten_*.json` were written from the documented shape of the endpoints, with
real-looking but invented identifiers. They prove the parsers read that shape and never panic; they do not prove archive.org still answers
like this. Add a recording before relying on anything under "Unverified" at the end.

## Why the types are separate from `music::model`

`music::model` validates id *shapes*: `MPREb_…` for an album, `UC…` for a channel, an 11-character video id. Those checks are an invariant,
not cosmetics. A non-YouTube source either has to fake ids in that shape or the checks have to be weakened, and both make the music model
untrustworthy. So `provider::model` has its own `Track`, `AlbumSummary`, `Album`, `Page<T>`, `Item` and `Resolved`, where `id` is opaque and
only meaningful to the provider that produced it. `Thumbnail` is shared, because a `{url, width, height}` triple is not YouTube-specific.

`Item` is `Track | Album` (a music search mixes four kinds; a catalogue search mixes these two) and serializes in serde's default externally
tagged form, `{"Album": {…}}`.

Fields the source did not state stay `None`: `license`, `attribution`, `duration_ms`, `year`. They are never inferred from a guess.

## The trait

```rust
pub trait Provider: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn search<'a>(&'a self, query: &'a str, continuation: Option<&'a str>) -> BoxFuture<'a, Result<Page<Item>, Error>>;
    fn album<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Album, Error>>;
    fn resolve<'a>(&'a self, track_id: &'a str) -> BoxFuture<'a, Result<Resolved, Error>>;
}
```

Object-safe like `Transport`, so providers live in an `Arc<dyn Provider>`. Rules a provider must keep:

- Return `crate::Error`, not a second error enum. An id it does not recognise is `InvalidId`; a response it cannot read is `Parse`;
  a rejected argument is `InvalidRequest`.
- Validate before fetching: a bad query or id gives an error with **zero requests sent**, which is asserted per provider.
- Be cancel-safe, like `Transport`.
- `continuation` is opaque. A provider may encode a page number in it; the caller treats it as a black box and only passes it back.

## `Fetcher`: the only way a provider reaches the network

```rust
let fetcher = Fetcher::new(
    Arc::new(ReqwestTransport::new(Duration::from_secs(10), 1 << 20)?),  // the provider's own transport
    Policy::polite(),
    Arc::new(TokioClock::new()),
    Duration::from_secs(10),
    1 << 20,
)?;
```

`get(url)` / `json(url)` are anonymous https GETs, and `Fetcher::new` refuses a `Policy` that does not validate, a zero timeout, or a zero
size cap. What that buys:

- **No cookie, ever.** The provider holds its own `Arc<dyn Transport>`; it never sees a `Client`, so the YouTube session cookie cannot leak
  to a third-party host. Requests carry no `Authorization` and no `Cookie`.
- **Same politeness as YouTube traffic.** The `Gate` from `docs/API.md` serialises and paces the fetches, honours `Retry-After`, and retries
  only 429 / 5xx / timeout / connect failure. A challenge body is named (`BotCheck`, `QuotaExceeded`) and never retried.
- **Bounded.** `check_url` rejects anything but a plain `https://host/path` URL: no `http`, no userinfo, no port, no fragment, no
  host-less name, at most 2048 characters. Bodies over `max_bytes` are `TooLarge`, and a non-JSON answer is `Parse`.
- **Not YouTube's budget.** Third-party traffic is not mixed into the `Client`'s gate, so a slow archive.org cannot starve InnerTube calls.

There is deliberately **no host allowlist** here: `Client::get_static` is YouTube-only on purpose, and which third-party hosts are trusted is
the application's choice when it registers a provider. `provider::http::encode` percent-encodes a whole query term so a user string can
neither close a parameter nor become a path segment.

## `Catalog`: several sources, one list

```rust
let mut catalog = Catalog::new();
catalog.register(Arc::new(Archive::with_fetcher(fetcher.clone())));
let round = catalog.search_all("midset live", 10).await?;
// round.items, round.failures: Vec<Failure { source, error }>, round.duplicates
```

The merge is stated in full, because a hidden ranking would be an opinion the caller cannot audit:

1. Ask every provider at once (`join_all`), first page each, cap each answer at `per_provider` (1..=100) *before* merging.
2. Round-robin by rank: hit 1 of provider 1, hit 1 of provider 2, … then hit 2 of each, and so on.
3. Drop an item whose **lowercased, trimmed title plus first credit** already appeared in this round. The first copy in that order wins.
   `duplicates` counts what was dropped. No fuzziness, no scoring. Registration order is the only preference, and registering is the caller.

Partial failure is data, not error: a provider that failed is listed in `failures` and the others still come back. `Err` only when *every*
provider failed (the first failure's error), so a total outage is never mistaken for a search that found nothing. `per_provider == 0`, a value
over 100, an empty catalog, or an empty/control-character/over-512-byte query is `InvalidRequest` before anything is asked.

## Internet Archive (`Archive`)

The first source, chosen because it is public, needs no key or account, documents its own endpoints, and its audio is mostly CC or public
domain. `Archive::new(fetcher, rows)` (rows 1..=50, `Archive::DEFAULT_ROWS` = 40) or `Archive::with_fetcher(fetcher)`.

| Step | Request | Read |
|---|---|---|
| `search` | `GET https://archive.org/advancedsearch.php?q=<encoded>&fl[]=identifier&fl[]=title&fl[]=creator&fl[]=year&fl[]=licenseurl&rows=<rows>&page=<n>&output=json` (the brackets go out percent-encoded as `fl%5B%5D=`) | `response.docs[]` → `Item::Album` |
| `album` | `GET https://archive.org/metadata/<identifier>` | `metadata` block for the summary, `files[]` for the audio rows |
| `resolve` | `GET https://archive.org/metadata/<identifier>` (the file list is re-read, nothing is downloaded) | `files[]` row by exact name |
| the answer | `https://archive.org/download/<identifier>/<file>` | returned as `Resolved::url`, never fetched by this crate |

- Ids: an album id is the archive identifier; a track id is `<identifier>/<file>` and its parts are validated separately. An identifier must
  be one path segment — no `/`, no `..`, no `:`, no backslash, no control characters, no whitespace, no leading or trailing dot, ≤200 bytes —
  and a file name must be an audio extension this crate offers (`.mp3`, `.ogg`, `.flac`), trimmed, ≤200 bytes. Everything is percent-encoded
  when the URL is built, so a hostile name cannot change the URL's shape; `tests/provider_fuzz.rs` asserts the result still passes `check_url`.
- `search` hits are albums, because one archive item *is* a recording, not a row in a catalogue. `continuation` is the next page number as a
  decimal string, and it is `None` when the answer's `start + docs.len()` already reached `numFound` or the page ceiling (10 000) was hit.
- Durations are read from `length`, which the site sometimes gives as decimal seconds (`1831.42`) and sometimes as `mm:ss` / `hh:mm:ss`;
  anything else is `None`. Titles, creators and years may arrive as a string, a list, or `{"type":…,"value":…}` — all three are accepted.
- `license` comes from `licenseurl` and `attribution` is the `https://archive.org/details/<identifier>` page. A row with no licence says `None`;
  the crate does not assume public domain just because the item is on archive.org.
- Cover art is `https://archive.org/services/img/<identifier>` with `width`/`height` 0 (unknown), the convention `Thumbnail` uses for "no size given".
- A file row that is not one of the three audio types (`tif`, `pdf`, torrent…) is skipped, not offered.
- `album` hands back the whole file list at once; `Album::tracks.continuation` is always `None`, because a metadata answer is not paged.

## Sources deliberately not implemented

- **Bandcamp: refused.** It publishes no API, so a provider could only scrape HTML or reverse-engineer private endpoints. That is scraping, it
  breaks without notice, and it is not what this crate is for. A future version would need an actual authorized partnership, not a parser.
- **SoundCloud: deferred, not refused.** Its API is real but needs a `client_id` issued to *your* application, which the caller would have to
  supply — the crate cannot and will not borrow someone else's key. The `Provider` trait already fits it; what is unsettled is stream
  resolution there, so nobody has wired one up.
- **Anything whose access control says no.** No source gets a bot check, login wall or rate limit worked around by pretending to be a
  different client. If a service pushes back, `innerlight` names it (`BotCheck`, `QuotaExceeded`, `RateLimited`) and stops. The non-goals in
  `README.md` and `docs/DESIGN.md` apply to third-party hosts exactly as they apply to YouTube.

## Unverified against the live service

1. `advancedsearch.php` field names and the `docs[]` / `numFound` / `start` envelope, including whether `fl[]=licenseurl` is answered as a
   string or a list.
2. Whether searching the audio collections needs an explicit `format:` or `mediatype:` filter in `q`. The provider passes the query through
   unchanged, so a hit-list that includes scans and books is the expected live behaviour, and rows without an audio file simply produce no tracks.
3. `metadata/<identifier>`: that `files[]` entries carry `name`, `length`, `title`, `artist`, and that `metadata` holds `identifier`, `title`,
   `creator`, `year`, `description`, `licenseurl`. Real answers carry far more keys; extra keys are ignored.
4. `services/img/<identifier>` as a cover URL, and that the `download/` URL serves the file without a redirect this client will not follow.
5. Pagination: that `page` is 1-based and that an over-last page returns empty `docs` rather than an error.

## Tests

- `tests/provider_archive.rs`: search → metadata → resolve end to end against `FixtureTransport`, the 429 path (waits the seconds the
  service asked for, exactly, then sends once more), the 403-with-challenge path (**exactly one request, zero waiting**), 5xx then 200,
  `TooLarge`, `Parse`, `Refused` for 404, `InvalidId` before any request, and empty GET headers.
- `tests/provider_fuzz.rs`: every archive parser on mutated and damaged documents, plus hostile identifiers and file names — nothing panics,
  nothing escapes the URL shape.
- `src/provider/catalog.rs`, `http.rs`, `archive.rs` unit tests: merge order and dedup, one failing provider not sinking the round, URL checks.
- Stub providers in the catalog tests are the pattern for testing an app's registration code offline.
