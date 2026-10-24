//! Fuzz-style robustness for the provider layer: the Archive parsers are fed truncated, shuffled and type-swapped
//! variants of every `archive_handwritten_*` fixture, and each answer is also tried against the other call, so a
//! search page gets parsed as metadata and back.
//!
//! The assertions are only "no panic" and "whatever comes out is still a safe URL and a still-safe id": a provider
//! must never be able to hand back a `url` that is not a plain https download, or an id that could escape one.

use innerlight::provider::model::{AlbumSummary, Item, Page, Resolved, Track};
use innerlight::provider::{Archive, Fetcher, Provider, check_url};
use innerlight::transport::{BoxFuture, Request, Response, Transport};
use innerlight::{Error, FakeClock, Policy};
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::time::Duration;

const ID: &str = "goatlivemusic2020-02-22";
const NAME: &str = "disc1/02 Midset.mp3";

/// Answers every GET with the same document, whatever URL was asked for.
struct Echo(Value);

impl Transport for Echo {
    fn send(&self, _request: Request) -> BoxFuture<'_, Result<Response, Error>> {
        Box::pin(async { panic!("a provider only ever GETs") })
    }
    fn get(&self, _url: String) -> BoxFuture<'_, Result<Response, Error>> {
        let body = serde_json::to_vec(&self.0).expect("a Value serializes");
        Box::pin(async move { Ok(Response { status: 200, retry_after: None, body }) })
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

fn fixtures() -> Vec<(String, Value)> {
    let dir = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| {
            let p = e.ok()?.path();
            let name = p.file_name()?.to_str()?.to_owned();
            if !name.starts_with("archive_handwritten_") {
                return None;
            }
            Some((name, serde_json::from_str(&std::fs::read_to_string(&p).ok()?).unwrap()))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(out.len() >= 3, "fixtures missing");
    out
}

/// A string that can be pasted into a URL path without changing where the request goes.
fn check_path_piece(s: &str, what: &str) {
    assert!(!s.is_empty(), "{what} is empty");
    assert!(!s.starts_with('.'), "{what} starts with a dot: {s}");
    assert!(!s.contains(".."), "{what} walks out of the path: {s}");
    assert!(!s.contains(':'), "{what} could carry a scheme: {s}");
    assert!(!s.contains('\\'), "{what} has a backslash: {s}");
    assert!(!s.chars().any(char::is_control), "{what} has control characters");
}

fn check_summary(a: &AlbumSummary) {
    assert_eq!(a.source, "internetarchive");
    check_path_piece(&a.id, "an album id");
    assert!(!a.title.trim().is_empty(), "a title that is only whitespace");
    assert!(a.year.is_none_or(|y| y < 10000));
    for t in &a.thumbnails {
        assert!(t.url.starts_with("https://archive.org/"), "cover {}", t.url);
    }
}

fn check_track(t: &Track) {
    assert_eq!(t.source, "internetarchive");
    let (id, name) = t.id.split_once('/').unwrap_or_else(|| panic!("a track id with no separator: {}", t.id));
    check_path_piece(id, "a track's item id");
    assert!(name.ends_with(".mp3") || name.ends_with(".ogg") || name.ends_with(".flac"), "track name {name}");
    assert!(!name.starts_with('/'), "an absolute track name: {name}");
    assert!(!t.title.trim().is_empty());
    for artist in &t.artists {
        assert!(!artist.trim().is_empty(), "a blank credit in {}", t.id);
    }
}

fn check_resolved(r: &Resolved) {
    assert!(r.url.starts_with("https://archive.org/download/"), "resolved {}", r.url);
    check_url(&r.url).expect("a resolved URL that Fetcher would refuse");
    if let Some(mime) = &r.mime {
        assert!(["audio/mpeg", "audio/ogg", "audio/flac"].contains(&mime.as_str()), "mime {mime}");
    }
}

/// Runs every Archive parse on `v`, through all three calls. Panics only on a panic or an unsafe result.
async fn run_all(v: &Value) {
    let transport: Arc<dyn Transport> = Arc::new(Echo(v.clone()));
    let fetcher =
        Fetcher::new(transport, Policy::no_retry(), Arc::new(FakeClock::new()), Duration::from_secs(5), 1 << 20)
            .expect("a valid policy");
    let a = Archive::new(fetcher, 40).unwrap();
    if let Ok(page) = a.search("goat live", None).await {
        let Page { items, continuation } = &page;
        for item in items {
            match item {
                Item::Album(al) => check_summary(al),
                Item::Track(t) => check_track(t),
            }
        }
        if let Some(c) = continuation {
            let n: u32 = c.parse().expect("a continuation that is not a page number");
            assert!((1..=10_000).contains(&n), "a page number of {n}");
        }
    }
    if let Ok(album) = a.album(ID).await {
        check_summary(&album.summary);
        for t in &album.tracks.items {
            check_track(t);
        }
        assert!(album.tracks.continuation.is_none(), "one metadata answer is the whole item");
    }
    let _ = run_resolve(&a).await;
}

async fn run_resolve(a: &Archive) -> Result<Resolved, Error> {
    let id = format!("{ID}/{NAME}");
    let r = a.resolve(&id).await;
    if let Ok(r) = &r {
        check_resolved(r);
    }
    r
}

fn mutate(v: &mut Value, rng: &mut Rng) {
    let here = rng.below(4) == 0;
    match v {
        Value::Object(m) if !m.is_empty() && !here => {
            let k = m.keys().nth(rng.below(m.len())).cloned().unwrap();
            mutate(m.get_mut(&k).unwrap(), rng);
        }
        Value::Array(a) if !a.is_empty() && !here => {
            let i = rng.below(a.len());
            mutate(&mut a[i], rng);
        }
        _ => damage(v, rng),
    }
}

fn damage(v: &mut Value, rng: &mut Rng) {
    match (rng.below(9), &mut *v) {
        (0, Value::Object(m)) if !m.is_empty() => {
            let k = m.keys().nth(rng.below(m.len())).cloned().unwrap();
            m.shift_remove(&k);
        }
        (1, Value::Object(m)) => {
            let keep = m.len() / 2;
            *m = std::mem::take(m).into_iter().take(keep).collect::<Map<String, Value>>();
        }
        (2, Value::Object(m)) if m.len() > 1 => {
            let mut e: Vec<_> = std::mem::take(m).into_iter().collect();
            let k = 1 + rng.below(e.len() - 1);
            e.rotate_left(k);
            e.reverse();
            *m = e.into_iter().collect();
        }
        (3, Value::Array(a)) => a.truncate(rng.below(a.len() + 1)),
        (4, Value::Array(a)) if a.len() > 1 => {
            let k = 1 + rng.below(a.len() - 1);
            a.rotate_left(k);
            a.reverse();
        }
        (5, Value::String(s)) => {
            s.truncate(s.char_indices().nth(rng.below(s.chars().count() + 1)).map_or(s.len(), |(i, _)| i))
        }
        (6, _) => *v = Value::Null,
        (7, _) => {
            *v = [
                json!(0),
                json!(-1),
                json!("x"),
                json!(true),
                json!([]),
                json!({}),
                json!(["a/b"]),
                json!({"value": 5}),
            ][rng.below(8)]
            .clone()
        }
        _ => *v = json!([{"name": "a/b/c.mp3"}, {"identifier": "../x", "title": "t"}, {"length": "1:2:"}]),
    }
}

#[tokio::test]
async fn originals_do_not_panic() {
    for (name, v) in fixtures() {
        run_all(&v).await;
        eprintln!("checked {name}");
    }
}

#[tokio::test]
async fn mutated_fixtures_never_panic_and_never_leak_an_unsafe_id() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for (name, v) in fixtures() {
        for round in 0..200 {
            let mut m = v.clone();
            for _ in 0..1 + round % 4 {
                mutate(&mut m, &mut rng);
            }
            run_all(&m).await;
        }
        eprintln!("fuzzed {name}");
    }
}

#[tokio::test]
async fn truncated_text_never_panics() {
    for (_, v) in fixtures() {
        let text = serde_json::to_string(&v).unwrap();
        let step = (text.len() / 120).max(1);
        for cut in (0..text.len()).step_by(step) {
            if let Some(Ok(p)) = text.get(..cut).map(serde_json::from_str::<Value>) {
                run_all(&p).await;
            }
        }
    }
}

#[tokio::test]
async fn degenerate_documents() {
    // One nested child per level: a document that deep, not a document that big.
    let mut deep = json!("leaf");
    for _ in 0..150 {
        deep = json!({"response": {"numFound": 1, "start": 0, "docs": [{"identifier": deep}]}});
    }
    let mut deep_files = json!("leaf");
    for _ in 0..150 {
        deep_files = json!({"metadata": {"title": deep_files}, "files": [{"name": "x.mp3"}]});
    }
    let mut deep_arr = json!("leaf");
    for _ in 0..400 {
        deep_arr = json!([deep_arr]);
    }
    for v in [
        json!(null),
        json!(true),
        json!(1),
        json!("s"),
        json!([]),
        json!({}),
        json!({"response": []}),
        json!({"response": {"docs": {}, "start": [], "numFound": "x"}}),
        json!({"metadata": [], "files": {"name": "x.mp3"}}),
        json!({"identifier": ID, "name": NAME}),
        deep_arr,
        deep,
        deep_files,
    ] {
        run_all(&v).await;
    }
}

#[tokio::test]
async fn a_file_name_cannot_turn_the_download_url_into_something_else() {
    // Every hostile file name the mutated fixtures produced is refused before a URL is built; these are the
    // shapes written on purpose.
    let transport: Arc<dyn Transport> = Arc::new(Echo(json!({
        "files": [
            {"name": "../../escape.mp3", "length": "10"},
            {"name": "https:/evil.mp3"},
            {"name": "a\\b.mp3"},
            {"name": "nul\u{1}char.mp3"},
            {"name": "/abs.mp3"},
            {"name": "trailing-space.mp3 "},
            {"name": "ok.mp3", "length": "10"}
        ],
        "metadata": {"identifier": ID, "title": "Hostile names"}
    })));
    let fetcher =
        Fetcher::new(transport, Policy::no_retry(), Arc::new(FakeClock::new()), Duration::from_secs(5), 1 << 20)
            .unwrap();
    let a = Archive::new(fetcher, 40).unwrap();
    let album = a.album(ID).await.unwrap();
    assert_eq!(album.tracks.items.len(), 1, "only the well-formed name survives");
    assert_eq!(album.tracks.items[0].id, format!("{ID}/ok.mp3"));
    for bad in ["../../escape.mp3", "https:/evil.mp3", "a\\b.mp3", "nul\u{1}char.mp3", "/abs.mp3"] {
        let id = format!("{ID}/{bad}");
        assert_eq!(a.resolve(&id).await.unwrap_err(), Error::InvalidId, "{bad:?}");
    }
    let r = run_resolve(&a).await;
    assert!(r.is_err(), "the item does not list {NAME}");
}
