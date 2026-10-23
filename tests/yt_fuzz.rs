//! Fuzz-style robustness: truncated text and structurally mutated fixtures must never panic the YouTube parsers.

use innerlight::youtube::{parse_playlist_page, parse_video_page};
use serde_json::Value;

const FILES: &[&str] = &[
    "yt_handwritten_liked_page1.json",
    "yt_handwritten_liked_page2.json",
    "yt_handwritten_history.json",
    "yt_handwritten_history_page2.json",
    "yt_handwritten_saved_playlists_lockup.json",
    "yt_handwritten_saved_playlists_grid.json",
    "yt_handwritten_lockup_videos.json",
    "yt_handwritten_empty_signed_out.json",
];

/// xorshift64*: deterministic, no dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn paths(v: &Value, here: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    out.push(here.clone());
    match v {
        Value::Object(m) => m.iter().for_each(|(k, c)| {
            here.push(k.clone());
            paths(c, here, out);
            here.pop();
        }),
        Value::Array(a) => a.iter().enumerate().for_each(|(i, c)| {
            here.push(i.to_string());
            paths(c, here, out);
            here.pop();
        }),
        _ => {}
    }
}

fn node_mut<'a>(v: &'a mut Value, path: &[String]) -> Option<&'a mut Value> {
    let mut cur = v;
    for p in path {
        cur = match cur {
            Value::Object(m) => m.get_mut(p)?,
            Value::Array(a) => a.get_mut(p.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

fn mutate(v: &mut Value, rng: &mut Rng) {
    let mut all = Vec::new();
    paths(v, &mut Vec::new(), &mut all);
    let path = &all[rng.below(all.len())];
    let Some(node) = node_mut(v, path) else { return };
    match rng.below(7) {
        0 => *node = Value::Null,
        1 => *node = Value::from(rng.next() as i64),
        2 => *node = Value::from("x".repeat(rng.below(40))),
        3 => *node = Value::Array(vec![node.clone(), Value::Null]),
        4 => {
            if let Value::Array(a) = node {
                a.reverse();
                a.truncate(rng.below(a.len() + 1));
            }
        }
        5 => {
            if let Value::Object(m) = node {
                let keys: Vec<_> = m.keys().cloned().collect();
                if !keys.is_empty() {
                    m.shift_remove(&keys[rng.below(keys.len())]);
                }
            }
        }
        _ => {
            if let Value::Object(m) = node {
                // swap two values: shuffles which key holds what
                let keys: Vec<_> = m.keys().cloned().collect();
                if keys.len() >= 2 {
                    let (a, b) = (keys[rng.below(keys.len())].clone(), keys[rng.below(keys.len())].clone());
                    let (va, vb) = (m[&a].clone(), m[&b].clone());
                    m[&a] = vb;
                    m[&b] = va;
                }
            }
        }
    }
}

fn run(v: &Value) {
    let _ = parse_video_page(v);
    let _ = parse_playlist_page(v);
}

#[test]
fn truncated_text_never_panics() {
    for f in FILES {
        let bytes = std::fs::read(format!("{}/tests/fixtures/{f}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        for end in (0..bytes.len()).step_by(7) {
            if let Ok(v) = serde_json::from_slice::<Value>(&bytes[..end]) {
                run(&v);
            }
        }
    }
}

#[test]
fn mutated_structure_never_panics() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for f in FILES {
        let base: Value = serde_json::from_slice(
            &std::fs::read(format!("{}/tests/fixtures/{f}", env!("CARGO_MANIFEST_DIR"))).unwrap(),
        )
        .unwrap();
        for _ in 0..400 {
            let mut v = base.clone();
            for _ in 0..1 + rng.below(4) {
                mutate(&mut v, &mut rng);
            }
            run(&v);
        }
    }
}

#[test]
fn deep_nesting_is_cut_off() {
    let mut v = Value::Null;
    for _ in 0..100 {
        v = serde_json::json!({"a": [v]});
    }
    run(&v);
    let _ = parse_video_page(&v).unwrap();
}
