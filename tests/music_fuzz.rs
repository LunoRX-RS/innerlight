//! Fuzz-style robustness: every parser is fed truncated, shuffled and type-swapped variants of every music fixture.
//! The only assertions are "no panic" and "whatever comes out carries validated ids".

use innerlight::ids::{is_channel_id, is_video_id};
use innerlight::music::ids::{is_album_browse_id, is_continuation_token, is_music_playlist_id};
use innerlight::music::model::*;
use innerlight::music::parse;
use serde_json::{Map, Value, json};

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
            if !name.starts_with("music_handwritten_") {
                return None;
            }
            Some((name, serde_json::from_str(&std::fs::read_to_string(&p).ok()?).unwrap()))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(out.len() >= 20, "fixtures missing");
    out
}

fn check_track(t: &Track) {
    assert!(is_video_id(&t.id), "{}", t.id);
    for a in &t.artists {
        assert!(a.id.as_deref().is_none_or(is_channel_id));
    }
    assert!(t.album.as_ref().and_then(|a| a.id.as_deref()).is_none_or(is_album_browse_id));
}

fn check_item(i: &Item) {
    match i {
        Item::Track(t) => check_track(t),
        Item::Album(a) => assert!(is_album_browse_id(&a.id)),
        Item::Artist(a) => assert!(is_channel_id(&a.id)),
        Item::Playlist(p) => assert!(is_music_playlist_id(&p.id)),
    }
}

/// Runs every parser on `v`. Panics only if a parser panics or returns an unvalidated id.
fn run_all(v: &Value) {
    if let Ok(p) = parse::search_tracks(v) {
        p.items.iter().for_each(check_track);
        assert!(p.continuation.as_deref().is_none_or(is_continuation_token));
    }
    let _ = parse::track_page(v).map(|p| p.items.iter().for_each(check_track));
    let _ = parse::search_suggestions(v);
    if let Ok(r) = parse::radio(v) {
        r.queue.iter().for_each(check_track);
    }
    let _ = parse::lyrics(v);
    if let Ok(a) = parse::album(v, "MPREb_9nqEki4ZDpp") {
        a.tracks.iter().for_each(check_track);
    }
    if let Ok(a) = parse::artist(v, "UCuAXFkgsw1L7xaCfnd5JJOw") {
        a.top_songs.iter().for_each(check_track);
        a.albums.iter().chain(&a.singles).for_each(|x| assert!(is_album_browse_id(&x.id)));
        a.related.iter().for_each(|x| assert!(is_channel_id(&x.id)));
    }
    if let Ok(p) = parse::playlist(v, "PLTESTPLAYLIST1234567890abcdefgh") {
        p.tracks.items.iter().for_each(check_track);
    }
    for s in parse::charts(v).into_iter().chain(parse::genre_playlists(v)).flatten() {
        s.items.iter().for_each(check_item);
    }
    let _ = parse::genres(v);
    let _ = parse::new_albums(v);
    let _ = parse::new_videos(v).map(|t| t.iter().for_each(check_track));
    let _ = parse::library_tracks(v).map(|p| p.items.iter().for_each(check_track));
    let _ = parse::library_playlists(v);
}

fn mutate(v: &mut Value, rng: &mut Rng) {
    // 1 in 4: stop here and damage this node; otherwise descend into a random child
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
            // drop the trailing half of the keys
            let keep = m.len() / 2;
            let kept: Map<String, Value> = std::mem::take(m).into_iter().take(keep).collect();
            *m = kept;
        }
        (2, Value::Object(m)) if m.len() > 1 => {
            // shuffle key order
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
            *v = [json!(0), json!(-1), json!("x"), json!(true), json!([]), json!({}), json!([[]]), json!({"runs": 5})]
                [rng.below(8)]
            .clone()
        }
        _ => *v = json!({"runs": [{"text": 1}, null, {"navigationEndpoint": 3}]}),
    }
}

#[test]
fn originals_do_not_panic() {
    for (_, v) in fixtures() {
        run_all(&v);
    }
}

#[test]
fn mutated_fixtures_never_panic() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for (name, v) in fixtures() {
        for round in 0..250 {
            let mut m = v.clone();
            for _ in 0..1 + round % 4 {
                mutate(&mut m, &mut rng);
            }
            run_all(&m);
        }
        eprintln!("fuzzed {name}");
    }
}

#[test]
fn truncated_text_never_panics() {
    // cut each fixture's serialized form at many offsets; whatever still parses as JSON goes through the parsers
    for (_, v) in fixtures() {
        let text = serde_json::to_string(&v).unwrap();
        let step = (text.len() / 150).max(1);
        for cut in (0..text.len()).step_by(step) {
            if let Some(Ok(p)) = text.get(..cut).map(serde_json::from_str::<Value>) {
                run_all(&p);
            }
        }
    }
}

#[test]
fn degenerate_documents() {
    let mut deep = json!("leaf");
    for _ in 0..200 {
        deep = json!({"contents": [{"musicShelfRenderer": deep}], "runs": [{"text": "x"}]});
    }
    let mut deep_arr = json!("leaf");
    for _ in 0..500 {
        deep_arr = json!([deep_arr]);
    }
    for v in [
        json!(null),
        json!(true),
        json!(1),
        json!("s"),
        json!([]),
        json!({}),
        json!([[]]),
        json!({"contents": null}),
        deep_arr,
        deep,
    ] {
        run_all(&v);
    }
}
