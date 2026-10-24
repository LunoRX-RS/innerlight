//! Several providers searched at once, merged into one list.

use super::Provider;
use super::model::{Item, Page};
use crate::Error;
use std::collections::HashSet;
use std::sync::Arc;

/// Longest query a provider is asked with, matching the music client's own ceiling.
const MAX_QUERY_LEN: usize = 512;
/// Ceiling for `per_provider`, so a merge cannot be asked to hold a million rows.
const MAX_PER_PROVIDER: usize = 100;

pub(crate) fn check_query(q: &str) -> Result<&str, Error> {
    let q = q.trim();
    if q.is_empty() || q.len() > MAX_QUERY_LEN || q.chars().any(char::is_control) {
        return Err(Error::InvalidRequest);
    }
    Ok(q)
}

/// Lowercased and trimmed: the whole of the match rule, which is stated in `docs/PROVIDERS.md`.
fn normalize(s: &str) -> String {
    s.trim().to_lowercase()
}

/// The merge key: title and first credit, case- and whitespace-insensitive. Nothing fuzzy.
fn key(item: &Item) -> String {
    format!("{}\u{1f}{}", normalize(item.title()), normalize(item.primary_artist().unwrap_or("")))
}

/// One provider that failed while the others answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// [`Provider::name`] of the provider that failed.
    pub source: String,
    pub error: Error,
}

/// The merged answer of [`Catalog::search_all`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRound {
    /// Merged, interleaved and deduplicated hits.
    pub items: Vec<Item>,
    /// Providers that answered nothing usable, in registration order. A non-empty list here means the result is
    /// partial, not that the search failed.
    pub failures: Vec<Failure>,
    /// How many items the merge dropped as duplicates.
    pub duplicates: usize,
}

/// Providers the application registered, searched as one.
///
/// The merge is mechanical and stated in full: take hit 1 of provider 1, hit 1 of provider 2, and so on, then hit
/// 2 of each, up to `per_provider` each; drop any item whose title and first credit already appeared, so the first
/// copy in that round-robin order is the one kept. There is no ranking, no scoring and no hidden preference for
/// one provider over another — the registration order is the preference, and it is the caller's.
#[derive(Default, Clone)]
pub struct Catalog {
    providers: Vec<Arc<dyn Provider>>,
}

impl std::fmt::Debug for Catalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Catalog").field("providers", &self.providers.len()).finish()
    }
}

impl Catalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a provider. Names are expected to be distinct: they are what [`Failure::source`] reports.
    pub fn register(&mut self, provider: Arc<dyn Provider>) -> &mut Self {
        self.providers.push(provider);
        self
    }

    pub fn len(&self) -> usize {
        self.providers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Registered names, in registration order.
    pub fn names(&self) -> Vec<&'static str> {
        self.providers.iter().map(|p| p.name()).collect()
    }

    /// Ask every provider at once, first page each.
    ///
    /// One provider failing does not fail the search: the rest still come back and the failure is listed. `Err`
    /// only when *every* provider failed, carrying the first failure's error, so a total outage is never
    /// mistaken for a search that found nothing. An answer with no items in it is a normal empty result.
    pub async fn search_all(&self, query: &str, per_provider: usize) -> Result<SearchRound, Error> {
        let q = check_query(query)?;
        if self.providers.is_empty() || per_provider == 0 || per_provider > MAX_PER_PROVIDER {
            return Err(Error::InvalidRequest);
        }
        let answers = futures_util::future::join_all(self.providers.iter().map(|p| p.search(q, None))).await;

        let mut pages: Vec<Vec<Item>> = Vec::with_capacity(answers.len());
        let mut failures = Vec::new();
        for (provider, answer) in self.providers.iter().zip(answers) {
            match answer {
                Ok(Page { items, .. }) => pages.push(items.into_iter().take(per_provider).collect()),
                Err(error) => failures.push(Failure { source: provider.name().to_owned(), error }),
            }
        }
        if pages.is_empty() {
            return Err(failures.first().map(|f| f.error).unwrap_or(Error::Parse));
        }

        let mut seen = HashSet::new();
        let mut items = Vec::new();
        let mut duplicates = 0usize;
        for rank in 0..per_provider {
            for page in &pages {
                let Some(item) = page.get(rank) else { continue };
                if seen.insert(key(item)) {
                    items.push(item.clone());
                } else {
                    duplicates += 1;
                }
            }
        }
        Ok(SearchRound { items, failures, duplicates })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::model;
    use crate::transport::BoxFuture;
    use serde_json::json;

    /// A provider that answers from a fixed value and records the queries it got.
    struct Stub {
        name: &'static str,
        answer: Result<Page<Item>, Error>,
        seen: Arc<std::sync::Mutex<Vec<String>>>,
    }

    fn track(source: &str, id: &str, title: &str, artist: &str) -> Item {
        Item::Track(model::Track {
            id: id.to_owned(),
            title: title.to_owned(),
            artists: vec![artist.to_owned()],
            album: None,
            duration_ms: None,
            thumbnails: vec![],
            license: None,
            attribution: None,
            source: source.to_owned(),
        })
    }

    impl Provider for Stub {
        fn name(&self) -> &'static str {
            self.name
        }
        fn search<'a>(&'a self, q: &'a str, _c: Option<&'a str>) -> BoxFuture<'a, Result<Page<Item>, Error>> {
            self.seen.lock().unwrap().push(q.to_owned());
            Box::pin(async { self.answer.clone() })
        }
        fn album<'a>(&'a self, _id: &'a str) -> BoxFuture<'a, Result<model::Album, Error>> {
            Box::pin(async { Err(Error::Parse) })
        }
        fn resolve<'a>(&'a self, _t: &'a str) -> BoxFuture<'a, Result<model::Resolved, Error>> {
            Box::pin(async { Err(Error::Parse) })
        }
    }

    fn ok(name: &'static str, items: Vec<Item>) -> Arc<Stub> {
        Arc::new(Stub {
            name,
            answer: Ok(Page { items, continuation: Some("x".to_owned()) }),
            seen: Arc::new(std::sync::Mutex::new(vec![])),
        })
    }

    fn err(name: &'static str, error: Error) -> Arc<Stub> {
        Arc::new(Stub { name, answer: Err(error), seen: Arc::new(std::sync::Mutex::new(vec![])) })
    }

    #[tokio::test]
    async fn hits_interleave_and_duplicates_are_dropped() {
        let a = ok("a", vec![track("a", "1", "First Song", "Artist"), track("a", "2", "Only A", "Solo")]);
        let b = ok("b", vec![track("b", "9", " first song ", "ARTIST"), track("b", "8", "Only B", "Solo")]);
        let mut catalog = Catalog::new();
        catalog.register(a.clone()).register(b.clone());
        let round = catalog.search_all("  first  ", 5).await.unwrap();
        // round robin by rank, and the case-insensitive copy from `b` loses to the first one seen
        let ids: Vec<_> = round.items.iter().map(|i| i.title()).collect();
        assert_eq!(ids, ["First Song", "Only A", "Only B"]);
        assert_eq!(round.duplicates, 1);
        assert!(round.failures.is_empty());
        assert_eq!(*a.seen.lock().unwrap(), ["first"]);
        assert_eq!(*b.seen.lock().unwrap(), ["first"]);
    }

    #[tokio::test]
    async fn per_provider_caps_each_source_before_merging() {
        // Titles carry the source: the merge key is title and credit only, so identical titles across sources
        // would be dropped as duplicates and the cap would not be visible.
        let many =
            |src: &str| (0..10).map(|i| track(src, i.to_string().as_str(), &format!("{src}-T{i}"), "X")).collect();
        let mut catalog = Catalog::new();
        catalog.register(ok("a", many("a"))).register(ok("b", many("b")));
        let round = catalog.search_all("q", 2).await.unwrap();
        assert_eq!(round.items.len(), 4);
        assert_eq!(round.duplicates, 0);
        let titles: Vec<_> = round.items.iter().map(|i| i.title()).collect();
        assert_eq!(titles, ["a-T0", "b-T0", "a-T1", "b-T1"]);
    }

    #[tokio::test]
    async fn one_failure_does_not_sink_the_round() {
        let mut catalog = Catalog::new();
        catalog.register(ok("a", vec![track("a", "1", "Kept", "X")]));
        catalog.register(err("b", Error::RateLimited { retry_after: None }));
        let round = catalog.search_all("q", 3).await.unwrap();
        assert_eq!(round.items.len(), 1);
        assert_eq!(
            round.failures,
            vec![Failure { source: "b".to_owned(), error: Error::RateLimited { retry_after: None } }]
        );
    }

    #[tokio::test]
    async fn everything_failing_is_an_error_not_an_empty_result() {
        let mut catalog = Catalog::new();
        catalog.register(err("a", Error::ServerError));
        catalog.register(err("b", Error::Network));
        assert_eq!(catalog.search_all("q", 3).await.unwrap_err(), Error::ServerError);

        // an answer with no items is not a failure; the round is empty but fine
        let mut catalog = Catalog::new();
        catalog.register(err("a", Error::ServerError));
        catalog.register(ok("b", vec![]));
        let round = catalog.search_all("q", 3).await.unwrap();
        assert!(round.items.is_empty());
        assert_eq!(round.failures.len(), 1);
    }

    #[tokio::test]
    async fn nonsense_is_rejected_before_any_request() {
        let mut catalog = Catalog::new();
        catalog.register(ok("a", vec![track("a", "1", "T", "X")]));
        for q in ["", "  ", "a\u{1}", &"x".repeat(513)] {
            assert_eq!(catalog.search_all(q, 3).await.unwrap_err(), Error::InvalidRequest, "{q:?}");
        }
        for n in [0, 101] {
            assert_eq!(catalog.search_all("q", n).await.unwrap_err(), Error::InvalidRequest, "{n}");
        }
        assert_eq!(Catalog::new().search_all("q", 3).await.unwrap_err(), Error::InvalidRequest);
    }

    #[test]
    fn keys_are_title_and_first_credit() {
        let i = track("a", "1", "  Song Name ", "Some Artist");
        let j = track("b", "2", "SONG NAME", "Some Artist");
        assert_eq!(key(&i), key(&j));
        let k = Item::Album(model::AlbumSummary {
            id: "z".into(),
            title: "Song Name".into(),
            artists: vec!["some artist".into()],
            year: None,
            thumbnails: vec![],
            license: None,
            attribution: None,
            source: "b".into(),
        });
        assert_eq!(key(&i), key(&k), "an album hit and a track hit with the same credit collide on purpose");
        // An `Item` serialises as a tagged object, so a caller can tell which kind it got.
        assert_eq!(
            json!(k),
            json!({"Album": {"id": "z", "title": "Song Name", "artists": ["some artist"],
            "year": null, "thumbnails": [], "license": null, "attribution": null, "source": "b"}})
        );
    }
}
