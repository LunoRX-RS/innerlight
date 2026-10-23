//! Turns heuristic candidates into a verified, ready-to-call transform.
//!
//! For every dependency depth, 0, 1, 2, ..., the roots of all candidates plus what they refer to are loaded into one
//! sandbox and each candidate is probed with synthetic inputs. A candidate is accepted only if it *behaves* like the
//! transform. If none does at any depth, the caller gets `ScriptShapeChanged`; the input is never returned unchanged.

use super::index::Index;
use super::locate::{BruteTarget, Candidate};
use super::sandbox::{Sandbox, SandboxLimits};
use super::{DecipherError, Which};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) const MAX_DEPTH: usize = 12;
const BRUTE_MAX_B: u64 = 4096;
const BRUTE_BUDGET: Duration = Duration::from_secs(20);

/// A transform that passed its probes.
#[derive(Clone)]
pub(crate) struct Resolved {
    pub sandbox: Arc<Sandbox>,
    pub template: String,
    /// Which heuristic found it (for tests and logs).
    pub how: &'static str,
}

/// 94 distinct printable characters.
fn alphabet() -> String {
    (0x21u8..0x7f).map(char::from).collect()
}

pub(crate) fn sig_probe(sb: &Sandbox, template: &str) -> bool {
    let alpha = alphabet();
    let Ok(out) = sb.call(template, &alpha) else { return false };
    let chars: Vec<char> = out.chars().collect();
    let distinct: BTreeSet<char> = chars.iter().copied().collect();
    if chars.is_empty() || chars.len() > alpha.len() || distinct.len() != chars.len() || out == alpha {
        return false;
    }
    let Some(pos) = chars.iter().map(|c| alpha.find(*c)).collect::<Option<Vec<usize>>>() else { return false };
    // A real descrambler is one fixed position mapping: apply it to a second input and compare.
    let reversed: Vec<char> = alpha.chars().rev().collect();
    let expect: String = pos.iter().map(|&p| reversed[p]).collect();
    let rev_in: String = reversed.iter().collect();
    sb.call(template, &rev_in).is_ok_and(|o| o == expect) && sb.call(template, &alpha).is_ok_and(|o| o == out)
}

pub(crate) fn n_probe(sb: &Sandbox, template: &str) -> bool {
    let x1 = "Qa3_9-XyZ0bk";
    let x2 = "abcdefghijklmnop";
    let mut outs = Vec::new();
    for x in [x1, x2] {
        let Ok(o) = sb.call(template, x) else { return false };
        if check_n_output(x, &o).is_err() || sb.call(template, x).map_or(true, |again| again != o) {
            return false;
        }
        outs.push(o);
    }
    outs[0] != outs[1]
}

/// Validates the result of the `n` transform against its input.
pub(crate) fn check_n_output(input: &str, out: &str) -> Result<(), DecipherError> {
    let charset_ok = out.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'~'));
    // The player's failure fallback is "<marker>" + input: longer than, and ending with, the input.
    let failure_shape = out.len() > input.len() && out.ends_with(input);
    if out.is_empty() || out.len() > 1024 || !charset_ok || out == input || failure_shape {
        return Err(DecipherError::BadOutput);
    }
    Ok(())
}

type Probe = fn(&Sandbox, &str) -> bool;

fn probe_for(which: Which) -> Probe {
    match which {
        Which::Signature => sig_probe,
        _ => n_probe,
    }
}

pub(crate) fn resolve(
    idx: &Index,
    which: Which,
    cands: &[Candidate],
    brute: &[BruteTarget],
    limits: SandboxLimits,
) -> Result<Resolved, DecipherError> {
    let fail = DecipherError::ScriptShapeChanged { which };
    let probe = probe_for(which);
    let cands: Vec<&Candidate> = cands.iter().filter(|c| c.roots.iter().all(|r| idx.defs.contains_key(r))).collect();
    let mut roots: Vec<String> = cands.iter().flat_map(|c| c.roots.iter().cloned()).collect();
    roots.extend(brute.iter().map(|b| b.root.clone()));
    roots.sort();
    roots.dedup();
    if roots.is_empty() {
        return Err(fail);
    }
    let mut last: Option<Arc<Sandbox>> = None;
    let mut prev_units = 0usize;
    for depth in 0..=MAX_DEPTH {
        let Some((program, units)) = idx.closure_program(&roots, depth) else { break };
        if depth > 0 && units == prev_units {
            break; // the closure stopped growing
        }
        prev_units = units;
        if std::env::var_os("INNERLIGHT_DECIPHER_DEBUG").is_some() {
            eprintln!(
                "[decipher] {which}: depth {depth}, {units} units, {} bytes, {} candidates",
                program.len(),
                cands.len()
            );
        }
        let Ok(sb) = Sandbox::new(&program, limits) else { continue };
        let sb = Arc::new(sb);
        for c in &cands {
            if probe(&sb, &c.template) {
                return Ok(Resolved { sandbox: sb, template: c.template.clone(), how: c.how });
            }
        }
        last = Some(sb);
    }
    if let (Some(sb), Which::Throttle) = (last, which) {
        let started = Instant::now();
        for t in brute {
            for &a in &t.selectors {
                for b in 0..BRUTE_MAX_B {
                    if started.elapsed() > BRUTE_BUDGET {
                        return Err(fail);
                    }
                    let template = format!("{}({a},{b},__a)", t.root);
                    if probe(&sb, &template) {
                        return Ok(Resolved { sandbox: sb, template, how: "brute-force" });
                    }
                }
            }
        }
    }
    Err(fail)
}
