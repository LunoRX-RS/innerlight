//! Lenient helpers for walking InnerTube JSON. Pure functions: every malformed or missing shape gives `None`
//! or an empty result, never a panic.

use serde_json::Value;

/// Depth limit used by [`find_all`]. Real responses nest well under this.
pub const DEFAULT_MAX_DEPTH: usize = 24;

/// Looks up a path such as `a.b[0].c` or `[1][2]`. Keys are object keys, `[n]` indexes arrays.
/// An empty path returns the root. A bad path (unbalanced bracket, non-numeric index) or a missing step gives `None`.
pub fn get<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = value;
    if path.is_empty() {
        return Some(cur);
    }
    for segment in path.split('.') {
        let (key, mut rest) = match segment.find('[') {
            Some(i) => segment.split_at(i),
            None => (segment, ""),
        };
        if !key.is_empty() {
            cur = cur.as_object()?.get(key)?;
        } else if rest.is_empty() {
            return None; // empty segment such as `a..b`
        }
        while !rest.is_empty() {
            let inner = rest.strip_prefix('[')?;
            let end = inner.find(']')?;
            let idx: usize = inner[..end].parse().ok()?;
            cur = cur.as_array()?.get(idx)?;
            rest = &inner[end + 1..];
        }
    }
    Some(cur)
}

/// Text of a rich-text node: a plain string, `{"simpleText": ..}`, or `{"runs": [{"text": ..}, ..]}` (runs are joined).
/// Returns `None` for other shapes and for empty text.
pub fn text(value: &Value) -> Option<String> {
    let s = if let Some(s) = value.as_str() {
        s.to_owned()
    } else if let Some(s) = value.get("simpleText").and_then(Value::as_str) {
        s.to_owned()
    } else {
        value.get("runs")?.as_array()?.iter().filter_map(|r| r.get("text")?.as_str()).collect()
    };
    (!s.is_empty()).then_some(s)
}

/// Every value stored under `key`, anywhere in the tree, in document order (objects keep their key order).
/// Also searches inside matches. Stops descending below [`DEFAULT_MAX_DEPTH`].
pub fn find_all<'a>(value: &'a Value, key: &str) -> Vec<&'a Value> {
    find_all_depth(value, key, DEFAULT_MAX_DEPTH)
}

/// [`find_all`] with an explicit depth limit (the root is depth 0).
pub fn find_all_depth<'a>(value: &'a Value, key: &str, max_depth: usize) -> Vec<&'a Value> {
    fn walk<'a>(v: &'a Value, key: &str, depth: usize, max: usize, out: &mut Vec<&'a Value>) {
        if depth > max {
            return;
        }
        match v {
            Value::Object(map) => {
                for (k, child) in map {
                    if k == key {
                        out.push(child);
                    }
                    walk(child, key, depth + 1, max, out);
                }
            }
            Value::Array(items) => items.iter().for_each(|c| walk(c, key, depth + 1, max, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(value, key, 0, max_depth, &mut out);
    out
}

/// One image from a `thumbnails` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumb {
    pub url: String,
    pub width: u32,
    pub height: u32,
}

/// Picks the widest thumbnail. Accepts the list itself, `{"thumbnails": [..]}`, or `{"thumbnail": {"thumbnails": [..]}}`.
/// Entries without a `url` are skipped; a missing width counts as 0; ties go to the later entry.
/// Protocol-relative URLs (`//host/..`) get `https:`.
pub fn thumbnail_best(value: &Value) -> Option<Thumb> {
    let list = value
        .as_array()
        .or_else(|| value.get("thumbnails")?.as_array())
        .or_else(|| value.get("thumbnail")?.get("thumbnails")?.as_array())?;
    list.iter()
        .filter_map(|t| {
            let url = t.get("url")?.as_str().filter(|u| !u.is_empty())?;
            let n = |k: &str| t.get(k).and_then(Value::as_u64).map_or(0, |n| n.min(u32::MAX as u64) as u32);
            let url = if url.starts_with("//") { format!("https:{url}") } else { url.to_owned() };
            Some(Thumb { url, width: n("width"), height: n("height") })
        })
        .max_by_key(|t| t.width)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn get_paths() {
        let v = json!({"a": {"b": [{"c": 1}, {"c": 2}], "x": [[10, 11], [12]]}, "": 5});
        assert_eq!(get(&v, "a.b[1].c"), Some(&json!(2)));
        assert_eq!(get(&v, "a.b[0]"), Some(&json!({"c": 1})));
        assert_eq!(get(&v, "a.x[0][1]"), Some(&json!(11)));
        assert_eq!(get(&v, ""), Some(&v));
        assert_eq!(get(&json!([[1], [2]]), "[1][0]"), Some(&json!(2)));
        assert_eq!(get(&json!([7]), "[0]"), Some(&json!(7)));
    }

    #[test]
    fn get_malformed_and_missing() {
        let v = json!({"a": {"b": [1]}, "s": "str", "n": null});
        for p in [
            "a.c",
            "a.b[1]",
            "a.b[x]",
            "a.b[",
            "a.b]",
            "a.b[0",
            "a.b[-1]",
            "a..b",
            ".a",
            "a.",
            "s.x",
            "s[0]",
            "n.x",
            "a[0]",
            "a.b[0].c",
            "a.b[0][0]",
            "[",
            "]",
            "[]",
            "a.b[99999999999999999999]",
            "zz",
        ] {
            assert_eq!(get(&v, p), None, "{p}");
        }
        for root in [json!(null), json!(1), json!("x"), json!([]), json!({})] {
            assert_eq!(get(&root, "a.b[0]"), None);
        }
    }

    #[test]
    fn text_shapes() {
        assert_eq!(text(&json!({"simpleText": "Hello"})), Some("Hello".into()));
        assert_eq!(text(&json!({"runs": [{"text": "A"}, {"text": " & "}, {"text": "B"}]})), Some("A & B".into()));
        assert_eq!(text(&json!("plain")), Some("plain".into()));
        // runs with a bad entry in the middle still join the good ones
        assert_eq!(text(&json!({"runs": [{"text": "A"}, {"nope": 1}, 5, {"text": "B"}]})), Some("AB".into()));
    }

    #[test]
    fn text_malformed() {
        for v in [
            json!(null),
            json!(1),
            json!([]),
            json!({}),
            json!({"runs": "x"}),
            json!({"runs": []}),
            json!({"runs": [{"text": 5}]}),
            json!({"simpleText": 5}),
            json!({"simpleText": ""}),
            json!(""),
        ] {
            assert_eq!(text(&v), None, "{v}");
        }
    }

    #[test]
    fn find_all_order_and_nesting() {
        let v = json!({"a": {"item": 1, "list": [{"item": 2}, {"x": {"item": 3}}]}, "item": 4});
        let got: Vec<_> = find_all(&v, "item").into_iter().cloned().collect();
        assert_eq!(got, [json!(1), json!(2), json!(3), json!(4)]);
        // matches are searched inside too
        let nested = json!({"k": {"k": 1}});
        assert_eq!(find_all(&nested, "k").len(), 2);
        assert!(find_all(&json!(null), "k").is_empty());
        assert!(find_all(&json!([1, "k", null]), "k").is_empty());
    }

    #[test]
    fn find_all_depth_limit() {
        let mut v = json!({"k": "deep"});
        for _ in 0..10 {
            v = json!({ "n": v });
        }
        // the object holding `k` sits at depth 10 (root = 0)
        assert_eq!(find_all_depth(&v, "k", 10).len(), 1);
        assert!(find_all_depth(&v, "k", 9).is_empty());
        // pathologically deep arrays: must terminate, not overflow the stack
        let mut deep = json!("k");
        for _ in 0..100 {
            deep = json!([deep]);
        }
        let _ = find_all(&deep, "k");
        assert!(find_all(&deep, "k").is_empty());
    }

    #[test]
    fn thumbnails() {
        let list = json!([
            {"url": "https://i/a", "width": 60, "height": 60},
            {"url": "//i/c", "width": 544, "height": 544},
            {"url": "https://i/b", "width": 120, "height": 120},
            {"width": 9999},
        ]);
        let want = Thumb { url: "https://i/c".into(), width: 544, height: 544 };
        assert_eq!(thumbnail_best(&list), Some(want.clone()));
        assert_eq!(thumbnail_best(&json!({"thumbnails": list.clone()})), Some(want.clone()));
        assert_eq!(thumbnail_best(&json!({"thumbnail": {"thumbnails": list}})), Some(want));
        // missing widths: later entry wins ties
        let nowidth = json!([{"url": "u1"}, {"url": "u2"}]);
        assert_eq!(thumbnail_best(&nowidth).unwrap().url, "u2");
    }

    #[test]
    fn thumbnails_malformed() {
        for v in [
            json!(null),
            json!(5),
            json!("x"),
            json!([]),
            json!({}),
            json!({"thumbnails": 5}),
            json!([{"width": 5}]),
            json!([{"url": ""}]),
            json!([{"url": 5, "width": 5}]),
            json!({"thumbnail": null}),
        ] {
            assert_eq!(thumbnail_best(&v), None, "{v}");
        }
        // junk width types do not panic
        assert_eq!(thumbnail_best(&json!([{"url": "u", "width": "wide", "height": -1}])).unwrap().width, 0);
        assert_eq!(thumbnail_best(&json!([{"url": "u", "width": 1e20}])).unwrap().width, 0);
    }
}
