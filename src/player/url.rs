//! Stream URL validation and HTTP range planning. Pure functions.

use super::PlayerError;

const MAX_URL_LEN: usize = 16 * 1024;
const HOST_SUFFIX: &str = ".googlevideo.com";

/// Accepts only `https://<label>.googlevideo.com/...`: no userinfo, no port, no fragment, no control characters
/// or whitespace, no quote or backslash, at most 16 KiB. Returns the normalised URL, which is the string to use
/// from then on.
pub fn check_stream_url(raw: &str) -> Result<String, PlayerError> {
    if raw.len() > MAX_URL_LEN || raw.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '"' | '\\')) {
        return Err(PlayerError::UnsafeUrl);
    }
    let url = reqwest::Url::parse(raw).map_err(|_| PlayerError::UnsafeUrl)?;
    let host_ok = url.host_str().is_some_and(|h| h.len() > HOST_SUFFIX.len() && h.ends_with(HOST_SUFFIX));
    let ok = url.scheme() == "https"
        && host_ok
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.fragment().is_none();
    if !ok {
        return Err(PlayerError::UnsafeUrl);
    }
    Ok(String::from(url))
}

/// An inclusive byte range, as in `Range: bytes=start-end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

impl ByteRange {
    /// The `Range` header value, e.g. `bytes=0-1048575`.
    pub fn header_value(&self) -> String {
        format!("bytes={}-{}", self.start, self.end)
    }

    /// Number of bytes in the range (always >= 1).
    pub fn size(&self) -> u64 {
        self.end - self.start + 1
    }
}

/// Splits `size` bytes into consecutive ranges of at most `chunk` bytes (googlevideo wants ranged requests).
/// Empty when `size` or `chunk` is 0. Lazy, so a huge `size` with a tiny `chunk` does not allocate.
pub fn ranged_reader_plan(size: u64, chunk: u64) -> impl Iterator<Item = ByteRange> {
    let first = (size > 0 && chunk > 0).then_some(0u64);
    std::iter::successors(first, move |&s| s.checked_add(chunk).filter(|&n| n < size))
        .map(move |s| ByteRange { start: s, end: s.saturating_add(chunk).min(size) - 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_googlevideo() {
        let u = "https://rr1---sn-abc.googlevideo.com/videoplayback?itag=251&n=x%2By";
        assert_eq!(check_stream_url(u).unwrap(), u);
    }

    #[test]
    fn rejects_bad_urls() {
        let long = format!("https://a.googlevideo.com/{}", "x".repeat(MAX_URL_LEN));
        for bad in [
            "http://a.googlevideo.com/x",
            "ftp://a.googlevideo.com/x",
            "https://googlevideo.com/x",
            "https://.googlevideo.com/x",
            "https://evilgooglevideo.com/x",
            "https://a.googlevideo.com.evil.com/x",
            "https://a.googlevideo.com./x",
            "https://user@a.googlevideo.com/x",
            "https://user:pw@a.googlevideo.com/x",
            "https://a.googlevideo.com:8443/x",
            "https://a.googlevideo.com/x#frag",
            "https://a.googlevideo.com/x\n",
            "https://a.googlevideo.com/x y",
            "https://a.googlevideo.com/\u{0}",
            "https://a.googlevideo.com\\@evil.com/x",
            "https://evil.com\\.googlevideo.com/x",
            "https://evil.com/.googlevideo.com",
            "https://127.0.0.1/x",
            "//a.googlevideo.com/x",
            "",
            "not a url",
            long.as_str(),
        ] {
            assert_eq!(check_stream_url(bad), Err(PlayerError::UnsafeUrl), "{bad:?}");
        }
    }

    #[test]
    fn plan_covers_every_byte_once() {
        let r: Vec<_> = ranged_reader_plan(10, 4).collect();
        assert_eq!(r, [ByteRange { start: 0, end: 3 }, ByteRange { start: 4, end: 7 }, ByteRange { start: 8, end: 9 }]);
        assert_eq!(r[2].header_value(), "bytes=8-9");
        assert_eq!(ranged_reader_plan(8, 4).count(), 2);
        assert_eq!(ranged_reader_plan(1, 1_000_000).collect::<Vec<_>>(), [ByteRange { start: 0, end: 0 }]);
        assert_eq!(ranged_reader_plan(0, 4).count(), 0);
        assert_eq!(ranged_reader_plan(4, 0).count(), 0);
    }

    #[test]
    fn plan_edge_and_overflow() {
        let last = ranged_reader_plan(u64::MAX, u64::MAX).collect::<Vec<_>>();
        assert_eq!(last, [ByteRange { start: 0, end: u64::MAX - 1 }]);
        let mut it = ranged_reader_plan(u64::MAX, u64::MAX / 2 + 1);
        assert_eq!(it.next().unwrap().start, 0);
        assert!(it.next().is_some());
        assert!(it.next().is_none());
        // exhaustive small sweep: contiguous, no overlap, exact total, each <= chunk
        for size in 0..40u64 {
            for chunk in 0..12u64 {
                let v: Vec<_> = ranged_reader_plan(size, chunk).collect();
                if size == 0 || chunk == 0 {
                    assert!(v.is_empty());
                    continue;
                }
                let mut next = 0;
                for r in &v {
                    assert_eq!(r.start, next);
                    assert!(r.size() <= chunk);
                    next = r.end + 1;
                }
                assert_eq!(next, size);
            }
        }
    }
}
