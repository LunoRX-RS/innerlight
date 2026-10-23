//! The saved real player script (`tests/fixtures/player_js_cbcfbf2f.js`, fetched 2026-10-23): both transforms must
//! be located by structure and behave like the transforms. This proves the extraction and execution machinery
//! against a real script; it cannot prove that the service accepts a deciphered URL (that needs a live player answer).

use innerlight::decipher::{PlayerScript, ScriptDecipher};
use std::collections::BTreeMap;
use std::time::Instant;

fn fixture() -> PlayerScript {
    let js = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/player_js_cbcfbf2f.js"))
        .expect("fixture");
    PlayerScript::from_source("cbcfbf2f".into(), js).expect("valid script")
}

fn counts(s: &str) -> BTreeMap<char, usize> {
    let mut m = BTreeMap::new();
    for c in s.chars() {
        *m.entry(c).or_insert(0) += 1;
    }
    m
}

#[test]
fn real_script_both_transforms() {
    let script = fixture();
    assert_eq!(script.signature_timestamp(), Ok(20746));
    let t = Instant::now();
    let d = ScriptDecipher::new(&script).expect("both transforms located and verified");
    eprintln!("analysis took {:?}; found by {:?}", t.elapsed(), d.found_by());
    // 100 chars with repeats.
    let alphabet: String =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_".chars().cycle().take(100).collect();
    let out = d.signature(&alphabet).expect("signature");
    eprintln!("signature: in {} chars, out {} chars", alphabet.len(), out.len());
    let (cin, cout) = (counts(&alphabet), counts(&out));
    assert!(!out.is_empty() && out != alphabet);
    assert!(out.len() <= alphabet.len());
    assert!(cout.iter().all(|(c, n)| cin.get(c).is_some_and(|m| n <= m)), "output chars come from the input");
    assert_eq!(d.signature(&alphabet).unwrap(), out, "deterministic");
    let n = "Qa3_9-XyZ0bk";
    let a = d.throttle_value(n).expect("n transform");
    assert!(!a.is_empty() && a != n);
    assert_eq!(d.throttle_value(n).unwrap(), a, "deterministic");
    assert_ne!(d.throttle_value("abcdefghijklmnop").unwrap(), a);
}

#[test]
fn real_script_details() {
    use innerlight::player::Decipher;
    let d = ScriptDecipher::new(&fixture()).expect("located");
    // The signature transform is one fixed position mapping: reversing the input reverses the mapping's picks.
    let distinct: String = (0x21u8..0x7f).map(char::from).collect();
    let out = d.signature(&distinct).unwrap();
    let pos: Vec<usize> = out.chars().map(|c| distinct.find(c).unwrap()).collect();
    let rev: String = distinct.chars().rev().collect();
    let want: String = pos.iter().map(|&p| rev.chars().nth(p).unwrap()).collect();
    assert_eq!(d.signature(&rev).unwrap(), want);
    eprintln!("signature positions (first 12 of {}): {:?}", pos.len(), &pos[..12]);
    // n: several inputs, all different, all valid, none the failure fallback
    let mut seen = std::collections::BTreeSet::new();
    for n in ["Qa3_9-XyZ0bk", "abcdefghijklmnop", "ZZZZZZZZZZZ", "0123456789a", "x_y-z_w-v_u-t"] {
        let o = d.throttle_value(n).unwrap();
        eprintln!("n {n} -> {o}");
        assert!(!o.is_empty() && o != n && !(o.len() > n.len() && o.ends_with(n)));
        assert!(seen.insert(o));
    }
    // through the trait, on URLs
    let url = "https://rr1---sn-abc.googlevideo.com/videoplayback?itag=251&n=Qa3_9-XyZ0bk&expire=1799999999";
    let fixed = d.throttle_fix(url).unwrap();
    assert!(
        fixed.starts_with("https://rr1---sn-abc.googlevideo.com/videoplayback?itag=251&n=")
            && fixed.ends_with("&expire=1799999999")
    );
    assert!(!fixed.contains("n=Qa3_9-XyZ0bk"));
    assert_eq!(d.throttle_fix("https://a.googlevideo.com/v?itag=1").unwrap(), "https://a.googlevideo.com/v?itag=1");
    assert!(d.decipher("ABCDEFGHIJ").is_ok());
}
