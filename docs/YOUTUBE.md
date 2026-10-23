# innerlight: `youtube` and `account` modules

Signed-in YouTube (client `Web`, host `www.youtube.com`) reads and account actions. All network access goes through `Client::post` /
`Client::post_authed`; everything returns `Result<_, innerlight::Error>`. Parsers are lenient: unknown or odd shapes give fewer items,
never a panic. Nothing here rotates user agents or clients or works around login or bot challenges; such answers surface as the
`Error` the transport maps them to.

The fixtures under `tests/fixtures/yt_handwritten_*` and `account_handwritten_*` are **hand-authored** from knowledge of the public
InnerTube JSON, not recordings. Shapes are unverified against the live site (see the end of this file).

## `innerlight::youtube`

```rust
let yt = Youtube::new(&client);                       // borrows the Client
let p1 = yt.liked_videos(None).await?;                // Page<Video>
let p2 = yt.liked_videos(p1.continuation.as_deref()).await?;
```

| Method | Browse id | Auth | Returns |
|---|---|---|---|
| `liked_videos(cont)` | `VLLL` | `post_authed` | `Page<Video>` |
| `watch_later(cont)` | `VLWL` | `post_authed` | `Page<Video>` |
| `history(cont)` | `FEhistory` | `post_authed` | `Page<Video>` (a video watched twice appears twice) |
| `saved_playlists(cont)` | `FEplaylist_aggregation` | `post_authed` | `Page<PlaylistSummary>` |
| `playlist(id, cont)` | `VL` + id | anonymous `post` | `Page<Video>` |

- `cont: Option<&str>` is `None` for the first page, else the previous page's `continuation`. A continuation request sends
  `{"continuation": token}` and no `browseId`. A token that is empty, over 8192 bytes or contains whitespace/control characters gives
  `InvalidRequest` before anything is sent.
- Signed-in methods without a cookie: `Err(Unauthenticated)`, no request recorded.
- `playlist`: `id` must pass `ids::is_playlist_id` (`PL...`; `VL...` is used as is). `LL`, `WL` and `VL` alone give `InvalidId`
  (use the signed-in methods). Private playlists need a signed-in fetch, which this method does not do.
- A top-level `error` object in the answer gives `Err(Refused)`; a root that is not a JSON object gives `Err(Parse)`.
- `parse_video_page(&Value)` and `parse_playlist_page(&Value)` are public so a caller can parse a stored response offline.

### Types (`youtube::model`, re-exported from `youtube`)

```rust
Page<T>          { items: Vec<T>, continuation: Option<String> }
Video            { id, title, channel: Option<ChannelRef>, duration_ms: Option<u64>, thumbnails: Vec<Thumbnail>,
                   is_live: bool, is_short: bool, set_video_id: Option<String> }
ChannelRef       { id: Option<String>, name: String }
PlaylistSummary  { id, title, video_count: Option<u64>, thumbnails: Vec<Thumbnail> }
Thumbnail        { url, width: u32, height: u32 }
```

All derive `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`.

- `Video.id` always passes `ids::is_video_id`; `ChannelRef.id` is only set when it passes `ids::is_channel_id`.
- Rows without a valid video id or without a title are skipped. Deleted/private rows that still have both stay in (title like
  "[Deleted video]", no channel or duration); filter them in the caller.
- `is_live` / `is_short` are flags only; nothing is filtered. Live rows have `duration_ms: None`.
- `set_video_id` (extra field, not in the original spec) is the playlist-entry id; pass it to `Account::remove_from_playlist` to remove
  exactly that entry.
- `PlaylistSummary.id` has no `VL` prefix. `video_count` is the digits of the row's count text (`None` for "No videos" / absent).

### What the parser recognises (searched anywhere in the response, depth limit 40)

Videos: `playlistVideoRenderer`, `videoRenderer`, `gridVideoRenderer`, `lockupViewModel` with `LOCKUP_CONTENT_TYPE_VIDEO`,
`shortsLockupViewModel`, `reelItemRenderer` (the last two are always `is_short`).
Playlists: `gridPlaylistRenderer`, `playlistRenderer`, `lockupViewModel` with `LOCKUP_CONTENT_TYPE_PLAYLIST`.
Continuation: the last `continuationItemRenderer` that holds a `continuationCommand.token` (also inside a `commandExecutorCommand`).
Works for the first page and for `onResponseReceivedActions[].appendContinuationItemsAction.continuationItems` pages.

## `innerlight::account`

```rust
let acc = Account::new(&client);
acc.like_video("dQw4w9WgXcQ").await?;
let id = acc.create_playlist("Road trip", Privacy::Private, &[]).await?;
```

Every call is `post_authed` on client `Web`, validates ids first (`InvalidId`, nothing sent), and returns `Ok` only for a 2xx JSON object
that has `responseContext` and no top-level `error`. A top-level `error` gives `Err(Refused)`; a missing `responseContext` or a
non-object body gives `Err(Parse)`; no cookie gives `Err(Unauthenticated)` with nothing sent.

| Method | Endpoint | Body fields |
|---|---|---|
| `like_video(video_id)` | `like/like` | `target.videoId` |
| `unlike_video(video_id)` | `like/removelike` | `target.videoId` |
| `dislike_video(video_id)` | `like/dislike` | `target.videoId` |
| `subscribe(channel_id)` | `subscription/subscribe` | `channelIds: [id]` |
| `unsubscribe(channel_id)` | `subscription/unsubscribe` | `channelIds: [id]` |
| `add_to_playlist(playlist_id, video_id)` | `browse/edit_playlist` | `playlistId`, `actions: [{action: "ACTION_ADD_VIDEO", addedVideoId}]` |
| `remove_from_playlist(playlist_id, video_id, set_video_id: Option<&str>)` | `browse/edit_playlist` | `Some`: `{action: "ACTION_REMOVE_VIDEO", removedVideoId, setVideoId}`; `None`: `{action: "ACTION_REMOVE_VIDEO_BY_VIDEO_ID", removedVideoId}` |
| `create_playlist(title, Privacy, &[video_id]) -> String` | `playlist/create` | `title`, `privacyStatus` (`PRIVATE`/`UNLISTED`/`PUBLIC`), `videoIds` |
| `delete_playlist(playlist_id)` | `playlist/delete` | `playlistId` |

- Playlist ids for the edit calls must not start with `VL` (that is the browse id form). `WL` is allowed for add/remove;
  `delete_playlist` rejects `LL` and `WL`.
- `create_playlist` returns the new playlist id (deviation from a bare `Ok(())`, since the caller needs it); a success answer without a
  valid `playlistId` is `Err(Parse)`. Title: 1..=150 chars, no control characters, not blank (`InvalidRequest` otherwise).
- `set_video_id` must be 1..=128 chars of `[A-Za-z0-9_.-]`, else `InvalidId`.

## Shapes not yet checked against real recordings

1. `playlistVideoRenderer` fields: `lengthSeconds` as a string, `setVideoId`, overlay `style` values `LIVE` / `SHORTS`, and that
   Shorts rows in playlists carry `reelWatchEndpoint` or a `/shorts/` URL.
2. `videoRenderer` rows in `FEhistory` (versus a newer `lockupViewModel` layout), day sections as `itemSectionRenderer`, the
   `BADGE_STYLE_TYPE_LIVE_NOW` badge.
3. All `lockupViewModel` details: `contentType` strings, `metadata.lockupMetadataViewModel.title.content`, where the playlist video
   count badge sits (assumed any `thumbnailBadgeViewModel.text` with digits), where the video duration / "LIVE" badge sits, and the
   channel name/id path (`metadataParts[0].text.content`, any `browseId` under `metadata`).
4. `shortsLockupViewModel` (`onTap.innertubeCommand.reelWatchEndpoint.videoId`, `overlayMetadata.primaryText.content`).
5. `FEplaylist_aggregation` returning `richGridRenderer` vs `gridPlaylistRenderer`, `videoCountText` runs, `sidebarThumbnails`.
6. Continuation: token under `continuationItemRenderer...continuationCommand.token` for both first pages and
   `appendContinuationItemsAction`; that the body `{"continuation": token}` (no `browseId`) is what the service wants.
7. Request bodies: `like/*` `target.videoId`; `subscription/*` `channelIds` (the live web client also sends `params`, omitted here);
   `browse/edit_playlist` action names, including `ACTION_REMOVE_VIDEO_BY_VIDEO_ID`; `playlist/create` `privacyStatus` / `videoIds`
   and a top-level `playlistId` in the answer; `playlist/delete` `playlistId`.
8. That every success answer contains `responseContext` and every failure a top-level `error` (a failing 200 that uses another shape,
   e.g. `status: "STATUS_FAILED"` in `edit_playlist`, would currently count as success).
9. History/liked requested anonymously-shaped signed-out answers: the parser returns an empty page (fixture
   `yt_handwritten_empty_signed_out.json`); the live signed-out text is a guess.
