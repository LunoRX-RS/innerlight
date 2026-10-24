//! Catalogues that are not YouTube, behind one trait.
//!
//! A [`Provider`] answers three questions in the plain data types of [`model`]: what matches a query, what is in
//! an album, and where a track's audio lives. Everything a provider fetches goes through [`Fetcher`], which is an
//! anonymous https GET with the same [`Policy`](crate::Policy) limits the YouTube client obeys — so a provider
//! cannot send a cookie, cannot rotate the `User-Agent`, and cannot flood a host.
//!
//! This is a read-only catalogue surface. Nothing here decodes audio or plays it: a [`Resolved`]
//! is a URL the caller hands to its own player, exactly like the player module's result for YouTube.
//!
//! See `docs/PROVIDERS.md` for what is implemented, what is deliberately refused (Bandcamp has no official API,
//! so a provider there would be a scraper), and what each source's data is and is not good for.

mod archive;
mod catalog;
pub mod http;
pub mod model;

pub use archive::Archive;
pub use catalog::{Catalog, Failure, SearchRound};
pub use http::{Fetcher, check_url};
pub use model::{Album, AlbumSummary, Item, Page, Resolved, Track};

use crate::Error;
use crate::transport::BoxFuture;

/// One external source. Object-safe, like [`Transport`](crate::Transport): providers live in an `Arc<dyn Provider>`
/// inside a [`Catalog`].
///
/// Every method is expected to be cancel-safe and to return the crate's own [`Error`] — no second error enum. An
/// id the provider cannot recognise is [`Error::InvalidId`]; a response it cannot read is [`Error::Parse`].
pub trait Provider: Send + Sync + 'static {
    /// Lowercase, stable, unique per catalog. Every item this provider returns carries it as its `source`, and it
    /// is what [`Failure::source`] reports.
    fn name(&self) -> &'static str;

    /// First page when `continuation` is `None`; the page after a previous answer otherwise. The handle is
    /// opaque: a provider may encode a page number, a token, or its own cursor format in it.
    fn search<'a>(&'a self, query: &'a str, continuation: Option<&'a str>) -> BoxFuture<'a, Result<Page<Item>, Error>>;

    /// One album with its track list.
    fn album<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<model::Album, Error>>;

    /// Where one track's audio lives. Returns a URL the caller may fetch itself; nothing is fetched or decoded
    /// here.
    fn resolve<'a>(&'a self, track_id: &'a str) -> BoxFuture<'a, Result<model::Resolved, Error>>;
}
