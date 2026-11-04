//! The heuristics and the verification step on hand-authored mini scripts in several structural styles.

mod decipher_common;
use decipher_common::*;
use innerlight::decipher::{DecipherError, ScriptDecipher, Which};
use innerlight::player::{Decipher, Player, PlayerError, Quality};
use serde_json::Value;

#[test]
fn variant_a_classic_helper_and_marker() {
    let d = decipher(SCRIPT_A);
    assert_eq!(d.found_by(), ("helper-function", "marker-in-function"));
    assert_eq!(d.signature(SIG_IN).unwrap(), ref_sig_a(SIG_IN));
    assert_eq!(d.signature("0123456789abcdef").unwrap(), ref_sig_a("0123456789abcdef"));
    assert_eq!(d.throttle_value("Qa3_9-XyZ0bk").unwrap(), ref_n_classic("Qa3_9-XyZ0bk"));
    assert_eq!(script(SCRIPT_A).signature_timestamp(), Ok(19900));
}

#[test]
fn variant_b_other_names_string_keys_splice_swap_declaration() {
    let d = decipher(SCRIPT_B);
    let (sig, n) = d.found_by();
    assert!(sig == "helper-function" || sig == "decode-call-site", "{sig}");
    assert_eq!(n, "get-n-call-site");
    assert_eq!(d.signature(SIG_IN).unwrap(), ref_sig_b(SIG_IN));
    assert_eq!(d.throttle_value("ThrottleN251").unwrap(), ref_n_plain("ThrottleN251"));
    assert_eq!(script(SCRIPT_B).signature_timestamp(), Ok(20123));
}

#[test]
fn variant_c_array_held_n_and_fromcharcode_anchor() {
    let d = decipher(SCRIPT_C);
    assert_eq!(d.found_by().1, "get-n-call-site");
    assert_eq!(d.throttle_value("abc-DEF_123").unwrap(), ref_n_plain("abc-DEF_123"));
    // reverse, splice(5), reverse
    let mut v: Vec<char> = SIG_IN.chars().collect();
    v.reverse();
    v.drain(..5);
    v.reverse();
    assert_eq!(d.signature(SIG_IN).unwrap(), v.into_iter().collect::<String>());
    assert_eq!(script(SCRIPT_C).signature_timestamp(), Ok(20999));
}

#[test]
fn variant_m_multiplexed_dispatcher() {
    let d = decipher(SCRIPT_M);
    assert_eq!(d.found_by(), ("helper-dispatcher", "marker-in-string-table"));
    assert_eq!(d.signature(SIG_IN).unwrap(), ref_sig_m(SIG_IN));
    assert_eq!(d.throttle_value("ThrottleN251").unwrap(), ref_n_m("ThrottleN251", 1300));
    assert_eq!(script(SCRIPT_M).signature_timestamp(), Ok(20500));
}

#[test]
fn variant_n_selector_reached_only_through_computed_arguments() {
    let d = decipher(SCRIPT_N);
    assert_eq!(d.found_by(), ("helper-dispatcher", "marker-in-string-table"));
    assert_eq!(d.signature(SIG_IN).unwrap(), ref_sig_m(SIG_IN));
    assert_eq!(d.throttle_value("ThrottleN251").unwrap(), ref_n_n("ThrottleN251"));
    assert_eq!(script(SCRIPT_N).signature_timestamp(), Ok(20600));
}

#[test]
fn variant_m_brute_force_when_no_marker() {
    let js = script_m_without_marker();
    let d = decipher(&js);
    assert_eq!(d.found_by().1, "brute-force");
    // Any b makes this toy dispatcher "work"; what matters is that the result is the real branch output.
    let out = d.throttle_value("ThrottleN251").unwrap();
    assert!((0..4096u32).any(|b| ref_n_m("ThrottleN251", b) == out));
}

#[test]
fn throttle_url_rewrites_only_n() {
    let d = decipher(SCRIPT_A);
    let url = "https://rr1---sn-abc.googlevideo.com/videoplayback?expire=1&ei=X%2BY&n=Qa3_9-XyZ0bk&itag=251&sig=a%3D";
    let fixed = d.throttle_url(url).unwrap();
    assert_eq!(
        fixed,
        format!(
            "https://rr1---sn-abc.googlevideo.com/videoplayback?expire=1&ei=X%2BY&n={}&itag=251&sig=a%3D",
            ref_n_classic("Qa3_9-XyZ0bk")
        )
    );
    // percent-encoded n is decoded first
    let enc = "https://a.googlevideo.com/v?n=Qa3_9-XyZ0bk%2D&itag=1";
    assert_eq!(
        d.throttle_url(enc).unwrap(),
        format!("https://a.googlevideo.com/v?n={}&itag=1", ref_n_classic("Qa3_9-XyZ0bk-"))
    );
    // no n: untouched, still Ok
    let plain = "https://a.googlevideo.com/videoplayback?itag=251&expire=9";
    assert_eq!(d.throttle_url(plain).unwrap(), plain);
    // a hostile URL never comes out
    assert_eq!(d.throttle_url("https://evil.example/v?n=Qa3_9"), Err(DecipherError::UnsafeUrl));
    assert_eq!(d.throttle_url("https://evil.example/v?itag=1"), Err(DecipherError::UnsafeUrl));
    assert_eq!(d.throttle_url("not a url"), Err(DecipherError::BadInput));
    assert_eq!(d.throttle_url("https://a.googlevideo.com/v?n="), Err(DecipherError::BadInput));
}

#[test]
fn input_validation() {
    let d = decipher(SCRIPT_A);
    assert_eq!(d.signature(""), Err(DecipherError::BadInput));
    assert_eq!(d.signature(&"A".repeat(5000)), Err(DecipherError::BadInput));
    assert_eq!(d.signature("AB\nCD"), Err(DecipherError::BadInput));
    assert_eq!(d.throttle_value(""), Err(DecipherError::BadInput));
    assert_eq!(d.throttle_value("a'b"), Err(DecipherError::BadInput));
    assert_eq!(d.throttle_value(&"a".repeat(513)), Err(DecipherError::BadInput));
    // too short for the descrambler (everything spliced away): an empty result is an error, not a silent value
    assert_eq!(d.signature("AB"), Err(DecipherError::BadOutput));
}

#[test]
fn rejected_when_structure_matches_but_behaviour_does_not() {
    // helper and entry exist but the entry does nothing
    let identity =
        SCRIPT_A.replace("Zq.Pm(a,63);Zq.xK(a,2);Zq.Ld(a,12);Zq.Pm(a,42);Zq.xK(a,1);", "Zq.Pm(a,1);Zq.Pm(a,1);");
    assert_eq!(
        ScriptDecipher::new(&script(&identity)).unwrap_err(),
        DecipherError::ScriptShapeChanged { which: Which::Signature }
    );
    // n function that always hits its failure fallback
    let failing = SCRIPT_A.replace("if(b.length<2)throw 1", "throw 1");
    assert_eq!(
        ScriptDecipher::new(&script(&failing)).unwrap_err(),
        DecipherError::ScriptShapeChanged { which: Which::Throttle }
    );
    // n function that returns its input
    let same = SCRIPT_A.replace("b[d]=String.fromCharCode(97+(e+c[d%3])%26)", "b[d]=b[d]").replace("b.reverse();", "");
    assert_eq!(
        ScriptDecipher::new(&script(&same)).unwrap_err(),
        DecipherError::ScriptShapeChanged { which: Which::Throttle }
    );
    // nothing at all
    for js in ["var a=1;", "function f(a){return a}", SCRIPT_A.split("var Nf").next().unwrap()] {
        let e = ScriptDecipher::new(&script(js)).unwrap_err();
        assert!(matches!(e, DecipherError::ScriptShapeChanged { .. }), "{e:?}");
    }
    // the signature entry reaches for the network: sandbox has none, the candidate is rejected
    let net =
        SCRIPT_A.replace("a=a.split(\"\");Zq.Pm(a,63)", "fetch('https://example.com');a=a.split(\"\");Zq.Pm(a,63)");
    assert_eq!(
        ScriptDecipher::new(&script(&net)).unwrap_err(),
        DecipherError::ScriptShapeChanged { which: Which::Signature }
    );
}

#[test]
fn result_cannot_smuggle_characters() {
    // a "descrambler" that appends a character the input never had
    let evil = SCRIPT_A.replace("return a.join(\"\")};\nvar Nf", "return a.join(\"\")+\"Z\"};\nvar Nf");
    let r = ScriptDecipher::new(&script(&evil));
    // the probe sees a result that is not a permutation of the input, so the candidate is rejected
    assert_eq!(r.unwrap_err(), DecipherError::ScriptShapeChanged { which: Which::Signature });
}

#[tokio::test]
async fn plugs_into_player_resolve() {
    let d = std::sync::Arc::new(decipher(SCRIPT_A));
    let json: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/player_handwritten_ok_ciphered.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let client = innerlight::Client::builder().build().unwrap();
    let r = Player::new(&client).with_decipher(d.clone()).resolve_response(&json, Quality::High).expect("resolves");
    // s=ABCDEFGHIJ -> reverse, drop 2, swap(12 % 8), reverse, drop 1
    assert!(r.url.starts_with("https://rr1---sn-hw4g5e6z.googlevideo.com/videoplayback?"), "{}", r.url);
    assert!(r.url.contains(&format!("&sig={}", ref_sig_a("ABCDEFGHIJ"))), "{}", r.url);
    assert!(r.url.contains(&format!("n={}", ref_n_classic("ThrottleN251"))), "{}", r.url);
    assert!(!r.needs_throttle_fix);
    // Decipher trait errors map to PlayerError
    assert_eq!(d.decipher("").unwrap_err(), PlayerError::DecipherFailed);
    assert_eq!(d.throttle_fix("https://evil.example/").unwrap_err(), PlayerError::UnsafeUrl);
}
