//! Internet Archive: its public audio catalogue, with no key and no account.
//!
//! Everything fetched is an anonymous https GET through [`Fetcher`], so the politeness limits apply here exactly as
//! they do on YouTube. Three URLs are involved: `advancedsearch.php` for a query, `metadata/<identifier>` for one
//! item's files and licence, and `download/<identifier>/<file>` — which [`resolve`] only *hands back*, never
//! fetches. Nothing is decoded or played.
//!
//! [`resolve`]: Provider::resolve

use super::Provider;
use super::catalog::check_query;
use super::http::{Fetcher, encode};
use super::model::{Album, AlbumSummary, Item, Page, Resolved, Track};
use crate::Error;
use crate::music::model::Thumbnail;
use crate::transport::BoxFuture;
use serde_json::Value;

/// [`Provider::name`], and the `source` string on every item.
const NAME: &str = "internetarchive";
const SEARCH: &str = "https://archive.org/advancedsearch.php";
const METADATA: &str = "https://archive.org/metadata/";
const DOWNLOAD: &str = "https://archive.org/download/";
const DETAILS: &str = "https://archive.org/details/";
/// The item's own cover image, which Archive serves at a fixed URL per identifier.
const IMAGE: &str = "https://archive.org/services/img/";

/// Audio files offered, with the MIME type each extension means.
const AUDIO: [(&str, &str); 3] = [(".mp3", "audio/mpeg"), (".ogg", "audio/ogg"), (".flac", "audio/flac")];

/// Longest identifier or file name accepted.
const MAX_ID_LEN: usize = 200;
/// Highest page a continuation may ask for: a crawl of the whole index is not what this is for.
const MAX_PAGE: u32 = 10_000;

/// One character that may appear in a path segment we send: everything else is percent-encoded.
fn is_safe_path_byte(b: u8) -> bool {
    matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/')
}

/// Percent-encode a path tail, keeping `/` as a separator and using `%20` for a space (`+` only means space
/// inside a query, and Archive file names are compared literally).
fn encode_path(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for b in v.bytes() {
        if is_safe_path_byte(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// An item identifier: one path segment, nothing that could escape it.
fn is_valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_ID_LEN
        && !s.starts_with('.')
        && !s.ends_with('.')
        && !s.contains('/')
        && !s.contains("..")
        && !s.contains(':')
        && !s.contains('\\')
        && !s.chars().any(char::is_control)
        && !s.chars().any(char::is_whitespace)
}

/// A file name inside an item: possibly nested, and one of the audio types. Spaces are allowed and encoded, leading
/// or trailing ones are not: they survive percent-encoding as noise the item does not actually have.
fn is_valid_track_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_ID_LEN
        && s.trim() == s
        && !s.starts_with('/')
        && !s.ends_with('/')
        && !s.contains("..")
        && !s.contains(':')
        && !s.contains('\\')
        && !s.chars().any(char::is_control)
        && mime_for(s).is_some()
}

/// The MIME type for a name by its extension, or `None` when it is not an audio file we offer.
fn mime_for(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    AUDIO.iter().find(|(ext, _)| lower.len() > ext.len() && lower.ends_with(ext)).map(|(_, mime)| *mime)
}

/// A field that Archive returns as a string, as a list of strings, or wrapped in `{"type":…, "value":…}`.
fn strings(value: &Value) -> Vec<String> {
    match value {
        Value::String(s) => vec![s.clone()],
        Value::Number(n) => vec![n.to_string()],
        Value::Object(map) => map.get("value").map(strings).unwrap_or_default(),
        Value::Array(items) => items.iter().flat_map(strings).collect(),
        _ => Vec::new(),
    }
}

fn one(value: &Value) -> Option<String> {
    strings(value).into_iter().find(|s| !s.trim().is_empty())
}

fn year(value: &Value) -> Option<u32> {
    let raw = one(value)?;
    let digits: String = raw.chars().take_while(|c| c.is_ascii_digit()).collect();
    match digits.len() {
        1..=4 => digits.parse().ok(),
        _ => None,
    }
}

/// `length` as milliseconds: plain (decimal) seconds, or `mm:ss` / `hh:mm:ss`. An empty leg means the source did
/// not state the length, so nothing is guessed.
fn parse_length(value: &Value) -> Option<u64> {
    let raw = one(value)?;
    let legs: Vec<&str> = raw.split(':').map(str::trim).collect();
    if legs.iter().any(|l| l.is_empty()) {
        return None;
    }
    let parts: Vec<f64> = legs.iter().map(|l| l.parse::<f64>().ok()).collect::<Option<Vec<f64>>>()?;
    if parts.iter().any(|p| !p.is_finite() || *p < 0.0) {
        return None;
    }
    let mut secs = 0f64;
    for p in &parts {
        secs = secs * 60.0 + p;
    }
    Some((secs * 1000.0).round() as u64)
}

/// Public-domain and CC material mostly, but the item's uploader declared it, so the licence is only ever copied
/// from what the response said — never inferred from the source being Archive.
fn license_of(metadata: &Value, file: Option<&Value>) -> Option<String> {
    file.and_then(|f| one(&f["licenseurl"])).or_else(|| one(&metadata["licenseurl"]))
}

/// The cover image. Archive does not state its size, so width and height stay 0 (= unknown).
fn cover(identifier: &str) -> Vec<Thumbnail> {
    vec![Thumbnail { url: format!("{IMAGE}{identifier}"), width: 0, height: 0 }]
}

/// The credit line worth showing: where the item came from.
fn attribution(identifier: &str) -> String {
    format!("{DETAILS}{identifier}")
}

/// One search hit. An Archive item is a collection, so a hit is an album, not a track.
fn summary(doc: &Value) -> Option<Item> {
    let id = one(&doc["identifier"])?;
    if !is_valid_id(&id) {
        return None;
    }
    let title = one(&doc["title"]).unwrap_or_else(|| id.clone());
    Some(Item::Album(AlbumSummary {
        id: id.clone(),
        title,
        artists: strings(&doc["creator"]).into_iter().filter(|s| !s.trim().is_empty()).collect(),
        year: year(&doc["year"]),
        thumbnails: cover(&id),
        license: one(&doc["licenseurl"]),
        attribution: Some(attribution(&id)),
        source: NAME.to_owned(),
    }))
}

/// One audio file of an item as a track. `metadata` supplies the defaults a file row leaves out.
fn track(identifier: &str, metadata: &Value, file: &Value) -> Option<Track> {
    let name = one(&file["name"])?;
    if !is_valid_track_name(&name) {
        return None;
    }
    let base = name.rsplit('/').next().unwrap_or(&name);
    let stem = {
        let lower = base.to_ascii_lowercase();
        AUDIO
            .iter()
            .find(|(ext, _)| lower.len() > ext.len() && lower.ends_with(ext))
            .map(|(ext, _)| &base[..base.len() - ext.len()])
            .unwrap_or(base)
            .to_owned()
    };
    let artists: Vec<String> =
        match strings(&file["artist"]).into_iter().filter(|s| !s.trim().is_empty()).collect::<Vec<_>>() {
            v if !v.is_empty() => v,
            _ => strings(&metadata["creator"]).into_iter().filter(|s| !s.trim().is_empty()).collect(),
        };
    Some(Track {
        id: format!("{identifier}/{name}"),
        title: one(&file["title"]).unwrap_or(stem),
        artists,
        album: one(&metadata["title"]),
        duration_ms: parse_length(&file["length"]),
        thumbnails: cover(identifier),
        license: license_of(metadata, Some(file)),
        attribution: Some(attribution(identifier)),
        source: NAME.to_owned(),
    })
}

/// The Archive provider: search, album and resolve against one host.
///
/// Build it with a [`Fetcher`] over your own [`Transport`](crate::Transport):
/// `Archive::new(Fetcher::new(transport, Policy::polite(), Arc::new(TokioClock::new()), Duration::from_secs(10), 1 << 20)?, Archive::DEFAULT_ROWS)?`.
#[derive(Clone)]
pub struct Archive {
    fetcher: Fetcher,
    rows: usize,
}

impl std::fmt::Debug for Archive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Archive(..)")
    }
}

impl Archive {
    /// Rows a search page asks for when the caller did not choose.
    pub const DEFAULT_ROWS: usize = 40;

    /// `rows` is the page size asked of the search API, 1..=50. A provider that asks for more than that is not
    /// using the endpoint politely, so it is refused here rather than clamped.
    pub fn new(fetcher: Fetcher, rows: usize) -> Result<Self, Error> {
        if rows == 0 || rows > 50 {
            return Err(Error::InvalidRequest);
        }
        Ok(Self { fetcher, rows })
    }

    /// The provider with the standard page size.
    pub fn with_fetcher(fetcher: Fetcher) -> Self {
        Self { fetcher, rows: Self::DEFAULT_ROWS }
    }

    /// `advancedsearch.php` for one page of a query.
    fn search_url(&self, query: &str, page: u32) -> String {
        format!(
            "{SEARCH}?q={}&fl%5B%5D=identifier&fl%5B%5D=title&fl%5B%5D=creator&fl%5B%5D=year&fl%5B%5D=licenseurl&rows={}&page={page}&output=json",
            encode(query),
            self.rows
        )
    }

    fn metadata_url(&self, identifier: &str) -> String {
        format!("{METADATA}{}", encode_path(identifier))
    }

    async fn metadata(&self, identifier: &str) -> Result<Value, Error> {
        if !is_valid_id(identifier) {
            return Err(Error::InvalidId);
        }
        let v = self.fetcher.json(&self.metadata_url(identifier)).await?;
        Ok(v)
    }
}

impl Provider for Archive {
    fn name(&self) -> &'static str {
        NAME
    }

    fn search<'a>(&'a self, query: &'a str, continuation: Option<&'a str>) -> BoxFuture<'a, Result<Page<Item>, Error>> {
        Box::pin(async move {
            let query = check_query(query)?;
            let page = match continuation {
                None => 1,
                Some(t) => {
                    let n: u32 = t.trim().parse().map_err(|_| Error::InvalidId)?;
                    if !(1..=MAX_PAGE).contains(&n) {
                        return Err(Error::InvalidId);
                    }
                    n
                }
            };
            let v = self.fetcher.json(&self.search_url(query, page)).await?;
            let response = v.get("response").ok_or(Error::Parse)?;
            let docs = response.get("docs").and_then(Value::as_array).ok_or(Error::Parse)?;
            let items: Vec<Item> = docs.iter().filter_map(summary).collect();
            let start = response.get("start").and_then(Value::as_u64).unwrap_or(0);
            let found = response.get("numFound").and_then(Value::as_u64).unwrap_or(0);
            let more = !docs.is_empty() && start.saturating_add(docs.len() as u64) < found && page < MAX_PAGE;
            Ok(Page { items, continuation: more.then(|| (page + 1).to_string()) })
        })
    }

    fn album<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Album, Error>> {
        Box::pin(async move {
            let v = self.metadata(id).await?;
            let meta = v.get("metadata").unwrap_or(&v);
            let title = one(&meta["title"]).ok_or(Error::Parse)?;
            let files = v.get("files").and_then(Value::as_array).cloned().unwrap_or_default();
            let items: Vec<Track> = files.iter().filter_map(|f| track(id, meta, f)).collect();
            Ok(Album {
                summary: AlbumSummary {
                    id: id.to_owned(),
                    title,
                    artists: strings(&meta["creator"]).into_iter().filter(|s| !s.trim().is_empty()).collect(),
                    year: year(&meta["year"]),
                    thumbnails: cover(id),
                    license: license_of(meta, None),
                    attribution: Some(attribution(id)),
                    source: NAME.to_owned(),
                },
                description: one(&meta["description"]),
                // Archive lists a whole item in one answer, so there is nothing to page through.
                tracks: Page { items, continuation: None },
            })
        })
    }

    fn resolve<'a>(&'a self, track_id: &'a str) -> BoxFuture<'a, Result<Resolved, Error>> {
        Box::pin(async move {
            let (id, name) = track_id.split_once('/').ok_or(Error::InvalidId)?;
            if !is_valid_id(id) || !is_valid_track_name(name) {
                return Err(Error::InvalidId);
            }
            let mime = mime_for(name).ok_or(Error::InvalidId)?;
            let v = self.metadata(id).await?;
            let meta = v.get("metadata").unwrap_or(&v);
            let file = v
                .get("files")
                .and_then(Value::as_array)
                .and_then(|files| files.iter().find(|f| one(&f["name"]).as_deref() == Some(name)))
                .ok_or(Error::InvalidId)?;
            Ok(Resolved {
                url: format!("{DOWNLOAD}{}/{}", encode_path(id), encode_path(name)),
                duration_ms: parse_length(&file["length"]),
                mime: Some(mime.to_owned()),
                license: license_of(meta, Some(file)),
                attribution: Some(attribution(id)),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_encoding_keeps_only_what_a_url_path_allows() {
        assert_eq!(encode_path("a-b_c.d~e/01.mp3"), "a-b_c.d~e/01.mp3");
        assert_eq!(encode_path("My Song (2020).mp3"), "My%20Song%20%282020%29.mp3");
        assert_eq!(encode_path("ä.md"), "%C3%A4.md");
        assert_eq!(encode_path("a?b&c"), "a%3Fb%26c");
    }

    #[test]
    fn ids_and_names_that_could_escape_the_path_are_refused() {
        for bad in [
            "",
            "a/b",
            "..",
            "a..b",
            ".hidden",
            "trailing.",
            "has space",
            "a:b",
            "a\\b",
            "a\u{1}b",
            &"x".repeat(MAX_ID_LEN + 1),
        ] {
            assert!(!is_valid_id(bad), "id {bad:?}");
        }
        assert!(is_valid_id("goatlivemusic2020-02-22"));
        assert!(is_valid_id("whatever.3"));
        // A name must additionally be one of the audio files we offer.
        for bad in [
            "",
            "/abs.mp3",
            "rel.mp3/",
            "../evil.mp3",
            "song.txt",
            "song",
            ".mp3",
            " Song.mp3",
            "Song.mp3 ",
            "a:b.mp3",
            "a\nb.mp3",
            &"y".repeat(MAX_ID_LEN),
        ] {
            assert!(!is_valid_track_name(bad), "name {bad:?}");
        }
        assert!(is_valid_track_name("01 Track.mp3"));
        assert!(is_valid_track_name("disc1/02.flac"));
    }

    #[test]
    fn the_mime_follows_the_extension_case_insensitively() {
        assert_eq!(mime_for("a.mp3"), Some("audio/mpeg"));
        assert_eq!(mime_for("A.B.OGG"), Some("audio/ogg"));
        assert_eq!(mime_for("x.flac"), Some("audio/flac"));
        assert_eq!(mime_for("mp3"), None);
        assert_eq!(mime_for(".mp3"), None);
        assert_eq!(mime_for("a.mp4"), None);
    }

    #[test]
    fn lengths_arrive_as_seconds_or_clock_notation() {
        let f = |s: &str| parse_length(&Value::String(s.to_owned()));
        assert_eq!(f("253.04"), Some(253_040));
        assert_eq!(f("0"), Some(0));
        assert_eq!(f("3:51"), Some(231_000));
        assert_eq!(f("1:02:03"), Some(3_723_000));
        assert_eq!(f("4:3.5"), Some(243_500));
        assert_eq!(f(" 12 "), Some(12_000));
        for junk in ["", "abc", "-5", "1:2:x", "3:51:"] {
            assert_eq!(f(junk), None, "{junk:?}");
        }
        assert_eq!(parse_length(&Value::Number(90.into())), Some(90_000));
        assert_eq!(parse_length(&Value::Null), None);
    }

    #[test]
    fn string_fields_accept_every_shape_archive_uses() {
        assert_eq!(one(&Value::String("A".into())), Some("A".into()));
        assert_eq!(strings(&Value::Array(vec!["A".into(), "B".into()])), ["A", "B"]);
        assert_eq!(one(&serde_json::json!({"type": "text/plain", "value": "V"})), Some("V".into()));
        assert_eq!(strings(&Value::Bool(true)), Vec::<String>::new());
        assert_eq!(year(&serde_json::json!("1972")), Some(1972));
        assert_eq!(year(&serde_json::json!(2020)), Some(2020));
        assert_eq!(year(&serde_json::json!("nope")), None);
    }

    #[test]
    fn a_search_hit_is_an_album_with_the_item_cover_and_whatever_licence_it_claimed() {
        let doc = serde_json::json!({
            "identifier": "goatlivemusic2020-02-22",
            "title": ["Goat, Live"],
            "creator": "Goat",
            "year": "2020",
            "licenseurl": "http://creativecommons.org/publicdomain/zero/1.0/"
        });
        let item = summary(&doc).unwrap();
        let Item::Album(a) = &item else { panic!("an Archive hit is an album: {item:?}") };
        assert_eq!(a.id, "goatlivemusic2020-02-22");
        assert_eq!(a.title, "Goat, Live");
        assert_eq!(a.artists, ["Goat"]);
        assert_eq!(a.year, Some(2020));
        assert_eq!(a.thumbnails[0].url, format!("{}goatlivemusic2020-02-22", IMAGE));
        assert_eq!(a.license.as_deref(), Some("http://creativecommons.org/publicdomain/zero/1.0/"));
        assert_eq!(a.attribution.as_deref(), Some("https://archive.org/details/goatlivemusic2020-02-22"));
        assert_eq!(a.source, NAME);
        // A hit whose id could not be put back into a URL is dropped, not sanitised.
        assert!(summary(&serde_json::json!({"identifier": "a/b", "title": "x"})).is_none());
        assert!(summary(&serde_json::json!({"title": "no id"})).is_none());
    }

    #[test]
    fn a_file_row_becomes_a_track_bearing_the_item_defaults() {
        let meta = serde_json::json!({"title": "Album", "creator": "Someone", "licenseurl": "L"});
        let file =
            serde_json::json!({"name": "disc1/01 Song.mp3", "title": "Song", "length": "3:51", "artist": "Other"});
        let t = track("item-id", &meta, &file).unwrap();
        assert_eq!(t.id, "item-id/disc1/01 Song.mp3");
        assert_eq!(t.title, "Song");
        assert_eq!(t.artists, ["Other"]);
        assert_eq!(t.album.as_deref(), Some("Album"));
        assert_eq!(t.duration_ms, Some(231_000));
        assert_eq!(t.license.as_deref(), Some("L"));
        assert_eq!(t.source, NAME);
        // A row with no title falls back to its stem, and with no artist to the item's creator.
        let bare = track("item-id", &meta, &serde_json::json!({"name": "02.mp3", "length": "10"})).unwrap();
        assert_eq!(bare.title, "02");
        assert_eq!(bare.artists, ["Someone"]);
        assert_eq!(bare.duration_ms, Some(10_000));
        // Video, scans and metadata dumps are not tracks.
        assert!(track("item-id", &meta, &serde_json::json!({"name": "scan.tif"})).is_none());
        assert!(track("item-id", &meta, &serde_json::json!({"title": "no name"})).is_none());
    }

    #[tokio::test]
    async fn a_nonsensical_page_size_or_request_is_refused_before_any_fetch() {
        let fx = std::sync::Arc::new(crate::transport::FixtureTransport::new());
        let fetcher = Fetcher::new(
            fx.clone(),
            crate::Policy::no_retry(),
            std::sync::Arc::new(crate::FakeClock::new()),
            std::time::Duration::from_secs(1),
            1 << 16,
        )
        .unwrap();
        assert!(Archive::new(fetcher.clone(), 0).is_err());
        assert!(Archive::new(fetcher.clone(), 51).is_err());
        let a = Archive::with_fetcher(fetcher);
        assert_eq!(a.name(), NAME);
        assert_eq!(a.search("", None).await.unwrap_err(), Error::InvalidRequest);
        assert_eq!(a.search(&"x".repeat(513), None).await.unwrap_err(), Error::InvalidRequest);
        for junk in ["", "0", "50000", "page-2", "-1"] {
            assert_eq!(a.search("q", Some(junk)).await.unwrap_err(), Error::InvalidId, "{junk:?}");
        }
        for junk in ["", "no-slash", "a/b", "bad//x.mp3", "../x.mp3", "a/b.txt"] {
            assert_eq!(a.resolve(junk).await.unwrap_err(), Error::InvalidId, "{junk:?}");
        }
        assert_eq!(a.album("bad..id").await.unwrap_err(), Error::InvalidId);
        // Nothing above reached the network.
        assert!(fx.requests().is_empty());
    }

    #[test]
    fn urls_are_built_from_the_constants_and_nothing_else() {
        let fetcher = Fetcher::new(
            std::sync::Arc::new(crate::transport::FixtureTransport::new()),
            crate::Policy::no_retry(),
            std::sync::Arc::new(crate::FakeClock::new()),
            std::time::Duration::from_secs(1),
            1 << 16,
        )
        .unwrap();
        let a = Archive::new(fetcher, 40).unwrap();
        assert_eq!(
            a.search_url("pile of songs", 2),
            "https://archive.org/advancedsearch.php?q=pile+of+songs&fl%5B%5D=identifier&fl%5B%5D=title&\
             fl%5B%5D=creator&fl%5B%5D=year&fl%5B%5D=licenseurl&rows=40&page=2&output=json"
        );
        assert_eq!(a.metadata_url("goat-2020"), "https://archive.org/metadata/goat-2020");
        assert_eq!(crate::provider::check_url(&a.search_url("q", 1)), Ok(()));
    }
}
