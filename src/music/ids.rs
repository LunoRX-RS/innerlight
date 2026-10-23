//! Music-specific identifier checks. Everything that goes into a request body passes through here first.
//! (Video, channel and plain playlist ids live in [`crate::ids`].)

fn id_chars(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// An album browse id: `MPREb_` followed by 6..=40 id characters.
pub fn is_album_browse_id(id: &str) -> bool {
    id.strip_prefix("MPREb_").is_some_and(|r| (6..=40).contains(&r.len()) && id_chars(r))
}

/// A bare playlist id as used in `playlistId` (`PL…`, `OLAK5uy_…`, `RDCLAK…`, `LM`). The browse form with the `VL`
/// prefix is rejected: strip it first. 2..=64 id characters.
pub fn is_music_playlist_id(id: &str) -> bool {
    !id.starts_with("VL") && crate::ids::is_playlist_id(id)
}

/// A lyrics browse id: `MPLYt_` followed by 6..=60 id characters.
pub fn is_lyrics_browse_id(id: &str) -> bool {
    id.strip_prefix("MPLYt_").is_some_and(|r| (6..=60).contains(&r.len()) && id_chars(r))
}

/// A "related" browse id: `MPTRt_` followed by 6..=60 id characters.
pub fn is_related_browse_id(id: &str) -> bool {
    id.strip_prefix("MPTRt_").is_some_and(|r| (6..=60).contains(&r.len()) && id_chars(r))
}

/// A continuation token as the service hands them out: URL-safe base64 plus `%`, `=`, `~`, `.`; at most 8192 bytes.
pub fn is_continuation_token(t: &str) -> bool {
    (8..=8192).contains(&t.len())
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'%' | b'=' | b'~' | b'.' | b'+' | b'/'))
}

/// The `params` blob of a mood or genre button: URL-safe characters, at most 256 bytes.
pub fn is_genre_params(p: &str) -> bool {
    (4..=256).contains(&p.len())
        && p.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'%' | b'='))
}

/// `ZZ` (global) or an upper-case two-letter country code.
pub fn is_chart_country(c: &str) -> bool {
    c.len() == 2 && c.bytes().all(|b| b.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn album_and_playlist() {
        assert!(is_album_browse_id("MPREb_9nqEki4ZDpp"));
        for bad in ["", "MPREb_", "MPREb_a", "MPRE_9nqEki4ZDpp", "MPREb_9nq/Eki4Z", "OLAK5uy_abcdef"] {
            assert!(!is_album_browse_id(bad), "{bad}");
        }
        assert!(is_music_playlist_id("OLAK5uy_nPVVQ5bB1Fx4XqXVWwTn7b0sc5nB7sYzE"));
        assert!(is_music_playlist_id("RDAMVMaqz-KE-bpKQ"));
        assert!(is_music_playlist_id("LM"));
        for bad in ["", "L", "VLPLabc", "PL abc", "PL/../x"] {
            assert!(!is_music_playlist_id(bad), "{bad}");
        }
    }

    #[test]
    fn lyrics_related_tokens_params() {
        assert!(is_lyrics_browse_id("MPLYt_s7WaJYzUc2cfGsaF"));
        assert!(!is_lyrics_browse_id("MPTRt_s7WaJYzUc2cfGsaF"));
        assert!(is_related_browse_id("MPTRt_s7WaJYzUc2cfGsaF"));
        assert!(is_continuation_token("4qmFsgKrAxIMVkxQTHhZ%3D%3D"));
        for bad in ["", "short", "has space in it", "tok\nen12345"] {
            assert!(!is_continuation_token(bad), "{bad}");
        }
        assert!(is_genre_params("ggMPOg1uX1ZaRGhUa29OSlhr"));
        assert!(!is_genre_params("a b"));
        assert!(is_chart_country("ZZ") && is_chart_country("TH"));
        assert!(!is_chart_country("th") && !is_chart_country("THA"));
    }
}
