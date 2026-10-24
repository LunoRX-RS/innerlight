//! The Internet Archive provider, end to end and offline: every fetch answered by `FixtureTransport` and every
//! wait measured on `FakeClock`.
//!
//! What this pins down: the shape of the catalogue it builds from the two JSON endpoints, the ids it refuses
//! before sending anything, the https URL it hands back for a track (and that it never fetches that URL), and the
//! politeness rules it inherits — a 429 is waited out, a challenge is named and never resent.

use innerlight::provider::model::{Item, Resolved};
use innerlight::provider::{Album, Archive, Catalog, Fetcher, Page, Provider};
use innerlight::{Error, FakeClock, FixtureTransport, Policy, TokioClock, Transport};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

const ID: &str = "goatlivemusic2020-02-22";
const SEARCH: &str = "advancedsearch.php";

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/archive_handwritten_{name}.json", env!("CARGO_MANIFEST_DIR"))
}

fn fetcher(fx: Arc<FixtureTransport>, clock: Arc<FakeClock>, policy: Policy, max_bytes: usize) -> Fetcher {
    let transport: Arc<dyn Transport> = fx;
    Fetcher::new(transport, policy, clock, Duration::from_secs(5), max_bytes).unwrap()
}

/// Archive over a search page, its last page, and one item's metadata. The two search answers share one rule, so
/// the first call gets page one and every later call gets the last page.
fn searched() -> (Archive, Arc<FixtureTransport>, Arc<FakeClock>) {
    let fx = Arc::new(
        FixtureTransport::new()
            .with_file(SEARCH, fixture("search"))
            .with_file(SEARCH, fixture("search_last"))
            .with_file(&format!("metadata/{ID}"), fixture("metadata")),
    );
    let clock = Arc::new(FakeClock::new());
    let a = Archive::new(fetcher(fx.clone(), clock.clone(), Policy::no_retry(), 1 << 20), 40).unwrap();
    (a, fx, clock)
}

#[tokio::test]
async fn an_empty_page_has_no_next_page_and_a_page_number_is_the_only_handle() {
    let fx =
        Arc::new(FixtureTransport::new().with(SEARCH, json!({"response": {"numFound": 2, "start": 0, "docs": []}})));
    let clock = Arc::new(FakeClock::new());
    let a = Archive::new(fetcher(fx.clone(), clock.clone(), Policy::polite(), 1 << 20), 12).unwrap();
    let page = a.search("  goat  live ", None).await.unwrap();
    assert!(page.items.is_empty());
    assert_eq!(page.continuation, None, "no rows means no next page, whatever the counts say");
    assert_eq!(clock.elapsed_ms(), 0, "a plain answer costs no waiting");
    for junk in ["", "0", "1,5", "page-2", "4294967296", "50000"] {
        assert_eq!(a.search("goat", Some(junk)).await.unwrap_err(), Error::InvalidId, "{junk:?}");
    }
    assert_eq!(fx.requests().len(), 1, "a handle that is not a page number never reaches the host");
    assert_eq!(a.search("", None).await.unwrap_err(), Error::InvalidRequest);
    assert_eq!(a.search(&"x".repeat(600), None).await.unwrap_err(), Error::InvalidRequest);
    assert_eq!(fx.requests().len(), 1);
}

#[tokio::test]
async fn a_search_page_is_albums_with_credits_cover_and_licence() {
    let (a, fx, _clock) = searched();
    let page = a.search("goat live", None).await.unwrap();
    // The third hit's identifier is not a single path segment, so it is dropped rather than sanitised.
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.continuation.as_deref(), Some("2"), "start 0 + 2 of 120 rows means there is more");
    let Item::Album(first) = &page.items[0] else { panic!("an Archive hit is an item, so an album") };
    assert_eq!(first.id, ID);
    assert_eq!(first.title, "Goat - Live at Doornroosje 2020-02-22");
    assert_eq!(first.artists, ["Goat"]);
    assert_eq!(first.year, Some(2020));
    assert_eq!(first.license.as_deref(), Some("http://creativecommons.org/publicdomain/zero/1.0/"));
    assert_eq!(first.thumbnails[0].url, format!("https://archive.org/services/img/{ID}"));
    assert_eq!(first.attribution.as_deref(), Some(&*format!("https://archive.org/details/{ID}")));
    assert_eq!(first.source, "internetarchive");
    // A title the source gave as a list keeps its first value; a year given as a number still reads.
    let Item::Album(second) = &page.items[1] else { panic!("album") };
    assert_eq!(second.title, "Ultra Sound");
    assert_eq!(second.year, Some(1972));
    assert_eq!(second.license, None, "a licence is copied, never assumed");
    assert_eq!(fx.requests().len(), 1);
    assert_eq!(fx.requests()[0].host, "archive.org");
    assert_eq!(fx.requests()[0].endpoint, SEARCH);

    let last = a.search("goat live", Some("2")).await.unwrap();
    assert_eq!(last.items.len(), 1);
    assert_eq!(last.continuation, None, "start 40 + 1 of 41 is the end");
    assert_eq!(fx.requests().len(), 2);
}

#[tokio::test]
async fn an_album_lists_only_the_audio_files_and_keeps_their_lengths() {
    let (a, _fx, _clock) = searched();
    let Album { summary, description, tracks } = a.album(ID).await.unwrap();
    assert_eq!(summary.id, ID);
    assert_eq!(summary.title, "Goat - Live at Doornroosje 2020-02-22");
    assert_eq!(summary.artists, ["Goat"]);
    assert_eq!(description.as_deref(), Some("Soundboard recording, shared by the band."));
    let ids: Vec<&str> = tracks.items.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "goatlivemusic2020-02-22/goat2020-02-22sideA.mp3",
            "goatlivemusic2020-02-22/disc1/02 Midset.mp3",
            "goatlivemusic2020-02-22/goat2020-02-22.flac"
        ],
        "the scan and the nameless row are not tracks"
    );
    assert_eq!(tracks.continuation, None, "one metadata answer holds the whole item");
    assert_eq!(tracks.items[0].duration_ms, Some(1_831_420), "1831.42 seconds");
    assert_eq!(tracks.items[1].duration_ms, Some(1_872_500), "31:12.5");
    assert_eq!(tracks.items[2].duration_ms, Some(3_380_000), "56:20");
    // A file row with no title falls back to its stem, and with no credit to the item's creator.
    assert_eq!(tracks.items[2].title, "goat2020-02-22");
    assert_eq!(tracks.items[2].artists, ["Goat"]);
    assert_eq!(tracks.items[1].title, "Midset");
    assert_eq!(tracks.items[1].artists, ["Goat", "Mats Gustavsson"]);
    assert_eq!(tracks.items[1].album.as_deref(), Some("Goat - Live at Doornroosje 2020-02-22"));
    assert!(
        tracks.items.iter().all(|t| t.license.as_deref() == Some("http://creativecommons.org/publicdomain/zero/1.0/"))
    );
}

#[tokio::test]
async fn a_track_resolves_to_an_https_url_that_is_never_fetched_here() {
    let (a, fx, _clock) = searched();
    let album = a.album(ID).await.unwrap();
    let resolved: Resolved = a.resolve(&album.tracks.items[1].id).await.unwrap();
    assert_eq!(resolved.url, format!("https://archive.org/download/{ID}/disc1/02%20Midset.mp3"));
    assert!(resolved.url.starts_with("https://"), "the caller's player only ever gets an https URL");
    assert_eq!(resolved.mime.as_deref(), Some("audio/mpeg"));
    assert_eq!(resolved.duration_ms, Some(1_872_500));
    assert_eq!(resolved.license.as_deref(), Some("http://creativecommons.org/publicdomain/zero/1.0/"));
    assert_eq!(resolved.attribution.as_deref(), Some(&*format!("https://archive.org/details/{ID}")));
    // Two fetches, both metadata: the album's list and resolve's own confirmation. The audio URL is not among them.
    let endpoints: Vec<String> = fx.requests().iter().map(|r| r.endpoint.clone()).collect();
    assert_eq!(endpoints, [format!("metadata/{ID}"), format!("metadata/{ID}")]);
}

#[tokio::test]
async fn nothing_the_item_does_not_list_and_nonsense_ids_are_not_resolvable() {
    let (a, fx, clock) = searched();
    for bad in [
        "",
        "no-slash",
        "../etc/passwd.mp3",
        "bad..id/x.mp3",
        "bad id/x.mp3",
        &format!("{ID}/../secret.mp3"),
        &format!("{ID}/a:b.mp3"),
        &format!("{ID}/not-listed.mp3"),
    ] {
        assert_eq!(a.resolve(bad).await.unwrap_err(), Error::InvalidId, "{bad:?}");
    }
    // Only the last of those was well enough formed to ask: the file simply is not in the item.
    assert_eq!(fx.requests().len(), 1);
    assert_eq!(clock.elapsed_ms(), 0);
    for bad in ["", "a/b", "..", "a b"] {
        assert_eq!(a.album(bad).await.unwrap_err(), Error::InvalidId, "{bad:?}");
    }
    assert_eq!(fx.requests().len(), 1, "a rejected id never reaches the host");
}

#[tokio::test]
async fn a_rate_limited_search_waits_the_time_asked_and_asks_once_more() {
    let fx = Arc::new(
        FixtureTransport::new()
            .with_retry_after(SEARCH, 429, Duration::from_secs(2), b"slow down")
            .with_file(SEARCH, fixture("search")),
    );
    let clock = Arc::new(FakeClock::new());
    let a = Archive::new(fetcher(fx.clone(), clock.clone(), Policy::polite(), 1 << 20), 40).unwrap();
    let page = a.search("goat", None).await.unwrap();
    assert_eq!(page.items.len(), 2);
    let sent = clock.elapsed_ms();
    assert_eq!(sent, 2_000, "the 2 s asked for is the wait, exactly: {sent} ms");
    assert_eq!(fx.requests().len(), 2);
    assert_eq!(fx.requests()[0].endpoint, SEARCH);
}

#[tokio::test]
async fn a_challenge_is_named_and_never_resent() {
    let fx =
        Arc::new(FixtureTransport::new().with_raw(SEARCH, 403, b"<html>Sign in to confirm you're not a bot</html>"));
    let clock = Arc::new(FakeClock::new());
    let a = Archive::new(fetcher(fx.clone(), clock.clone(), Policy::polite(), 1 << 20), 40).unwrap();
    assert_eq!(a.search("goat", None).await.unwrap_err(), Error::BotCheck);
    assert_eq!(clock.elapsed_ms(), 0, "a challenge is not a transient failure");
    assert_eq!(fx.requests().len(), 1, "exactly one: the provider does not hammer a bot check");
}

#[tokio::test]
async fn a_service_error_is_retried_and_an_oversized_answer_is_refused() {
    let fx = Arc::new(FixtureTransport::new().with_raw(SEARCH, 503, b"gateway").with_file(SEARCH, fixture("search")));
    let clock = Arc::new(FakeClock::new());
    let a = Archive::new(fetcher(fx.clone(), clock.clone(), Policy::polite(), 1 << 20), 40).unwrap();
    assert_eq!(a.search("goat", None).await.unwrap().items.len(), 2, "a 5xx is transient");
    assert!(clock.elapsed_ms() >= 1000, "the second send waits at least one ladder step");
    assert_eq!(fx.requests().len(), 2);

    // A body over the cap is refused whole, before anything is parsed.
    let big = Arc::new(FixtureTransport::new().with_raw(SEARCH, 200, &[[b'x'; 200]; 4].concat()));
    let a = Archive::new(fetcher(big, Arc::new(FakeClock::new()), Policy::no_retry(), 512), 40).unwrap();
    assert_eq!(a.search("goat", None).await.unwrap_err(), Error::TooLarge);
}

#[tokio::test]
async fn an_answer_that_is_not_the_promised_shape_is_a_parse_error() {
    let fx = Arc::new(
        FixtureTransport::new()
            .with_raw(SEARCH, 200, b"<html>not json</html>")
            .with_raw("metadata/untitled", 200, b"{\"files\":[]}")
            .with_raw("metadata/empty", 200, b"{}")
            .with_raw("metadata/gone", 404, b""),
    );
    let a = Archive::new(fetcher(fx, Arc::new(FakeClock::new()), Policy::no_retry(), 1 << 16), 40).unwrap();
    assert_eq!(a.search("goat", None).await.unwrap_err(), Error::Parse, "an answer that is not JSON");
    assert_eq!(a.album("untitled").await.unwrap_err(), Error::Parse, "an item with no title is not an album");
    assert_eq!(a.album("empty").await.unwrap_err(), Error::Parse, "an answer with no metadata block");
    assert_eq!(a.resolve("empty/x.mp3").await.unwrap_err(), Error::InvalidId, "a file no list names");
    assert_eq!(a.album("gone").await.unwrap_err(), Error::Refused, "404 says refused");
}

#[tokio::test]
async fn the_archive_is_one_provider_among_several_in_a_catalog() {
    struct Other;
    impl Provider for Other {
        fn name(&self) -> &'static str {
            "other"
        }
        fn search<'a>(
            &'a self,
            _q: &'a str,
            _c: Option<&'a str>,
        ) -> innerlight::transport::BoxFuture<'a, Result<Page<Item>, Error>> {
            Box::pin(async { Err(Error::Network) })
        }
        fn album<'a>(&'a self, _id: &'a str) -> innerlight::transport::BoxFuture<'a, Result<Album, Error>> {
            Box::pin(async { Err(Error::Parse) })
        }
        fn resolve<'a>(&'a self, _t: &'a str) -> innerlight::transport::BoxFuture<'a, Result<Resolved, Error>> {
            Box::pin(async { Err(Error::Parse) })
        }
    }

    let (a, _fx, _clock) = searched();
    let mut catalog = Catalog::new();
    catalog.register(Arc::new(a)).register(Arc::new(Other));
    assert_eq!(catalog.names(), ["internetarchive", "other"]);
    let round = catalog.search_all("goat live", 5).await.unwrap();
    assert_eq!(round.items.len(), 2, "the other source failed, so only Archive's hits come back");
    assert_eq!(round.failures, vec![innerlight::provider::Failure { source: "other".into(), error: Error::Network }]);
    assert_eq!(round.duplicates, 0);
}

#[tokio::test]
async fn the_provider_never_carries_a_youtube_cookie_and_needs_no_account() {
    let fx =
        Arc::new(FixtureTransport::new().with(SEARCH, json!({"response": {"numFound": 0, "start": 0, "docs": []}})));
    let a = Archive::new(fetcher(fx.clone(), Arc::new(FakeClock::new()), Policy::no_retry(), 1 << 16), 40).unwrap();
    let page = a.search("anything", None).await.unwrap();
    assert!(page.items.is_empty());
    assert!(page.continuation.is_none());
    assert!(fx.requests()[0].headers.is_empty(), "a GET through Fetcher adds nothing at all, cookie included");
}

#[test]
fn production_time_and_a_page_size_outside_the_polite_range_are_rejected() {
    let fx = Arc::new(FixtureTransport::new());
    let transport: Arc<dyn Transport> = fx;
    let fetcher =
        Fetcher::new(transport, Policy::polite(), Arc::new(TokioClock::new()), Duration::from_secs(10), 1 << 20)
            .unwrap();
    assert_eq!(Archive::new(fetcher.clone(), 0).unwrap_err(), Error::InvalidRequest);
    assert_eq!(Archive::new(fetcher.clone(), 51).unwrap_err(), Error::InvalidRequest);
    assert_eq!(Archive::new(fetcher, 50).unwrap().name(), "internetarchive");
    assert_eq!(Archive::DEFAULT_ROWS, 40);
}
