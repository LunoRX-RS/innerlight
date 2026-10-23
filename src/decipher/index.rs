//! Splits a script into named top-level "units" (`name=function(){...}`, `var name=...`, `g.name=...`) and builds
//! a bounded dependency closure from chosen root names. Only these units are ever evaluated, never the whole script.

use super::lexer::{Kind, Tok, lex};
use std::collections::{HashMap, HashSet};

/// One named top-level definition: a token range and the name it defines (`g.x` for a property of the wrapper's `g`).
#[derive(Debug, Clone)]
pub(crate) struct Unit {
    pub s: usize,
    pub e: usize,
    pub name: String,
    pub is_var: bool,
    pub decl_fn: bool,
}

pub(crate) struct Index<'a> {
    pub src: &'a str,
    pub toks: Vec<Tok>,
    pub units: Vec<Unit>,
    pub defs: HashMap<String, Vec<usize>>,
}

pub(crate) const MAX_PROGRAM_BYTES: usize = 3 * 1024 * 1024;

impl<'a> Index<'a> {
    pub fn new(src: &'a str) -> Self {
        let toks = lex(src);
        let mut idx = Index { src, toks, units: Vec::new(), defs: HashMap::new() };
        let n = idx.toks.len();
        idx.scan_container(0, n, 0);
        for (i, u) in idx.units.iter().enumerate() {
            idx.defs.entry(u.name.clone()).or_default().push(i);
        }
        idx
    }

    pub fn text(&self, t: usize) -> &'a str {
        let tok = self.toks[t];
        &self.src[tok.s as usize..tok.e as usize]
    }

    pub fn is(&self, t: usize, s: &str) -> bool {
        t < self.toks.len() && self.text(t) == s
    }

    pub fn is_kind(&self, t: usize, k: Kind) -> bool {
        t < self.toks.len() && self.toks[t].kind == k
    }

    pub fn is_id(&self, t: usize) -> bool {
        self.is_kind(t, Kind::Id)
    }

    /// Source text from token `s` to token `e - 1`.
    pub fn range_text(&self, s: usize, e: usize) -> &'a str {
        if s >= e || e > self.toks.len() {
            return "";
        }
        &self.src[self.toks[s].s as usize..self.toks[e - 1].e as usize]
    }

    /// Index of the token closing the bracket opened at `open`, if any.
    pub fn close_of(&self, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        for k in open..self.toks.len() {
            if self.toks[k].kind != Kind::Punct {
                continue;
            }
            match self.src.as_bytes()[self.toks[k].s as usize] {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(k);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Splits `toks[from..to]` into statements at depth 0 and records the units of each. An immediately invoked
    /// function expression is not a unit but a container: its body is scanned too (at most two levels deep).
    fn scan_container(&mut self, from: usize, to: usize, level: usize) {
        let mut stmt_start = from;
        let mut depth = 0usize;
        let mut k = from;
        while k < to {
            let t = self.toks[k];
            let mut end_stmt = false;
            if t.kind == Kind::Punct {
                match self.src.as_bytes()[t.s as usize] {
                    b'(' | b'[' | b'{' => depth += 1,
                    b')' | b']' => depth = depth.saturating_sub(1),
                    b'}' => {
                        depth = depth.saturating_sub(1);
                        // `function name(){...}` has no `;` after its body.
                        end_stmt = depth == 0 && self.is(stmt_start, "function") && self.is_id(stmt_start + 1);
                    }
                    b';' => end_stmt = depth == 0,
                    _ => {}
                }
            }
            if end_stmt {
                let e = if self.is(k, ";") { k } else { k + 1 };
                self.statement(stmt_start, e, level);
                stmt_start = k + 1;
            }
            k += 1;
        }
        if stmt_start < to {
            self.statement(stmt_start, to, level);
        }
    }

    fn statement(&mut self, s: usize, e: usize, level: usize) {
        if s >= e {
            return;
        }
        // Container: `(function(...){ body })(...)` or `!function(){...}()`.
        let f = if self.is(s, "(") || self.is(s, "!") { s + 1 } else { s };
        if level < 2 && self.is(f, "function") && !self.is_id(f + 1) {
            if let Some(po) = (f + 1 < e && self.is(f + 1, "(")).then_some(f + 1)
                && let Some(pc) = self.close_of(po)
                && self.is(pc + 1, "{")
                && let Some(bc) = self.close_of(pc + 1)
                && bc < e
            {
                self.scan_container(pc + 2, bc, level + 1);
            }
            return;
        }
        if self.is(s, "var") || self.is(s, "let") || self.is(s, "const") {
            for (ds, de) in self.split_commas(s + 1, e) {
                if self.is_id(ds) && self.is_assign(ds + 1) {
                    let name = self.text(ds).to_owned();
                    self.units.push(Unit { s: ds, e: de, name, is_var: true, decl_fn: false });
                }
            }
            return;
        }
        if self.is(s, "function") && self.is_id(s + 1) {
            let name = self.text(s + 1).to_owned();
            self.units.push(Unit { s, e, name, is_var: false, decl_fn: true });
            return;
        }
        // Expression statement: `a=...`, `g.a=...`, possibly several joined by commas.
        let mut cur: Option<usize> = None;
        for (ps, pe) in self.split_commas(s, e) {
            if let Some(name) = self.def_name(ps) {
                self.units.push(Unit { s: ps, e: pe, name, is_var: false, decl_fn: false });
                cur = Some(self.units.len() - 1);
            } else if let Some(c) = cur {
                self.units[c].e = pe;
            }
        }
    }

    /// `=` at `t` that is an assignment, not `==`, `=>` (tokens are single characters).
    fn is_assign(&self, t: usize) -> bool {
        self.is(t, "=") && !self.is(t + 1, "=") && !self.is(t + 1, ">")
    }

    fn def_name(&self, t: usize) -> Option<String> {
        if !self.is_id(t) {
            return None;
        }
        if self.is_assign(t + 1) {
            return Some(self.text(t).to_owned());
        }
        if self.text(t) == "g" && self.is(t + 1, ".") && self.is_id(t + 2) && self.is_assign(t + 3) {
            return Some(format!("g.{}", self.text(t + 2)));
        }
        None
    }

    /// Splits `toks[s..e]` at commas that are not inside brackets.
    fn split_commas(&self, s: usize, e: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut depth = 0usize;
        let mut start = s;
        for k in s..e.min(self.toks.len()) {
            if self.toks[k].kind != Kind::Punct {
                continue;
            }
            match self.src.as_bytes()[self.toks[k].s as usize] {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                b',' if depth == 0 => {
                    out.push((start, k));
                    start = k + 1;
                }
                _ => {}
            }
        }
        if start < e {
            out.push((start, e));
        }
        out
    }

    /// Names a unit refers to: identifiers not used as property names (`a.b` gives `a` only, except `g.b` gives
    /// `g.b`), skipping object keys and labels. An over-approximation: local variables that share a top-level
    /// name are included too, which only makes the closure larger.
    pub fn unit_refs(&self, ui: usize, out: &mut Vec<String>) {
        let u = &self.units[ui];
        for k in u.s..u.e {
            if self.toks[k].kind != Kind::Id {
                continue;
            }
            if k > u.s && self.is(k - 1, ".") {
                if k >= u.s + 2 && self.is(k - 2, "g") && !(k >= u.s + 3 && self.is(k - 3, ".")) {
                    out.push(format!("g.{}", self.text(k)));
                }
                continue;
            }
            if self.is(k + 1, ":") && k > u.s && (self.is(k - 1, "{") || self.is(k - 1, ",")) {
                continue;
            }
            out.push(self.text(k).to_owned());
        }
    }

    /// The unit that contains token `t`, if it is inside one.
    pub fn unit_at(&self, t: usize) -> Option<usize> {
        self.units.iter().position(|u| u.s <= t && t < u.e)
    }

    /// Source of one unit as it is evaluated: `var ` restored, a function declaration rewritten as an assignment.
    fn unit_code(&self, u: &Unit) -> String {
        if u.decl_fn {
            // `function name(...){...}` -> `name=function(...){...}`
            let rest = self.range_text(u.s + 2, u.e);
            return format!("{}=function{rest}", u.name);
        }
        let body = self.range_text(u.s, u.e);
        if u.is_var { format!("var {body}") } else { body.to_owned() }
    }

    /// Builds the program for `roots` plus everything they refer to within `depth` hops (depth 0 = the roots).
    /// Returns the program and the number of units, or `None` when a root is unknown or the program exceeds
    /// [`MAX_PROGRAM_BYTES`]. Units are emitted in script order, each guarded so one failing definition cannot
    /// stop the rest; the program is evaluated in strict mode like the original.
    pub fn closure_program(&self, roots: &[String], depth: usize) -> Option<(String, usize)> {
        let mut seen: HashSet<usize> = HashSet::new();
        let mut frontier: Vec<usize> = Vec::new();
        for r in roots {
            for &ui in self.defs.get(r)? {
                if seen.insert(ui) {
                    frontier.push(ui);
                }
            }
        }
        let mut refs = Vec::new();
        for _ in 0..depth {
            let mut next = Vec::new();
            for &ui in &frontier {
                refs.clear();
                self.unit_refs(ui, &mut refs);
                for r in &refs {
                    for &uj in self.defs.get(r).map(Vec::as_slice).unwrap_or(&[]) {
                        if seen.insert(uj) {
                            next.push(uj);
                        }
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        let mut order: Vec<usize> = seen.into_iter().collect();
        order.sort_unstable();
        let mut names: Vec<&str> =
            order.iter().map(|&i| self.units[i].name.as_str()).filter(|n| !n.contains('.')).collect();
        names.sort_unstable();
        names.dedup();
        let mut program = String::from("'use strict';var g={},window=this;");
        if !names.is_empty() {
            program.push_str("var ");
            program.push_str(&names.join(","));
            program.push(';');
        }
        for &ui in &order {
            let u = &self.units[ui];
            if u.name == "g" || u.name == "window" {
                continue;
            }
            program.push_str("\ntry{");
            program.push_str(&self.unit_code(u));
            program.push_str("}catch(_e){}");
            if program.len() > MAX_PROGRAM_BYTES {
                return None;
            }
        }
        Some((program, order.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_in_wrapper_and_plain() {
        let src = "var _p={};(function(g){var window=this;'use strict';var T,U;ba=\"a;b\".split(\";\");F=function(a){return ba[0]+a},G=3;\
                   function H(x){return x}\nvar K=1,L=F(2);g.M=function(){};N.prototype.q=1;J(1)})(_p);Z=5;";
        let idx = Index::new(src);
        let mut names: Vec<&str> = idx.units.iter().map(|u| u.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["F", "G", "H", "K", "L", "Z", "_p", "ba", "g.M", "window"]);
        let (prog, n) = idx.closure_program(&["F".to_owned()], 1).unwrap();
        assert_eq!(n, 2);
        assert!(prog.contains("F=function") && prog.contains("ba=\"a;b\""));
        assert!(idx.closure_program(&["nope".to_owned()], 3).is_none());
        let (prog, _) = idx.closure_program(&["H".to_owned()], 0).unwrap();
        assert!(prog.contains("H=function(x){return x}"));
    }

    #[test]
    fn garbage_does_not_panic() {
        for s in [
            "",
            "(",
            "(function(",
            "(function(){",
            "a=",
            "var",
            "var a=",
            "function",
            "function a(",
            "}}}",
            "g.",
            "g.x=",
            ",,,",
            "a=1,",
        ] {
            let idx = Index::new(s);
            let _ = idx.closure_program(&["a".to_owned()], 3);
        }
    }
}
