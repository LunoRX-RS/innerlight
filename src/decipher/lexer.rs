//! A small JavaScript tokenizer, just good enough to find statement boundaries, bracket matches and identifiers in a
//! minified script. It never panics and always advances; on odd input it produces odd tokens, not errors.
//!
//! Known limits: a regular expression directly after `}` is read as a division; a template literal that nests
//! another template inside `${}` ends early. Neither occurs in the scripts this crate targets, and both only
//! produce a wrong boundary (the verification step then rejects the candidate).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Id,
    Num,
    Str,
    Regex,
    Template,
    Punct,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Tok {
    pub kind: Kind,
    pub s: u32,
    pub e: u32,
}

const RE_AFTER: &[&str] = &[
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
    "yield",
    "await",
];

fn is_id_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$' || b >= 0x80
}

fn is_id(b: u8) -> bool {
    is_id_start(b) || b.is_ascii_digit()
}

/// Tokenizes `src` (at most `u32::MAX` bytes; the caller enforces a far lower limit).
pub(crate) fn lex(src: &str) -> Vec<Tok> {
    let b = src.as_bytes();
    let n = b.len();
    let mut toks: Vec<Tok> = Vec::with_capacity(n / 3);
    let mut i = 0usize;
    let push = |toks: &mut Vec<Tok>, kind, s: usize, e: usize| toks.push(Tok { kind, s: s as u32, e: e as u32 });
    while i < n {
        let c = b[i];
        match c {
            b' ' | b'\n' | b'\t' | b'\r' | 0x0b | 0x0c => i += 1,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < n && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = src[i + 2..].find("*/").map_or(n, |p| i + 2 + p + 2);
            }
            _ if is_id_start(c) => {
                let s = i;
                i += 1;
                while i < n && is_id(b[i]) {
                    i += 1;
                }
                push(&mut toks, Kind::Id, s, i);
            }
            b'0'..=b'9' | b'.' if c != b'.' || b.get(i + 1).is_some_and(u8::is_ascii_digit) => {
                let s = i;
                i += 1;
                while i < n && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || b[i] == b'_') {
                    if matches!(b[i], b'e' | b'E') && matches!(b.get(i + 1), Some(b'+' | b'-')) {
                        i += 1;
                    }
                    i += 1;
                }
                push(&mut toks, Kind::Num, s, i);
            }
            b'"' | b'\'' => {
                let s = i;
                i += 1;
                while i < n && b[i] != c && b[i] != b'\n' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i = (i + 1).min(n);
                push(&mut toks, Kind::Str, s, i);
            }
            b'`' => {
                let s = i;
                i += 1;
                let mut depth = 0usize;
                while i < n {
                    match b[i] {
                        b'\\' => i += 1,
                        b'`' if depth == 0 => break,
                        b'$' if b.get(i + 1) == Some(&b'{') => {
                            depth += 1;
                            i += 1;
                        }
                        b'}' if depth > 0 => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
                i = (i + 1).min(n);
                push(&mut toks, Kind::Template, s, i);
            }
            b'/' if regex_allowed(src, toks.last()) => {
                let s = i;
                i += 1;
                let mut class = false;
                while i < n && b[i] != b'\n' {
                    match b[i] {
                        b'\\' => i += 1,
                        b'[' => class = true,
                        b']' => class = false,
                        b'/' if !class => break,
                        _ => {}
                    }
                    i += 1;
                }
                i = (i + 1).min(n);
                while i < n && is_id(b[i]) {
                    i += 1;
                }
                push(&mut toks, Kind::Regex, s, i);
            }
            _ => {
                // One punctuation byte. Non-ASCII bytes were consumed as identifier characters above.
                push(&mut toks, Kind::Punct, i, i + 1);
                i += 1;
            }
        }
    }
    toks
}

fn regex_allowed(src: &str, prev: Option<&Tok>) -> bool {
    let Some(p) = prev else { return true };
    match p.kind {
        Kind::Num | Kind::Str | Kind::Regex | Kind::Template => false,
        Kind::Id => RE_AFTER.contains(&&src[p.s as usize..p.e as usize]),
        Kind::Punct => !matches!(src.as_bytes()[p.s as usize], b')' | b']' | b'}'),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(src: &str) -> Vec<&str> {
        lex(src).iter().map(|t| &src[t.s as usize..t.e as usize]).collect()
    }

    #[test]
    fn basics() {
        assert_eq!(
            texts("a=b/2;c=/x\\/y[/]/g.test(d)"),
            ["a", "=", "b", "/", "2", ";", "c", "=", "/x\\/y[/]/g", ".", "test", "(", "d", ")"]
        );
        assert_eq!(texts("x='a\\'b'+\"c\"//t\n+1.5e-3"), ["x", "=", "'a\\'b'", "+", "\"c\"", "+", "1.5e-3"]);
        assert_eq!(texts("/*c*/a /* d */ b"), ["a", "b"]);
        assert_eq!(texts("return /a/.test(x)"), ["return", "/a/", ".", "test", "(", "x", ")"]);
    }

    #[test]
    fn garbage_terminates() {
        for s in ["'", "\"abc", "`", "`${", "/", "/*", "a/*", "\\", "\u{0}", "é=1", "/[", "1e", "..5", "(((", "'\\"] {
            let t = lex(s);
            assert!(t.iter().all(|t| t.e as usize <= s.len() && t.s < t.e), "{s:?}");
        }
    }
}
