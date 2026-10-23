//! LIVE, manual, off by default. Two anonymous GETs (the public loader and the public player script), no `player`
//! request, no retry, nothing but the script is evaluated, in the sandbox.
//!
//! A human runs it deliberately:
//!
//! ```text
//! INNERLIGHT_LIVE=1 cargo test --test decipher_live -- --ignored --nocapture
//! ```
//!
//! Prints the player version, `signatureTimestamp`, which heuristics matched and whether both transforms ran.
//! It does not prove that the service accepts a deciphered URL.

use innerlight::Client;
use innerlight::decipher::{PlayerScript, ScriptDecipher};

#[tokio::test]
#[ignore = "talks to the real service; run deliberately, see the file header"]
async fn live_script_both_extractors() {
    if std::env::var("INNERLIGHT_LIVE").as_deref() != Ok("1") {
        println!("skipped: set INNERLIGHT_LIVE=1 to download the current player script");
        return;
    }
    let client = Client::builder().build().unwrap();
    let script = PlayerScript::fetch_current(&client).await.expect("current player script");
    println!("player version {} ({} bytes)", script.version(), script.len());
    println!("signatureTimestamp: {:?}", script.signature_timestamp());
    let d = ScriptDecipher::new(&script).expect("both transforms located and verified");
    println!("found by: {:?}", d.found_by());
    let sig: String =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_".chars().cycle().take(100).collect();
    println!("signature transform: {:?} chars in, {:?} out", sig.len(), d.signature(&sig).map(|s| s.len()));
    println!("n transform: {:?}", d.throttle_value("Qa3_9-XyZ0bk"));
}
