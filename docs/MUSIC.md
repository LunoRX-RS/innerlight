# innerlight: music module (phase 2 + signed-in music reads)

`innerlight::music`: YouTube Music (`music.youtube.com`, client `WEB_REMIX`). Read-only. No like/unlike (the `account` module owns
that), no playback. Everything returns `Result<_, innerlight::Error>`; network goes only through `Client::post` / `Client::post_authed`.

**The fixtures are hand-authored, not recordings.** `tests/fixtures/music_handwritten_*.json` were written from the author's knowledge
of the site's JSON, with invented titles, ids and URLs (`example.invalid`). They prove the parsers are consistent with that
knowledge and never panic; they do not prove the live site still looks like this. Replace or add real recordings (named without
`handwritten_`) before relying on any shape listed under "Unverified" below.

Clean-room: nothing here was read from or ported from `rustypipe`; model types and parsing strategy are this crate's own.
This crate does not defeat bot detection: one fixed user agent, no client rotation, no workaround for sign-in or bot challenges.
If the site answers with a challenge, you get `Refused` / `Unauthenticated` / `Parse`, nothing more.

## Handle

```rust
let client = innerlight::Client::builder().build()?;
let music = innerlight::music::Music::new(&client);   // Copy, borrows the client
```

Layout: `music::model` (types, re-exported in `music`), `music::ids` (validators), `music::parse` (pure parsers),
`music::Music` (network methods). Ids and tokens are validated before a request is built: a bad one gives `Error::InvalidId`
(text queries give `Error::InvalidRequest`) and nothing is sent.

## Methods

| Method | Endpoint / body | Returns |
|---|---|---|
| `search_tracks(query)` | `search` `{query, params: <songs filter>}` | `Page<Track>` |
| `search_tracks_more(token)` | `search` `{continuation}` | `Page<Track>` |
| `search_suggestions(input)` | `music/get_search_suggestions` `{input}` | `Vec<String>` |
| `radio(video_id)` | `next` `{videoId, playlistId: "RDAMVM<id>", isAudioOnly, ...}` | `Radio` (queue + lyrics/related browse ids) |
| `album(browse_id)` | `browse` `{browseId: "MPREb_..."}` | `Album` |
| `artist(channel_id)` | `browse` `{browseId: "UC..."}` | `Artist` |
| `playlist(id)` | `browse` `{browseId: "VL<id>"}` (a leading `VL` on the argument is stripped) | `Playlist` |
| `playlist_more(token)` | `browse` `{continuation}` | `Page<Track>` |
| `charts(country: Option<&str>)` | `browse` `FEmusic_charts` (+ `formData.selectedValues` for a country; `"ZZ"` = global) | `Vec<Section>` |
| `genres()` | `browse` `FEmusic_moods_and_genres` | `Vec<GenreGroup>` |
| `genre_playlists(params)` | `browse` `FEmusic_moods_and_genres_category` + `params` | `Vec<Section>` |
| `new_albums()` / `new_videos()` | `browse` `FEmusic_new_releases_albums` / `_videos` | `Vec<AlbumSummary>` / `Vec<Track>` |
| `lyrics(browse_id)` | `browse` `{browseId: "MPLYt_..."}` (id from `Radio::lyrics_browse_id`) | `Lyrics` |
| `saved_tracks()` / `_more(token)` | authed `browse` `FEmusic_liked_videos` | `Page<Track>` |
| `saved_playlists()` / `_more(token)` | authed `browse` `FEmusic_liked_playlists` | `Page<PlaylistSummary>` |
| `history()` / `_more(token)` | authed `browse` `FEmusic_history` | `Page<Track>` (groups like "Today" are flattened) |

Library methods use `Client::post_authed`: without a `SAPISID` cookie they return `Error::Unauthenticated` and send nothing.
All other methods are anonymous and never send the cookie even if one is stored.

Continuation handles are opaque strings (`Page::continuation`, `Playlist::tracks.continuation`); `None` means last page. A handle
only works with the matching `*_more` method.

`music::parse` exposes the pure parsers (`search_tracks`, `track_page`, `search_suggestions`, `radio`, `lyrics`, `album(v, id)`,
`artist(v, id)`, `playlist(v, id)`, `charts`, `genres`, `genre_playlists`, `new_albums`, `new_videos`, `library_tracks`,
`library_playlists`), all `fn(&Value, ..) -> Result<_, Error>`. Use them to test against your own recordings.

## Model (`music::model`)

- `Thumbnail {url, width, height}` (0 = unknown), `ArtistRef {id: Option<String>, name}`, `AlbumRef {id: Option<String>, name}`.
  Ids are validated: `ArtistRef::id` is a `UC...` channel id, `AlbumRef::id` an `MPREb_...` id, otherwise `None`.
- `Track {id (video id), title, artists, album, duration_ms, thumbnails, is_explicit, kind}`; `TrackKind {Audio, Video, Unknown}`.
- `AlbumSummary`, `Album {tracks, track_count, duration_ms, audio_playlist_id, ...}`, `AlbumKind {Album, Single, Ep, Unknown}`.
- `ArtistSummary`, `Artist {top_songs, top_songs_playlist_id, albums, singles, related, subscribers (text), ...}`.
- `PlaylistSummary`, `Playlist {tracks: Page<Track>, track_count, author, ...}`.
- `Page<T> {items, continuation: Option<String>}`, `Item {Track|Album|Artist|Playlist}`, `Section {title, items}`,
  `Genre {name, params}`, `GenreGroup`, `Radio {queue, playlist_id, lyrics_browse_id, related_browse_id}`,
  `Lyrics {text, lines: Vec<LyricLine>, source}` (`is_timed()`), `LyricLine {start_ms, end_ms, text}`.
- All derive `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`.

### Non-music content

Podcast episodes and livestreams are **skipped** (not returned, not marked). A row is skipped when its `musicVideoType` contains
`PODCAST`, `EPISODE` or `NON_MUSIC`, when a badge icon contains `LIVE`, or when a byline run is exactly `LIVE`. Rows without a valid
11-character video id (greyed-out/deleted entries) are skipped too. Albums, artists and playlists with an invalid id are dropped.
`Radio::queue` skips the same things and never includes the "counterpart" (video version) of a queue entry. Skipping is silent:
you cannot tell how many rows were dropped.

### Leniency

Parsers never panic. Odd or missing parts give fewer items or `None` fields. `Error::Parse` is returned when the expected
top-level shape is absent: a search/library/continuation response with none of `contents`, `continuationContents`,
`onResponseReceivedActions`; an album/playlist with neither header nor rows; an artist without a header title; charts, genres,
genre pages and new-release pages with no items; lyrics with neither timed nor plain text. An empty library or search result with the
right top-level key is `Ok` with no items.

Localisation: type labels ("Album", "Single", "EP", "Song"), counts ("25 songs") and the singles/albums split use English
words. With another `hl` those fields fall back to `Unknown` / `None` and singles land in `albums`; ids, titles, durations and
links do not depend on language.

## Tests

- `tests/music_parse.rs`: every parser against its fixture, plus malformed/missing-field cases.
- `tests/music_api.rs`: request bodies, validation before sending, auth handling, error pass-through (`FixtureTransport`).
- `tests/music_fuzz.rs`: every parser on every fixture, mutated (truncated arrays/objects/strings, shuffled order, type swaps,
  deletions), on truncated JSON text and on degenerate documents. Asserts no panic and that returned ids pass validation.

## Unverified against the live site

All of these come from memory of the public site and are covered only by hand-authored fixtures. Check them against recordings.

1. Song filter `params` value `EgWKAQIIAWoMEA4QChADEAQQCRAF` for `search`.
2. Continuations: this crate sends `{"continuation": token}` in the body (the `Client` cannot add the older
   `?ctoken=..&continuation=..&type=next` query string). Whether `search`/`browse` answer that with `continuationContents.*`
   (legacy) or `onResponseReceivedActions[].appendContinuationItemsAction` is not known; both are parsed. Tokens come from
   `nextContinuationData.continuation` or `continuationItemRenderer...continuationCommand.token`.
3. Track rows: video id from `playlistItemData.videoId` / overlay play button / title run; `musicVideoType` values
   `MUSIC_VIDEO_TYPE_ATV|OMV|UGC|OFFICIAL_SOURCE_MUSIC` and the podcast marker; byline runs `Artist • Album • m:ss` with
   `browseEndpoint` page types `MUSIC_PAGE_TYPE_ARTIST|USER_CHANNEL|ALBUM`; duration in a run or in `fixedColumns`;
   explicit badge `MUSIC_EXPLICIT_BADGE` in `badges` (rows, queue) and `subtitleBadges` (cards).
4. Livestream marker: the real badge/icon name is a guess (`LIVE` substring in an icon type, or a run reading `LIVE`).
5. Radio: request fields; queue at `playlistPanelRenderer.contents[]` with `playlistPanelVideoWrapperRenderer.primaryRenderer`;
   lyrics/related tabs found by any `browseId` starting `MPLYt_` / `MPTRt_`.
6. Lyrics: plain text in `musicDescriptionShelfRenderer.description` and attribution in its `footer`. Timed lyrics
   (`timedLyricsData[].cueRange.startTimeMilliseconds`, `sourceMessage`) is the shape another client uses; `WEB_REMIX` may never return
   it, so the timed path may never trigger live.
7. Album: `twoColumnBrowseResultsRenderer` with `musicResponsiveHeaderRenderer` (`straplineTextOne` = artists, `subtitle` =
   `Album • year`, `secondSubtitle` = `N songs • ...`) and rows in `secondaryContents`; legacy `musicDetailHeaderRenderer` with
   `singleColumnBrowseResultsRenderer`. `audio_playlist_id` = first `playlistId` starting `OLAK5uy_` in the header, else the page.
8. Artist: header `musicImmersiveHeaderRenderer` / `musicVisualHeaderRenderer`; subscribers in `subscriberCountText`; top songs =
   first `musicShelfRenderer`, "show all" playlist = `VL...` browse id on its title run; albums/singles/related are split by item type
   and English labels, not by shelf title.
9. Playlist: same header layouts as albums; rows from `musicPlaylistShelfRenderer` (else the first `musicShelfRenderer`) so side
   shelves like "Suggestions" are excluded; next handle taken only from inside that shelf.
10. Explore: charts country via `formData.selectedValues`; genre buttons as `gridRenderer.items[].musicNavigationButtonRenderer`
    with `clickCommand.browseEndpoint.params`; genre page id `FEmusic_moods_and_genres_category`; new-release ids; shelves read from
    any `musicCarouselShelfRenderer` / `musicShelfRenderer` / `gridRenderer`.
11. Suggestions: `music/get_search_suggestions` body `{input}`; entries as `searchSuggestionRenderer` /
    `historySuggestionRenderer` with `navigationEndpoint.searchEndpoint.query`.
12. Library: browse ids `FEmusic_liked_videos`, `FEmusic_liked_playlists`, `FEmusic_history`; that signed-in responses are plain row
    shelves / a card grid (a "New playlist" card without a browse id is skipped; `VLLM` becomes playlist id `LM`); that a signed-out or
    expired cookie shows up as `Unauthenticated`/`Refused`/`Parse` rather than an empty success is not verified.
13. Thumbnail lists are taken from the first `thumbnails` array found under the item's thumbnail node.
