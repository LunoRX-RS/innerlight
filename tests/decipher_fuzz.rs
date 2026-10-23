//! No script, however broken, makes the decipher module panic or hang; it ends in a typed error or a value.

mod decipher_common;
use decipher_common::*;
use innerlight::decipher::{PlayerScript, SandboxLimits, ScriptDecipher};
use std::time::Duration;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn tight() -> SandboxLimits {
    SandboxLimits {
        call_timeout: Duration::from_millis(100),
        load_timeout: Duration::from_secs(5),
        loop_iterations: 20_000,
        ..SandboxLimits::default()
    }
}

fn run(js: &str) {
    if let Ok(s) = PlayerScript::from_source("fuzz1234".into(), js.to_owned()) {
        let _ = s.signature_timestamp();
        if let Ok(d) = ScriptDecipher::with_limits(&s, tight()) {
            let _ = d.signature("ABCDEFGHIJKLMNOP");
            let _ = d.throttle_value("abcdefghijk");
            let _ = d.throttle_url("https://a.googlevideo.com/v?n=abc");
        }
    }
}

#[test]
fn garbage_scripts() {
    let cases = [
        "",
        " ",
        "(",
        ")",
        "{",
        "}",
        "{{{{",
        "}}}}",
        "(function(",
        "(function(){",
        "(function(){}",
        "(function(g){var a=",
        "var",
        "var a=",
        "a=",
        "g.",
        "g.a=",
        "function",
        "function a",
        "function a(",
        "function a(){",
        "'",
        "\"",
        "`",
        "/*",
        "//",
        "/",
        "/[",
        "\\",
        "\u{0}",
        "\u{feff}",
        "é",
        "😀=1",
        "a=function(",
        "a={b:function(",
        "a={b:function(){},c:function(){}}",
        "a={b:function(a){a.reverse()},c:function(a,b){a.splice(0,b)}};function f(a){a.b(1);a.b(1)}",
        "x=\"a;b\".split(\";\");",
        "x=\"a;b\".split(\"\");",
        "T=\"1ZzQ9xYw_w8_\".split(\";\");D=function(a,b,c){var e=b^a;try{}catch(x){return T[e^1]+c}};D(1,2,3)",
        "get(\"n\")",
        ".get(\"n\")",
        ".get(\"n\")f(a)",
        "x.get(\"n\");y(z)",
        "sp.sp.sp",
        ".sp(1,2,Q(1,2,",
        "decodeURIComponent(",
        "(decodeURIComponent(",
        "f(decodeURIComponent(c))",
        "signatureTimestamp",
        "signatureTimestamp:",
        "signatureTimestamp:99999999999999999999",
        "sts:",
        "sts:1",
    ];
    for c in cases {
        run(c);
    }
    run(&"(".repeat(100_000));
    run(&"a=function(){".repeat(10_000));
    run(&"{a:function(a){a.reverse()},".repeat(2_000));
    run(&format!("a=\"{}\".split(\";\");", ";".repeat(50_000)));
    run(&"x".repeat(1_000_000));
}

#[test]
fn mutated_scripts() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for base in [SCRIPT_A, SCRIPT_B, SCRIPT_C, SCRIPT_M] {
        for _ in 0..60 {
            let mut b = base.as_bytes().to_vec();
            for _ in 0..(1 + rng.next() % 6) {
                let i = (rng.next() as usize) % b.len();
                match rng.next() % 4 {
                    0 => b[i] = b"(){}[];,.=^%\"'/+"[(rng.next() % 16) as usize],
                    1 => {
                        b.remove(i);
                    }
                    2 => b.insert(i, b'a' + (rng.next() % 26) as u8),
                    _ => b.truncate(i.max(1)),
                }
            }
            run(&String::from_utf8_lossy(&b));
        }
    }
}

#[test]
fn truncated_real_script() {
    let js =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/player_js_cbcfbf2f.js")).unwrap();
    let mut rng = Rng(42);
    for _ in 0..6 {
        let mut cut = (rng.next() as usize) % 400_000;
        while !js.is_char_boundary(cut) {
            cut -= 1;
        }
        run(&js[..cut]);
    }
}
