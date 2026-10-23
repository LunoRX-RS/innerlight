//! Structural heuristics that propose where the two transforms live. They only *propose*: every candidate is run
//! in the sandbox and kept only if it behaves like the transform (see `plan.rs`). Names are never hard-coded.
//!
//! Signature descrambler, as seen in shipped players:
//! * classic: `NAME=function(a){a=a.split("");H.xx(a,3);H.yy(a,2);return a.join("")}` with a helper object `H` whose
//!   members reverse, splice and swap the array;
//! * multiplexed: one dispatcher function `D(a,b,c,d)` computing `e=b^a` and selecting behaviour by `a`, reaching `H`
//!   and a string table through `ba[e^123]`-style lookups; called as `D(16,1453,s)`.
//!
//! `n` transform:
//! * classic: a function with a `try{...}catch(d){return"enhanced_except_..._w8_"+a}` fallback;
//! * multiplexed: the same dispatcher family, selected by the failure marker living in the string table.
//!
//! To add a heuristic, write a function returning `Vec<Candidate>` and call it from `signature_candidates` /
//! `throttle_candidates`; see `docs/DECIPHER.md`.

use super::index::Index;
use super::lexer::Kind;
use std::collections::BTreeSet;

/// One proposal: the unit names to load and the call expression (`__a` stands for the argument).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub roots: Vec<String>,
    pub template: String,
    pub how: &'static str,
}

/// A dispatcher to brute-force when no heuristic named the exact arguments.
#[derive(Debug, Clone)]
pub(crate) struct BruteTarget {
    pub root: String,
    pub selectors: Vec<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    Reverse,
    Splice,
    Swap,
}

fn balanced(s: &str) -> bool {
    let mut d = 0i32;
    for c in s.chars() {
        match c {
            '[' | '(' | '{' => d += 1,
            ']' | ')' | '}' => {
                d -= 1;
                if d < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    d == 0
}

fn classify_op(params: &[&str], body: &str) -> Option<Op> {
    let a = *params.first()?;
    let rest = body.strip_prefix(a);
    if params.len() == 1 {
        let rest = rest?;
        let reverse = rest == ".reverse()"
            || (rest.starts_with('[') && rest.ends_with("]()") && balanced(&rest[1..rest.len() - 3]));
        return reverse.then_some(Op::Reverse);
    }
    let b = *params.get(1)?;
    if let Some(rest) = rest {
        let tail = format!("(0,{b})");
        let splice = rest == format!(".splice{tail}")
            || (rest.starts_with('[')
                && rest.ends_with(&format!("]{tail}"))
                && balanced(&rest[1..rest.len() - 1 - tail.len()]));
        if splice {
            return Some(Op::Splice);
        }
    }
    (body.contains(&format!("{b}%{a}")) && body.contains(&format!("{a}[0]"))).then_some(Op::Swap)
}

/// Names of objects shaped like the signature helper: 2 to 6 function members, each a reverse, splice or swap of
/// its first argument, with at least two kinds present.
fn helper_objects(idx: &Index) -> Vec<String> {
    let mut out = Vec::new();
    for i in 2..idx.toks.len() {
        if !idx.is(i, "{") || !(idx.is(i - 1, "=")) || !idx.is_id(i - 2) {
            continue;
        }
        // cheap pre-check before parsing the members: `{key:function(`
        if !((idx.is_id(i + 1) || idx.is_kind(i + 1, Kind::Str)) && idx.is(i + 2, ":") && idx.is(i + 3, "function")) {
            continue;
        }
        if let Some(kinds) = parse_members(idx, i) {
            let distinct: BTreeSet<u8> = kinds.iter().map(|k| *k as u8).collect();
            if (2..=6).contains(&kinds.len()) && distinct.len() >= 2 {
                out.push(idx.text(i - 2).to_owned());
            }
        }
    }
    out
}

fn parse_members(idx: &Index, open: usize) -> Option<Vec<Op>> {
    let mut kinds = Vec::new();
    let mut k = open + 1;
    loop {
        if !(idx.is_id(k) || idx.is_kind(k, Kind::Str))
            || !idx.is(k + 1, ":")
            || !idx.is(k + 2, "function")
            || !idx.is(k + 3, "(")
        {
            return None;
        }
        let pc = idx.close_of(k + 3)?;
        if !idx.is(pc + 1, "{") {
            return None;
        }
        let bc = idx.close_of(pc + 1)?;
        let params: Vec<&str> = (k + 4..pc).filter(|&t| idx.is_id(t)).map(|t| idx.text(t)).collect();
        let body: String = (pc + 2..bc).map(|t| idx.text(t)).collect();
        kinds.push(classify_op(&params, &body)?);
        k = bc + 1;
        if idx.is(k, ",") {
            k += 1;
        } else if idx.is(k, "}") {
            return Some(kinds);
        } else {
            return None;
        }
    }
}

/// Call sites `name(NUM,NUM,` as (first, second) argument text, deduplicated, in script order.
fn call_pairs(idx: &Index, name: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for i in 0..idx.toks.len().saturating_sub(5) {
        if idx.is_id(i)
            && idx.text(i) == name
            && idx.is(i + 1, "(")
            && idx.is_kind(i + 2, Kind::Num)
            && idx.is(i + 3, ",")
            && idx.is_kind(i + 4, Kind::Num)
            && idx.is(i + 5, ",")
        {
            let p = (idx.text(i + 2).to_owned(), idx.text(i + 4).to_owned());
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// First arguments of calls `name(NUM,` whatever follows, sorted.
fn selectors(idx: &Index, name: &str) -> Vec<u32> {
    let mut set = BTreeSet::new();
    for i in 0..idx.toks.len().saturating_sub(3) {
        if idx.is_id(i)
            && idx.text(i) == name
            && idx.is(i + 1, "(")
            && idx.is_kind(i + 2, Kind::Num)
            && idx.is(i + 3, ",")
            && let Ok(n) = idx.text(i + 2).parse::<u32>()
        {
            set.insert(n);
        }
    }
    set.into_iter().collect()
}

fn is_global_unit(idx: &Index, name: &str) -> bool {
    idx.defs.contains_key(name) && !name.contains('.')
}

fn push(out: &mut Vec<Candidate>, roots: &[&str], template: String, how: &'static str) {
    let c = Candidate { roots: roots.iter().map(|s| (*s).to_owned()).collect(), template, how };
    if !out.contains(&c) {
        out.push(c);
    }
}

pub(crate) fn signature_candidates(idx: &Index) -> Vec<Candidate> {
    let mut out = Vec::new();

    // S1: units that use a helper object at least twice (the descrambler, or the dispatcher that contains it).
    for h in helper_objects(idx) {
        for u in &idx.units {
            if u.name == h || u.name.contains('.') {
                continue;
            }
            let uses = (u.s..u.e)
                .filter(|&t| idx.is_id(t) && idx.text(t) == h && (idx.is(t + 1, ".") || idx.is(t + 1, "[")))
                .count();
            if uses >= 2 {
                push(&mut out, &[&u.name], format!("{}(__a)", u.name), "helper-function");
                for (a, b) in call_pairs(idx, &u.name) {
                    push(&mut out, &[&u.name], format!("{}({a},{b},__a)", u.name), "helper-dispatcher");
                }
            }
        }
    }

    // S2: the call site that hands `decodeURIComponent(s)` to the descrambler.
    for i in 2..idx.toks.len() {
        if !(idx.is_id(i) && idx.text(i) == "decodeURIComponent" && idx.is(i - 1, "(")) {
            continue;
        }
        if idx.is_id(i - 2) {
            let x = idx.text(i - 2);
            if !matches!(x, "encodeURIComponent" | "decodeURIComponent" | "escape" | "unescape")
                && is_global_unit(idx, x)
            {
                push(&mut out, &[x], format!("{x}(__a)"), "decode-call-site");
            }
        } else if i >= 5
            && idx.is(i - 2, "]")
            && idx.is_kind(i - 3, Kind::Num)
            && idx.is(i - 4, "[")
            && idx.is_id(i - 5)
        {
            let x = idx.text(i - 5);
            if is_global_unit(idx, x) {
                push(&mut out, &[x], format!("{x}[{}](__a)", idx.text(i - 3)), "decode-call-site");
            }
        }
    }

    // S3: the call site that reads the `sp` parameter name and then applies `NAME(A,B,NAME2(C,D,s))`.
    for i in 1..idx.toks.len() {
        if !(idx.is_id(i) && idx.text(i) == "sp" && idx.is(i - 1, ".")) {
            continue;
        }
        for j in i + 1..(i + 60).min(idx.toks.len().saturating_sub(10)) {
            if idx.is_id(j)
                && idx.is(j + 1, "(")
                && idx.is_kind(j + 2, Kind::Num)
                && idx.is(j + 3, ",")
                && idx.is_kind(j + 4, Kind::Num)
                && idx.is(j + 5, ",")
                && idx.is_id(j + 6)
                && idx.is(j + 7, "(")
            {
                let n = idx.text(j);
                if is_global_unit(idx, n) {
                    push(&mut out, &[n], format!("{n}({},{},__a)", idx.text(j + 2), idx.text(j + 4)), "sp-call-site");
                }
                break;
            }
        }
    }
    out
}

fn is_failure_marker(s: &str) -> bool {
    let id_chars = |t: &str| t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    (s.starts_with("enhanced_except_") && s.ends_with('_') && id_chars(s))
        || (s.len() >= 12 && s.ends_with("_w8_") && id_chars(s))
}

fn unquote(t: &str) -> &str {
    t.get(1..t.len().saturating_sub(1)).unwrap_or("")
}

/// `NAME="a;b;c".split(";")` string tables: (name, entries).
fn string_tables(idx: &Index) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for u in &idx.units {
        let s = u.s;
        if idx.is(s + 1, "=")
            && idx.is_kind(s + 2, Kind::Str)
            && idx.is(s + 3, ".")
            && idx.is(s + 4, "split")
            && idx.is(s + 5, "(")
            && idx.is_kind(s + 6, Kind::Str)
        {
            let sep = unquote(idx.text(s + 6));
            if !sep.is_empty() && !sep.starts_with('\\') {
                out.push((u.name.clone(), unquote(idx.text(s + 2)).split(sep).map(str::to_owned).collect()));
            }
        }
    }
    out
}

pub(crate) fn throttle_candidates(idx: &Index) -> (Vec<Candidate>, Vec<BruteTarget>) {
    let mut out = Vec::new();

    // N1: a failure-marker string literal inside a function unit: that unit is the transform.
    for t in 0..idx.toks.len() {
        if idx.is_kind(t, Kind::Str)
            && is_failure_marker(unquote(idx.text(t)))
            && let Some(ui) = idx.unit_at(t)
        {
            let u = &idx.units[ui];
            if !u.name.contains('.') && (u.s..u.e).any(|k| idx.text(k) == "function") {
                push(&mut out, &[&u.name], format!("{}(__a)", u.name), "marker-in-function");
            }
        }
    }

    // N2: the marker lives in a string table and a dispatcher reads it as `T[e^K]+x`; K ^ index gives `e = b ^ a`.
    for (table, entries) in string_tables(idx) {
        let marks: Vec<u64> =
            entries.iter().enumerate().filter(|(_, e)| is_failure_marker(e)).map(|(i, _)| i as u64).collect();
        if marks.is_empty() {
            continue;
        }
        for i in 0..idx.toks.len().saturating_sub(7) {
            if !(idx.is_id(i)
                && idx.text(i) == table
                && idx.is(i + 1, "[")
                && idx.is(i + 3, "^")
                && idx.is(i + 5, "]")
                && idx.is(i + 6, "+"))
            {
                continue;
            }
            let k = if idx.is_kind(i + 4, Kind::Num) && idx.is_id(i + 2) {
                idx.text(i + 4)
            } else if idx.is_kind(i + 2, Kind::Num) && idx.is_id(i + 4) {
                idx.text(i + 2)
            } else {
                continue;
            };
            let Ok(k) = k.parse::<u64>() else { continue };
            let Some(ui) = idx.unit_at(i) else { continue };
            let d = idx.units[ui].name.clone();
            if d.contains('.') {
                continue;
            }
            let mut sels = selectors(idx, &d);
            if !sels.contains(&1) {
                sels.push(1);
            }
            for &m in &marks {
                let e = k ^ m;
                for &a in &sels {
                    push(&mut out, &[&d], format!("{d}({a},{},__a)", u64::from(a) ^ e), "marker-in-string-table");
                }
            }
        }
    }

    // N3: the site that reads the `n` parameter (`.get("n")` / `.get(String.fromCharCode(110))`) and calls a function on it.
    let mut anchors = 0;
    for i in 3..idx.toks.len().saturating_sub(4) {
        let is_get = idx.text(i - 2) == "get" && idx.is(i - 1, "(") && idx.is(i - 3, ".");
        let n_arg = idx.is_kind(i, Kind::Str) && matches!(idx.text(i), "\"n\"" | "'n'");
        let from_code = idx.is_id(i)
            && idx.text(i) == "String"
            && idx.is(i + 1, ".")
            && idx.text(i + 2) == "fromCharCode"
            && idx.is(i + 4, "110");
        if !(is_get && (n_arg || from_code)) {
            continue;
        }
        anchors += 1;
        if anchors > 12 {
            break;
        }
        let mut found = 0;
        for j in i..(i + 48).min(idx.toks.len().saturating_sub(5)) {
            if found >= 3 {
                break;
            }
            let plain = idx.is_id(j) && idx.is(j + 1, "(") && idx.is_id(j + 2) && idx.is(j + 3, ")");
            let indexed = idx.is_id(j)
                && idx.is(j + 1, "[")
                && idx.is_kind(j + 2, Kind::Num)
                && idx.is(j + 3, "]")
                && idx.is(j + 4, "(");
            let x = idx.text(j);
            if (plain || indexed)
                && is_global_unit(idx, x)
                && !matches!(x, "get" | "set" | "String" | "Number" | "decodeURIComponent" | "encodeURIComponent")
            {
                let t = if plain { format!("{x}(__a)") } else { format!("{x}[{}](__a)", idx.text(j + 2)) };
                push(&mut out, &[x], t, "get-n-call-site");
                found += 1;
            }
        }
    }

    // N4: shape: a sizeable function with split, join, try and catch.
    let mut shaped: Vec<(usize, &str)> = Vec::new();
    for u in &idx.units {
        if u.name.contains('.') {
            continue;
        }
        let words: BTreeSet<&str> = (u.s..u.e).filter(|&t| idx.is_id(t)).map(|t| idx.text(t)).collect();
        let size = idx.range_text(u.s, u.e).len();
        if (300..60_000).contains(&size)
            && words.contains("function")
            && ["split", "join", "try", "catch"].iter().all(|w| words.contains(w))
        {
            shaped.push((size, &u.name));
        }
    }
    shaped.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (_, name) in shaped.into_iter().take(6) {
        push(&mut out, &[name], format!("{name}(__a)"), "split-join-try-shape");
    }

    // Brute-force targets: multiplexed dispatchers `function(a,b,..){var e=b^a;` that contain a `try`.
    let mut brute = Vec::new();
    for u in &idx.units {
        if u.name.contains('.') || !idx.is(u.s + 1, "=") || !idx.is(u.s + 2, "function") || !idx.is(u.s + 3, "(") {
            continue;
        }
        let Some(pc) = idx.close_of(u.s + 3) else { continue };
        let params: Vec<&str> = (u.s + 4..pc).filter(|&t| idx.is_id(t)).map(|t| idx.text(t)).collect();
        if params.len() < 3 || !idx.is(pc + 1, "{") {
            continue;
        }
        let b = pc + 2;
        let shape = idx.is(b, "var")
            && idx.is_id(b + 1)
            && idx.is(b + 2, "=")
            && idx.is(b + 3, params[1])
            && idx.is(b + 4, "^")
            && idx.is(b + 5, params[0]);
        if shape && (u.s..u.e).any(|t| idx.is_id(t) && idx.text(t) == "try") {
            let sels = selectors(idx, &u.name);
            if !sels.is_empty() {
                brute.push(BruteTarget { root: u.name.clone(), selectors: sels });
            }
        }
    }
    (out, brute)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify() {
        assert_eq!(classify_op(&["a"], "a.reverse()"), Some(Op::Reverse));
        assert_eq!(classify_op(&["a"], "a[ba[31]]()"), Some(Op::Reverse));
        assert_eq!(classify_op(&["a", "b"], "a.splice(0,b)"), Some(Op::Splice));
        assert_eq!(classify_op(&["a", "b"], "a[ba[2]](0,b)"), Some(Op::Splice));
        assert_eq!(classify_op(&["a", "b"], "varc=a[0];a[0]=a[b%a.length];a[b%a.length]=c"), Some(Op::Swap));
        assert_eq!(classify_op(&["a", "b"], "a.splice(0,1,a.splice(b%a.length,1,a[0])[0])"), Some(Op::Swap));
        assert_eq!(classify_op(&["a"], "a.push(1)"), None);
        assert_eq!(classify_op(&[], "x"), None);
    }

    #[test]
    fn marker_shapes() {
        assert!(is_failure_marker("enhanced_except_pgQ0APH-_w8_"));
        assert!(is_failure_marker("1MSRXuYDuYciqIB9d3-_w8_"));
        assert!(!is_failure_marker("local"));
        assert!(!is_failure_marker("enhanced_except_x y_"));
    }
}
