//! No input makes the player module panic: truncated bodies, mutated JSON, hostile shapes.

use innerlight::player::parse::{parse_playability, parse_streaming_data};
use innerlight::player::{Player, Quality, ResolveOptions};
use innerlight::{Client, FixtureTransport};
use serde_json::{Value, json};
use std::sync::Arc;

const FIXTURES: [&str; 9] =
    ["ok_direct", "ok_ciphered", "login_plain", "login_bot", "age", "live", "unplayable", "unavailable", "malformed"];

fn raw(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/fixtures/player_handwritten_{name}.json", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn junk(r: &mut Rng) -> Value {
    match r.next() % 8 {
        0 => Value::Null,
        1 => json!(true),
        2 => json!(-1),
        3 => json!(u64::MAX),
        4 => json!("\u{0}\u{2019}x%zz&=?#"),
        5 => json!([]),
        6 => json!({"a": [1, {"b": null}]}),
        _ => json!("https://evil.example/\n"),
    }
}

/// Replaces one randomly chosen node by junk.
fn mutate(v: &mut Value, r: &mut Rng) {
    let mut nodes = 0usize;
    fn count(v: &Value, n: &mut usize) {
        *n += 1;
        match v {
            Value::Array(a) => a.iter().for_each(|x| count(x, n)),
            Value::Object(o) => o.values().for_each(|x| count(x, n)),
            _ => {}
        }
    }
    count(v, &mut nodes);
    let mut target = (r.next() as usize) % nodes;
    fn walk(v: &mut Value, target: &mut usize, j: &mut Option<Value>) {
        if *target == 0 {
            *v = j.take().unwrap_or(Value::Null);
            *target = usize::MAX;
            return;
        }
        *target -= 1;
        match v {
            Value::Array(a) => a.iter_mut().for_each(|x| walk(x, target, j)),
            Value::Object(o) => o.values_mut().for_each(|x| walk(x, target, j)),
            _ => {}
        }
    }
    walk(v, &mut target, &mut Some(junk(r)));
}

#[test]
fn mutated_json_never_panics() {
    let c = Client::builder().build().unwrap();
    let p = Player::new(&c);
    let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
    for name in FIXTURES {
        let base: Value = serde_json::from_slice(&raw(name)).unwrap();
        for _ in 0..400 {
            let mut v = base.clone();
            for _ in 0..1 + rng.next() % 3 {
                mutate(&mut v, &mut rng);
            }
            let _ = parse_playability(&v);
            let _ = parse_streaming_data(&v);
            for q in [Quality::Low, Quality::Normal, Quality::High] {
                let _ = p.resolve_response(&v, q);
            }
        }
    }
}

#[test]
fn non_object_roots() {
    let c = Client::builder().build().unwrap();
    for v in [json!(null), json!(1), json!("x"), json!([]), json!([{"playabilityStatus": {"status": "OK"}}])] {
        assert!(Player::new(&c).resolve_response(&v, Quality::High).is_err());
    }
}

#[tokio::test]
async fn truncated_bodies_are_errors_not_panics() {
    for name in FIXTURES {
        let body = raw(name);
        let step = (body.len() / 60).max(1);
        for cut in (0..body.len()).step_by(step) {
            let t = Arc::new(FixtureTransport::new().with_raw("player", 200, &body[..cut]));
            let c = Client::builder().transport(t).build().unwrap();
            let r = Player::new(&c).resolve("dQw4w9WgXcQ", ResolveOptions::default()).await;
            assert!(r.is_err(), "{name} cut at {cut} of {}", body.len());
        }
    }
}

#[tokio::test]
async fn garbage_bodies() {
    for body in [&b""[..], b"null", b"[]", b"{}", b"\xff\xfe\x00", b"{\"playabilityStatus\":", b"\"OK\""] {
        let t = Arc::new(FixtureTransport::new().with_raw("player", 200, body));
        let c = Client::builder().transport(t).build().unwrap();
        assert!(Player::new(&c).resolve("dQw4w9WgXcQ", ResolveOptions::default()).await.is_err());
    }
}

#[test]
fn huge_stream_list_is_capped() {
    let one = json!({"itag": 140, "mimeType": "audio/mp4", "bitrate": 1, "contentLength": "1", "url": "https://a.googlevideo.com/v"});
    let v = json!({"streamingData": {"adaptiveFormats": vec![one; 5000]}});
    assert!(parse_streaming_data(&v).unwrap().streams.len() <= 512);
}
