//! Identifier checks. Every id that goes into a URL, a request body or a file name passes through here first.

/// An 11-character video id: letters, digits, `_` and `-`.
pub fn is_video_id(id: &str) -> bool {
    id.len() == 11 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A channel id: `UC` followed by 22 id characters.
pub fn is_channel_id(id: &str) -> bool {
    id.len() == 24 && id.starts_with("UC") && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A playlist id (`PL…`, `OLAK5uy_…`, `RDAMVM…`, `VL…`, `LM`, `WL`): letters, digits, `_` and `-`, up to 64 characters.
pub fn is_playlist_id(id: &str) -> bool {
    (2..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_ids() {
        assert!(is_video_id("aqz-KE-bpKQ"));
        for bad in ["", "short", "has space!!", "../../etc/pa", "aqz-KE-bpKQx"] {
            assert!(!is_video_id(bad), "{bad}");
        }
    }

    #[test]
    fn channel_and_playlist_ids() {
        assert!(is_channel_id("UCuAXFkgsw1L7xaCfnd5JJOw"));
        assert!(!is_channel_id("UCshort"));
        assert!(!is_channel_id("XXuAXFkgsw1L7xaCfnd5JJOw"));
        assert!(is_playlist_id("PLrAXtmErZgOeiKm4sgNOknGvNjby9efdf"));
        assert!(!is_playlist_id("a/b"));
        assert!(!is_playlist_id(""));
    }
}
