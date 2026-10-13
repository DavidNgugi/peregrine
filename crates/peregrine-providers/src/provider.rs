//! The `MetadataProvider` port.
//!
//! One trait, implemented by every metadata adapter, and read by everything that needs to
//! know what a provider is before it asks for anything. The registry holds them
//! (`Vec<Arc<dyn MetadataProvider>>`, PG-E5-12), the token bucket is built from the rate the
//! descriptor declares (PG-E5-03), and the diagnostics screen lists what each one says about
//! itself (PG-E5-22).
//!
//! # Why the signature is a boxed future
//!
//! `search` reaches the network, so it is asynchronous, and the port is `dyn`-compatible
//! because a registry of providers is heterogeneous — TMDB, TVDB, and `OMDb` behind one list,
//! composed from settings rather than from a `match`. On this toolchain an `async fn` in a
//! trait rules `dyn` out (`error[E0038]: the trait is not dyn compatible`), and a concrete
//! return type in its place would make every provider's future a different type. So the
//! method returns a boxed future through [`ProviderFuture`], which costs one allocation per
//! network request and buys object safety without a proc-macro dependency.
//!
//! # Why only one operation
//!
//! `ARCHITECTURE` §13 lists eight. `search` is the one whose request and response the flows
//! already fix — FLOWS F3 has the sync service call `search` and get candidates back to
//! upsert as stub titles — and the rest return *drafts* of the catalogue entities, which carry
//! no row id and therefore cannot be the domain's stored types. Those drafts are the normal
//! form, and the normal form is designed against real provider payloads in PG-E5-06 to
//! PG-E5-16 rather than guessed at here from the providers' documentation.

use std::future::Future;
use std::num::NonZeroU32;
use std::pin::Pin;

use jiff::civil::Date;

use peregrine_domain::catalogue::TitleKind;
use peregrine_domain::id::ExternalId;
use peregrine_domain::text::NonEmptyText;

use crate::descriptor::ProviderDescriptor;
use crate::error::ProviderError;

/// What a provider operation answers with, as a future the caller can await.
///
/// Named rather than written out because every operation returns one, and a signature that
/// spelled `Pin<Box<dyn Future<Output = Result<…>> + Send + 'a>>` at each of them would be a
/// wall of punctuation around the part that matters.
pub type ProviderFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ProviderError>> + Send + 'a>>;

/// What a caller asks a provider to look for.
///
/// The text is a [`NonEmptyText`] and the limit a [`NonZeroU32`], so the two ways a search
/// request can be meaningless — an empty query and a request for nothing — are unrepresentable
/// rather than checked for. The kind is optional because a provider that can search both
/// films and series takes one endpoint per kind, and a caller that does not care asks both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    text: NonEmptyText,
    limit: NonZeroU32,
    kind: Option<TitleKind>,
}

impl SearchQuery {
    /// Builds a query for `text`, asking for at most `limit` candidates.
    #[must_use]
    pub const fn new(text: NonEmptyText, limit: NonZeroU32, kind: Option<TitleKind>) -> Self {
        Self { text, limit, kind }
    }

    /// What to look for.
    #[must_use]
    pub const fn text(&self) -> &NonEmptyText {
        &self.text
    }

    /// The most candidates to return.
    #[must_use]
    pub const fn limit(&self) -> NonZeroU32 {
        self.limit
    }

    /// The kind to restrict to, when the caller cares.
    #[must_use]
    pub const fn kind(&self) -> Option<TitleKind> {
        self.kind
    }
}

/// A title a search matched, as the provider identifies it.
///
/// A candidate is a *stub*: enough to list, to choose between two same-named titles, and to
/// re-ask for in full. It is not a [`peregrine_domain::catalogue::Title`] and cannot be — that
/// type carries the `i64` row id `peregrine-storage` assigns — so the sync service upserts
/// these as stub rows and a later `get_title` fills them in (PG-E5-06, PG-E6-01).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchCandidate {
    external_id: ExternalId,
    name: NonEmptyText,
    kind: TitleKind,
    released: Option<Date>,
}

impl SearchCandidate {
    /// Builds a candidate.
    #[must_use]
    pub const fn new(
        external_id: ExternalId,
        name: NonEmptyText,
        kind: TitleKind,
        released: Option<Date>,
    ) -> Self {
        Self {
            external_id,
            name,
            kind,
            released,
        }
    }

    /// The provider's own identifier for the title.
    ///
    /// It must name the provider that answered: a provider returning another provider's id is
    /// the coupling ADR 0007 rule 2 exists to prevent, and it is the one thing this port's
    /// conformance suite can check about a candidate without knowing the provider's fixtures.
    #[must_use]
    pub const fn external_id(&self) -> &ExternalId {
        &self.external_id
    }

    /// The title's name, as the provider writes it.
    #[must_use]
    pub const fn name(&self) -> &NonEmptyText {
        &self.name
    }

    /// Film or series.
    #[must_use]
    pub const fn kind(&self) -> TitleKind {
        self.kind
    }

    /// The release date for a film or the first air date for a series.
    ///
    /// It is what tells a remake from its original when the two share a name, which is the
    /// reconciliation PG-E5-13 has to do, so a candidate that drops it makes that task harder
    /// than the provider made it.
    #[must_use]
    pub const fn released(&self) -> Option<Date> {
        self.released
    }
}

/// A source of catalogue metadata.
///
/// Implementors are adapters: `peregrine-providers` keeps every provider's own types inside
/// its own module and returns only domain values (ADR 0007 rule 2), so nothing about TMDB's
/// JSON escapes the adapter that speaks it.
pub trait MetadataProvider: Send + Sync {
    /// What this provider is, what it can be asked for, and the terms it answers under.
    ///
    /// Borrowed rather than returned by value: the descriptor is fixed when the adapter is
    /// constructed and read on every call — the token bucket reads the rate before each
    /// request — so copying it would be a per-request allocation for no gain.
    fn descriptor(&self) -> &ProviderDescriptor;

    /// Finds titles whose name matches `query`.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Unsupported`] when the descriptor does not declare
    /// [`crate::capability::Capability::Search`]. The check is the implementation's to make, because only it
    /// knows what it can serve, and the conformance suite is what holds it to that: a provider
    /// either answers or refuses, and it never spends a request finding out which.
    ///
    /// [`ProviderError::RateLimited`], [`ProviderError::SchemaDrift`],
    /// [`ProviderError::TooLarge`], and [`ProviderError::Transport`] are the provider's own
    /// answers, mapped by the adapter from its HTTP stack (PG-E5-02, PG-E5-05).
    fn search(&self, query: &SearchQuery) -> ProviderFuture<'_, Vec<SearchCandidate>>;

    /// Reads one title's details.
    ///
    /// `kind` is a parameter because a provider's identifier space is not always split by kind — TMDB
    /// numbers films and series in one namespace of integers — so an id alone does not say which
    /// endpoint answers it.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Unsupported`] when the descriptor does not declare
    /// [`crate::capability::Capability::Title`], and otherwise as [`MetadataProvider::search`].
    fn title(
        &self,
        id: &ExternalId,
        kind: TitleKind,
    ) -> ProviderFuture<'_, peregrine_domain::catalogue::TitleDetails>;
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::num::NonZeroU32;
    use std::pin::Pin;

    use jiff::civil::Date;

    use peregrine_domain::TitleKind;
    use peregrine_domain::id::{ExternalId, Provider};
    use peregrine_domain::text::NonEmptyText;

    use super::*;

    fn text(value: &str) -> NonEmptyText {
        NonEmptyText::new(value).expect("the test passes real text")
    }

    fn limit(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).expect("the test passes a non-zero limit")
    }

    /// A provider whose only job is to prove the trait can be used behind `dyn`.
    struct Stub {
        descriptor: ProviderDescriptor,
    }

    impl MetadataProvider for Stub {
        fn descriptor(&self) -> &ProviderDescriptor {
            &self.descriptor
        }

        fn title(
            &self,
            _id: &ExternalId,
            _kind: TitleKind,
        ) -> ProviderFuture<'_, peregrine_domain::catalogue::TitleDetails> {
            Box::pin(async {
                Err(ProviderError::Unsupported {
                    provider: Provider::Tmdb,
                    capability: "title",
                })
            })
        }

        fn search(&self, _query: &SearchQuery) -> ProviderFuture<'_, Vec<SearchCandidate>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    fn stub() -> Stub {
        use crate::capability::{Capabilities, Capability};
        use crate::descriptor::{CachePolicy, RatePolicy};

        let rate = RatePolicy::new(limit(5), std::time::Duration::from_secs(1), limit(1), false)
            .expect("well formed");
        let cache =
            CachePolicy::new(std::time::Duration::from_secs(60), None, false).expect("well formed");
        Stub {
            descriptor: ProviderDescriptor::new(
                Provider::Tmdb,
                "Stub",
                Capabilities::EMPTY.with(Capability::Search),
                rate,
                cache,
                None,
            )
            .expect("well formed"),
        }
    }

    #[test]
    fn the_port_can_be_used_behind_a_trait_object() {
        // The registry holds providers of different types in one list (PG-E5-12), which is
        // why the operation returns a boxed future instead of an `async fn` — that form does
        // not compile behind `dyn` on this toolchain.
        let provider: Box<dyn MetadataProvider> = Box::new(stub());

        assert_eq!(provider.descriptor().provider(), Provider::Tmdb);
    }

    #[test]
    fn a_query_carries_what_was_asked_for() {
        let query = SearchQuery::new(text("breaking bad"), limit(20), Some(TitleKind::Show));

        assert_eq!(query.text().as_str(), "breaking bad");
        assert_eq!(query.limit().get(), 20);
        assert_eq!(query.kind(), Some(TitleKind::Show));
    }

    #[test]
    fn a_query_for_either_kind_says_so_rather_than_guessing_one() {
        let query = SearchQuery::new(text("dune"), limit(5), None);

        assert!(query.kind().is_none());
    }

    #[test]
    fn a_candidate_keeps_the_provider_that_answered() {
        let id = ExternalId::new(Provider::Tmdb, "1396").expect("a valid id");
        let candidate = SearchCandidate::new(
            id.clone(),
            text("Breaking Bad"),
            TitleKind::Show,
            Some(Date::constant(2008, 1, 20)),
        );

        assert_eq!(candidate.external_id(), &id);
        assert_eq!(candidate.name().as_str(), "Breaking Bad");
        assert_eq!(candidate.kind(), TitleKind::Show);
        assert_eq!(candidate.released(), Some(Date::constant(2008, 1, 20)));
    }

    #[test]
    fn a_candidate_whose_provider_gave_no_date_is_still_a_candidate() {
        // Not every provider dates everything, and a candidate without a date is still
        // enough to list and to re-ask for.
        let candidate = SearchCandidate::new(
            ExternalId::new(Provider::Tvdb, "81189").expect("a valid id"),
            text("Breaking Bad"),
            TitleKind::Show,
            None,
        );

        assert!(candidate.released().is_none());
    }

    #[test]
    fn a_provider_can_be_awaited_through_the_port() {
        // The future is `Send`, which is what lets a provider be called from the scheduler's
        // worker pool rather than from one thread.
        fn assert_send<T: Send>(_: &T) {}

        let stub = stub();
        let query = SearchQuery::new(text("anything"), limit(1), None);
        let future: Pin<Box<dyn Future<Output = _> + Send + '_>> = stub.search(&query);
        assert_send(&future);
    }
}
