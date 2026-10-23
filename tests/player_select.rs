//! `select_audio`: table tests plus a randomised check against an independent brute-force oracle.

use innerlight::player::{AudioTrack, Quality, Stream, StreamSource, select_audio};

fn s(itag: u32, mime: &str, bitrate: u32, len: u64) -> Stream {
    let codecs = if mime.contains("webm") { "opus" } else { "mp4a.40.2" };
    Stream {
        itag,
        mime_type: format!("{mime}; codecs=\"{codecs}\""),
        codecs: Some(codecs.into()),
        bitrate,
        average_bitrate: None,
        content_length: len,
        audio_quality: None,
        sample_rate: None,
        loudness_db: None,
        track: None,
        drm: false,
        source: StreamSource::Direct(format!("https://a.googlevideo.com/{itag}")),
    }
}
fn opus(itag: u32, br: u32) -> Stream {
    s(itag, "audio/webm", br, 100)
}
fn aac(itag: u32, br: u32) -> Stream {
    s(itag, "audio/mp4", br, 100)
}
fn tr(mut st: Stream, id: &str, default: bool) -> Stream {
    st.track = Some(AudioTrack { id: id.into(), display_name: id.into(), is_default: default });
    st
}
fn pick(v: &[Stream], q: Quality) -> Option<u32> {
    select_audio(v, q).map(|s| s.itag)
}

#[test]
fn quality_table() {
    let v = [aac(1, 48_000), opus(2, 64_000), opus(3, 128_000), aac(4, 130_000), opus(5, 160_000)];
    assert_eq!(pick(&v, Quality::Low), Some(2), "64k counts as <= 64k");
    assert_eq!(pick(&v, Quality::Normal), Some(3));
    assert_eq!(pick(&v, Quality::High), Some(5));
}

#[test]
fn nothing_under_limit_takes_lowest_above() {
    let v = [opus(1, 200_000), opus(2, 150_000), opus(3, 300_000)];
    assert_eq!(pick(&v, Quality::Low), Some(2));
    assert_eq!(pick(&v, Quality::Normal), Some(2));
}

#[test]
fn ties_prefer_opus() {
    assert_eq!(pick(&[aac(1, 128_000), opus(2, 128_000)], Quality::High), Some(2));
    assert_eq!(pick(&[opus(2, 128_000), aac(1, 128_000)], Quality::High), Some(2));
    // lowest-above fallback ties too
    assert_eq!(pick(&[aac(1, 130_000), opus(2, 130_000)], Quality::Low), Some(2));
    assert_eq!(pick(&[opus(2, 130_000), aac(1, 130_000)], Quality::Low), Some(2));
}

#[test]
fn excludes_video_drm_and_empty() {
    let mut drm = opus(1, 300_000);
    drm.drm = true;
    let video = s(2, "video/mp4", 900_000, 100);
    let empty = s(3, "audio/mp4", 200_000, 0);
    assert_eq!(pick(&[drm.clone(), video.clone(), empty.clone()], Quality::High), None);
    assert_eq!(pick(&[drm, video, empty, aac(4, 50_000)], Quality::High), Some(4));
    assert_eq!(pick(&[], Quality::High), None);
}

#[test]
fn default_track_beats_bitrate() {
    let v = [tr(opus(1, 160_000), "es", false), tr(opus(2, 100_000), "en", true), tr(opus(3, 300_000), "de", false)];
    assert_eq!(pick(&v, Quality::High), Some(2));
    // untracked streams count as default
    let v = [tr(opus(1, 160_000), "es", false), aac(2, 100_000)];
    assert_eq!(pick(&v, Quality::High), Some(2));
    // no default anywhere: every track competes
    let v = [tr(opus(1, 160_000), "es", false), tr(opus(2, 100_000), "de", false)];
    assert_eq!(pick(&v, Quality::High), Some(1));
    // a DRM default track does not make the others ineligible
    let mut d = tr(opus(3, 90_000), "en", true);
    d.drm = true;
    assert_eq!(pick(&[d, tr(opus(1, 160_000), "es", false)], Quality::High), Some(1));
}

/// xorshift, deterministic.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[test]
fn property_against_oracle() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..3000 {
        let n = rng.below(9) as usize;
        let v: Vec<Stream> = (0..n)
            .map(|i| {
                let br = [0, 32_000, 48_000, 64_000, 64_001, 96_000, 128_000, 128_001, 160_000, 256_000]
                    [rng.below(10) as usize];
                let mut st = match rng.below(3) {
                    0 => aac(i as u32, br),
                    1 => opus(i as u32, br),
                    _ => s(i as u32, "video/mp4", br, 100),
                };
                st.content_length = if rng.below(6) == 0 { 0 } else { 100 };
                st.drm = rng.below(6) == 0;
                match rng.below(4) {
                    0 => st = tr(st, "a", true),
                    1 => st = tr(st, "b", false),
                    _ => {}
                }
                st
            })
            .collect();
        for q in [Quality::Low, Quality::Normal, Quality::High] {
            let got = select_audio(&v, q);
            // oracle
            let elig: Vec<&Stream> = v.iter().filter(|s| s.is_audio() && !s.drm && s.content_length > 0).collect();
            let dflt = |s: &&Stream| s.track.as_ref().is_none_or(|t| t.is_default);
            let any_default = elig.iter().any(dflt);
            let cands: Vec<&Stream> = elig.into_iter().filter(|s| !any_default || dflt(s)).collect();
            match got {
                None => assert!(cands.is_empty()),
                Some(g) => {
                    assert!(cands.iter().any(|c| c.itag == g.itag), "result must be a candidate");
                    let lim = q.bitrate_limit();
                    if cands.iter().any(|c| c.bitrate <= lim) {
                        assert!(g.bitrate <= lim);
                        assert!(cands.iter().filter(|c| c.bitrate <= lim).all(|c| c.bitrate <= g.bitrate));
                        assert!(
                            cands.iter().filter(|c| c.bitrate == g.bitrate).all(|c| c.is_opus() <= g.is_opus()),
                            "opus wins ties"
                        );
                    } else {
                        assert!(cands.iter().all(|c| c.bitrate >= g.bitrate));
                        assert!(cands.iter().filter(|c| c.bitrate == g.bitrate).all(|c| c.is_opus() <= g.is_opus()));
                    }
                }
            }
        }
    }
}
