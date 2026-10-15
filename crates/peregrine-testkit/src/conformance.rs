//! The conformance suite: what any `Clock`, any `IdGenerator`, and any `TorrentEngine` must satisfy.
//!
//! A fake is only worth sharing if it and the adapter it stands in for answer to the same
//! rules, so the rules are written here once and run twice. The fake runs them in this
//! crate's tests; `peregrine-platform` runs them against `SystemClock` and `UlidGenerator`
//! in its own, which is what `Cargo.toml`'s dev-dependency on this crate is for. The edge
//! is one-way — this crate knows `peregrine-domain`, never an adapter — so there is no
//! cycle and `peregrine-platform`'s shipping build is unchanged.
//!
//! The suite observes only the port. It reads no wall clock, takes no lock, and sleeps
//! nowhere, because a suite that needed real time to pass could not be run against a
//! frozen fake — and the fake is the only implementation whose instant a test can pin.
//!
//! The torrent half is different in kind from the clock and identifier halves: an engine has
//! state, so its assertions are about a sequence of operations — add, select, stream, pause,
//! resume, verify, remove — and about the refusal each state produces. It checks the *contract*,
//! never the fixture: which files a torrent has, how far along it is, and what its peers are belong
//! to the caller, and [`EngineProbe`] is where the suite is told them.

use std::collections::BTreeSet;
use std::num::NonZeroU32;

use jiff::Timestamp;
use peregrine_domain::check_text;
use peregrine_domain::id::Provider;
use peregrine_providers::{
    BrowseRequest, BrowseSort, Capability, MetadataProvider, ProviderError, RAIL_PAGE_LIMIT,
    RailKind, SearchCandidate, SearchQuery,
};
use peregrine_torrent::{
    AddRequest, Bytes, EngineError, EngineLimits, FileSelection, StreamChunk, StreamRequest,
    TorrentEngine, TorrentHandle, TorrentStats,
};
use ulid::Ulid;

/// Identifiers the suite draws from an [`IdGenerator`](peregrine_domain::IdGenerator) before it is satisfied.
///
/// Four would catch a generator that returns a constant; this many also catches one that
/// cycles through a short list, and 64 ULIDs are nothing to hold.
const CONFORMING_DRAWS: usize = 64;

/// A set of the identifiers a run drew, which is how the suite reports what it saw.
type Set<T> = BTreeSet<T>;

/// The project's first commit, in milliseconds. A clock reporting seconds, microseconds,
/// or a different epoch lands below it.
const PROJECT_FLOOR_MILLISECONDS: i64 = 1_600_000_000_000;

/// Asserts that `now` satisfies the [`Clock`](peregrine_domain::Clock) contract, and
/// returns the first instant read.
///
/// Every read must land after the project's first commit and no read may go backwards,
/// which is the property a journal's ordering rests on. The suite is silent about whether
/// two reads *differ*: a coarse clock may legitimately answer twice from the same tick, so
/// requiring real time to have passed would make the suite flaky for the one
/// implementation that reads a real clock.
///
/// # Panics
///
/// Panics with the offending instant when the contract is broken.
pub fn clock_satisfies_the_port(now: impl Fn() -> Timestamp) -> Timestamp {
    let first = now();
    assert!(
        first.as_millisecond() > PROJECT_FLOOR_MILLISECONDS,
        "{first} is before the project existed, which a clock returning the wrong unit \
         would look like"
    );
    let second = now();
    assert!(
        second >= first,
        "a later read went backwards: {first} then {second}"
    );
    let third = now();
    assert!(
        third >= second,
        "a later read went backwards: {second} then {third}"
    );
    first
}

/// Asserts that `next_id` satisfies the [`IdGenerator`](peregrine_domain::IdGenerator)
/// contract.
///
/// Successive draws must differ: the scheduler asks for an identifier per submission, and
/// a repeat collides in its dedupe index. Ordering is deliberately *not* asserted — a
/// ULID's leading bits are its millisecond, so two of them minted in the same millisecond
/// order arbitrarily, and a suite demanding a strict order would be asserting something
/// the production generator does not promise.
///
/// # Panics
///
/// Panics naming the repeated identifier when the contract is broken.
pub fn id_generator_satisfies_the_port(next_id: impl Fn() -> Ulid) -> Set<Ulid> {
    let drawn: Vec<Ulid> = (0..CONFORMING_DRAWS).map(|_| next_id()).collect();
    let unique: Set<Ulid> = drawn.iter().copied().collect();
    assert_eq!(
        unique.len(),
        drawn.len(),
        "a repeated identifier would collide in the dedupe index"
    );
    unique
}

/// Asserts that `provider` satisfies the [`MetadataProvider`] contract, and returns the
/// candidates the search probe drew.
///
/// The suite observes only the port, and it takes a `dyn` reference on purpose: that the port
/// is object safe is itself part of the contract, because the registry holds one list of
/// providers of different types (PG-E5-12) and a signature that quietly stopped being
/// `dyn`-compatible would only be discovered there.
///
/// `rail` and `rail_page` are the rail probe: a caller names one list and one page, because the
/// suite cannot invent either — a rail's answer is a statement about a provider's catalogue, and
/// only the provider's own fixtures know what is in it.
///
/// What it asserts, for a provider that either declares `search` or does not:
///
/// - if it declares `search`, the probe is answered with `Ok` — a provider that claims the
///   capability and then fails the call is a claim the registry would have repeated to a user;
/// - if it does not, the call is refused with [`ProviderError::Unsupported`] naming this
///   provider and the capability, rather than falling through to a request;
/// - a refusal is the *only* thing an undeclared capability produces, so a provider cannot
///   spend a rate-limit token finding out what it already said about itself;
/// - no more candidates come back than the query asked for, because the number travels to the
///   UI and the IPC payload budget is a contract rather than a hope (AGENTS R11);
/// - every candidate carries **this** provider's identifier. A provider returning another
///   provider's ids is the coupling ADR 0007 rule 2 exists to prevent — the reference
///   implementation's TVDB adapter called TMDB internally and married the two forever — and
///   it is the one fact about a candidate the suite can check without knowing the provider's
///   fixtures;
/// - a candidate that carries artwork carries *something*: an empty or whitespace-only path is a
///   request for an image host's root dressed as a poster, and the cache key it would build names
///   no image. A provider that has no poster for an item says so with `None`.
///
/// And the same shape for `rail`, with the two differences a rail has: the bound is
/// [`RAIL_PAGE_LIMIT`] rather than a caller's limit, and a provider that *declares* `trending` and
/// then refuses one rail is conforming — a provider may have a trending list and no "on the air"
/// one, and the port names that case. What is *not* conforming is declaring the capability and
/// refusing with [`ProviderError::Unsupported`] attributing it to another provider, which is the
/// same misattribution the search refusal is checked for.
///
/// It deliberately says nothing about *which* candidates come back: that is what the
/// provider's recorded fixtures are for (PG-E5-17), and a suite that guessed at them would be
/// asserting its own guess.
///
/// # Panics
///
/// Panics naming what the provider did when any of the above is broken.
pub async fn metadata_provider_satisfies_the_port(
    provider: &dyn MetadataProvider,
    probe: &SearchQuery,
    rail: RailKind,
    rail_page: NonZeroU32,
) -> Vec<SearchCandidate> {
    let descriptor = provider.descriptor();
    let named = descriptor.provider();

    // A probe is not one of §13's capabilities, so every provider must be able to answer it — that is
    // what makes it usable for the key-validation flow (PG-E5-20). It may *fail*, and for a fixture
    // that is the point, but a failure attributed to another provider is the same misattribution the
    // search refusal below is checked for, and a user sent to the wrong settings pane.
    if let Err(failure) = provider.check().await {
        assert_eq!(
            failure.provider(),
            named,
            "a probe failure must name the provider it is about"
        );
    }

    let declared = descriptor.can(Capability::Search);
    let answer = provider.search(probe).await;

    // The rail probe runs whatever the search probe did: the two capabilities are independent, and
    // a provider that cannot search may still publish a rail.
    check_rail(provider, rail, rail_page).await;

    // The browse and genre probes are the same shape again, and they run after the rail for one
    // reason: a fixture that exists to misname a rail refusal must still fail on the rail, and a
    // probe that panicked first would make the test named for that rule pass for the wrong one.
    check_browse(provider).await;
    check_genres(provider).await;
    check_similar(provider).await;

    if !declared {
        match answer {
            Err(ProviderError::Unsupported {
                provider: reported,
                capability,
            }) => {
                assert_eq!(
                    reported, named,
                    "a refusal must name the provider that refused"
                );
                assert_eq!(
                    capability,
                    Capability::Search.as_str(),
                    "a refusal must name the capability as it is declared"
                );
            }
            other => panic!(
                "{named} does not declare search, so it must refuse it rather than {other:?}"
            ),
        }
        return Vec::new();
    }

    let candidates = match answer {
        Ok(candidates) => candidates,
        Err(error) => panic!("{named} declares search but answered {error}"),
    };

    assert!(
        candidates.len() <= probe.limit().get() as usize,
        "{named} answered {} candidates for a limit of {}",
        candidates.len(),
        probe.limit()
    );

    for candidate in &candidates {
        check_candidate(candidate, named, "search");
    }

    candidates
}

/// The facts about one candidate the suite can check without knowing a provider's fixtures.
///
/// These are identity rules rather than taste: the id must name the provider that answered,
/// artwork that is present must address something — the poster and the backdrop both, because both
/// are paths a caller hands back to [`MetadataProvider::image`] — a synopsis that is present must say
/// something, and a vote count must not be the zero a provider uses for "nobody has voted".
/// `operation` names the call that answered, so a failure says which endpoint produced it.
fn check_candidate(candidate: &SearchCandidate, named: Provider, operation: &str) {
    assert_eq!(
        candidate.external_id().provider(),
        named,
        "{named} answered a {operation} candidate identified by {}",
        candidate.external_id().provider()
    );
    if let Some(path) = candidate.artwork_path() {
        assert!(
            !path.trim().is_empty(),
            "{named} answered a {operation} candidate whose artwork path is blank, which names no image"
        );
    }
    if let Some(path) = candidate.backdrop_path() {
        assert!(
            !path.trim().is_empty(),
            "{named} answered a {operation} candidate whose backdrop path is blank, which names no image"
        );
    }
    if let Some(overview) = candidate.overview() {
        assert!(
            !overview.trim().is_empty(),
            "{named} answered a {operation} candidate whose synopsis is blank, which says nothing"
        );
    }
    assert_ne!(
        candidate.votes(),
        Some(0),
        "{named} answered a {operation} candidate with a vote count of zero, which is a provider's way of saying nobody has voted"
    );
}

/// The rail half of the contract, which is the search half's shape with a rail's two differences.
///
/// Written as a panic rather than a `Result` because that is what the search half does and because
/// a suite that returned its failures would be a suite a caller could ignore.
async fn check_rail(provider: &dyn MetadataProvider, rail: RailKind, page: NonZeroU32) {
    let descriptor = provider.descriptor();
    let named = descriptor.provider();

    if !descriptor.can(Capability::Trending) {
        match provider.rail(rail, page).await {
            Err(ProviderError::Unsupported {
                provider: reported,
                capability,
            }) => {
                assert_eq!(
                    reported, named,
                    "a rail refusal must name the provider that refused"
                );
                assert_eq!(
                    capability,
                    Capability::Trending.as_str(),
                    "a rail refusal must name the capability as it is declared"
                );
            }
            other => panic!(
                "{named} does not declare trending, so it must refuse a rail rather than {other:?}"
            ),
        }
        return;
    }

    match provider.rail(rail, page).await {
        Ok(candidates) => {
            assert!(
                candidates.len() <= RAIL_PAGE_LIMIT,
                "{named} answered {} candidates for a page, which holds at most {RAIL_PAGE_LIMIT}",
                candidates.len()
            );
            for candidate in &candidates {
                check_candidate(candidate, named, "rail");
            }
        }
        // A provider with a trending list and no list for this particular rail is conforming, and
        // the refusal has to be about this provider — the same misattribution rule as everywhere.
        Err(ProviderError::Unsupported {
            provider: reported, ..
        }) => assert_eq!(
            reported, named,
            "{named} refused a rail in another provider's name"
        ),
        Err(error) => panic!("{named} declares trending but answered a rail with {error}"),
    }
}

/// The browse half of the contract: a page composed from filters.
///
/// **The request is the suite's own rather than a caller's argument.** A browse request is a
/// composition — a kind, an order, a genre, a floor — and every one of those is the caller's taste;
/// what the port fixes is the *shape* of the answer, which is what this checks. Asking the most
/// ordinary question there is (the first page of films, most popular first) is enough to hold every
/// adapter to the rules that hold for all of them.
///
/// Two rules belong to this operation and not to the rail: a page may hold at most
/// [`RAIL_PAGE_LIMIT`], and a page that holds nothing may not claim another page follows — the
/// second is what a screen's "load more" hangs off, and an empty page that says there is more sends
/// a reader to a page with nothing on it for ever.
async fn check_browse(provider: &dyn MetadataProvider) {
    let descriptor = provider.descriptor();
    let named = descriptor.provider();
    let request = BrowseRequest::new(
        peregrine_domain::catalogue::TitleKind::Movie,
        NonZeroU32::new(1).expect("one is not zero"),
        BrowseSort::Popularity,
        None,
        None,
    );

    if !descriptor.can(Capability::Trending) {
        match provider.browse(&request).await {
            Err(ProviderError::Unsupported {
                provider: reported,
                capability,
            }) => {
                assert_eq!(
                    reported, named,
                    "a browse refusal must name the provider that refused"
                );
                assert_eq!(
                    capability,
                    Capability::Trending.as_str(),
                    "a browse refusal must name the capability as it is declared"
                );
            }
            other => panic!(
                "{named} does not declare trending, so it must refuse a browse rather than {other:?}"
            ),
        }
        return;
    }

    match provider.browse(&request).await {
        Ok(page) => {
            assert!(
                page.items().len() <= RAIL_PAGE_LIMIT,
                "{named} answered {} candidates for a page, which holds at most {RAIL_PAGE_LIMIT}",
                page.items().len()
            );
            if page.items().is_empty() {
                assert!(
                    !page.has_more(),
                    "{named} answered an empty page that claims another page follows"
                );
            }
            for candidate in page.items() {
                check_candidate(candidate, named, "browse");
            }
        }
        // A provider with a queried list and no filtered endpoint is conforming, and the refusal
        // has to be about this provider — the same misattribution rule as everywhere.
        Err(ProviderError::Unsupported {
            provider: reported, ..
        }) => assert_eq!(
            reported, named,
            "{named} refused a browse in another provider's name"
        ),
        Err(error) => panic!("{named} declares trending but answered a browse with {error}"),
    }
}

/// The genre half, which is what gives a browse filter its vocabulary.
///
/// The two rules are the ones a filter control depends on: every genre must be named — a control
/// that drew a blank row would offer a filter with no label — and no identifier may appear twice,
/// because the control keys its options by id and two rows with one key is a control that cannot
/// say which was chosen.
async fn check_genres(provider: &dyn MetadataProvider) {
    let descriptor = provider.descriptor();
    let named = descriptor.provider();
    let kind = peregrine_domain::catalogue::TitleKind::Movie;

    if !descriptor.can(Capability::Trending) {
        match provider.genres(kind).await {
            Err(ProviderError::Unsupported {
                provider: reported,
                capability,
            }) => {
                assert_eq!(
                    reported, named,
                    "a genre refusal must name the provider that refused"
                );
                assert_eq!(
                    capability,
                    Capability::Trending.as_str(),
                    "a genre refusal must name the capability as it is declared"
                );
            }
            other => panic!(
                "{named} does not declare trending, so it must refuse a genre list rather than {other:?}"
            ),
        }
        return;
    }

    match provider.genres(kind).await {
        Ok(genres) => {
            // There is deliberately no check that a name says something: `NonEmptyText` refuses a
            // blank or whitespace-only one where the adapter builds the `GenreRef`, so a nameless
            // genre is unrepresentable rather than something a suite could catch. What the suite
            // *can* catch is a list a control cannot distinguish, which is what the id is for.
            let mut seen = BTreeSet::new();
            for genre in &genres {
                assert!(
                    seen.insert(genre.id()),
                    "{named} answered the genre id {} twice, which a filter control keys by",
                    genre.id()
                );
            }
        }
        Err(ProviderError::Unsupported {
            provider: reported, ..
        }) => assert_eq!(
            reported, named,
            "{named} refused a genre list in another provider's name"
        ),
        Err(error) => panic!("{named} declares trending but answered genres with {error}"),
    }
}

/// The recommendation half: the titles a provider considers like one it knows.
///
/// The probe's identifier is the suite's own — a fixture's real id is not the port's business — and
/// what the port fixes about the call is the shape every list shares: gated on `trending`, refused in
/// the refusing provider's own name, bounded by [`RAIL_PAGE_LIMIT`], and identified by the provider
/// that answered.
async fn check_similar(provider: &dyn MetadataProvider) {
    let descriptor = provider.descriptor();
    let named = descriptor.provider();
    let Ok(id) = peregrine_domain::id::ExternalId::new(named, "1") else {
        panic!("{named} cannot name an identifier at all");
    };
    let kind = peregrine_domain::catalogue::TitleKind::Movie;

    if !descriptor.can(Capability::Trending) {
        match provider.similar(&id, kind).await {
            Err(ProviderError::Unsupported {
                provider: reported,
                capability,
            }) => {
                assert_eq!(
                    reported, named,
                    "a recommendation refusal must name the provider that refused"
                );
                assert_eq!(
                    capability,
                    Capability::Trending.as_str(),
                    "a recommendation refusal must name the capability as it is declared"
                );
            }
            other => panic!(
                "{named} does not declare trending, so it must refuse a similar read rather than {other:?}"
            ),
        }
        return;
    }

    match provider.similar(&id, kind).await {
        Ok(candidates) => {
            assert!(
                candidates.len() <= RAIL_PAGE_LIMIT,
                "{named} answered {} candidates for a recommendation page, which holds at most {RAIL_PAGE_LIMIT}",
                candidates.len()
            );
            for candidate in &candidates {
                check_candidate(candidate, named, "similar");
            }
        }
        Err(ProviderError::Unsupported {
            provider: reported, ..
        }) => assert_eq!(
            reported, named,
            "{named} refused a similar read in another provider's name"
        ),
        Err(error) => panic!("{named} declares trending but answered a similar read with {error}"),
    }
}

/// A real path, a magnet URI, and a bare info-hash: the three shapes no refusal may carry.
///
/// They are the canaries the leak scan is run against, so a `Display` that interpolated a value
/// fails the suite with the class of leak it produced rather than only with a failed predicate.
const CANARY_PATH: &str = "/Users/dave/Downloads/Peregrine/Movie.mkv";
const CANARY_MAGNET: &str = "magnet:?xt=urn:btih:C12FE1C06BBA254A4DE7DDD1AD9E1B0D9C6F0A4B";
const CANARY_HASH: &str = "C12FE1C06BBA254A4DE7DDD1AD9E1B0D9C6F0A4B";

/// What the suite cannot invent about the engine it is driven against.
///
/// A torrent fixture is a torrent: the suite cannot invent the request a caller has already
/// inspected and allowed, the file list the engine's metadata reports, or the limits a settings
/// screen set. The probe carries them, in the same way the metadata suite is handed a
/// [`SearchQuery`] and a rail rather than guessing one.
#[derive(Debug)]
pub struct EngineProbe {
    request: AddRequest,
    reattach: AddRequest,
    file_count: usize,
    file_len: Bytes,
    limits: EngineLimits,
}

impl EngineProbe {
    /// A probe over one torrent.
    ///
    /// `file_count` must be at least two, and at least one past every index `request` and
    /// `reattach` select: the suite probes the file after the selection's last one as out of range,
    /// and a file the selection does not name as unselected, and neither exists in a one-file
    /// torrent. `file_len` is the length the engine's metadata reports for each file.
    ///
    /// `reattach` is the same torrent added with
    /// [`AddIntent::Reattach`](peregrine_torrent::AddIntent::Reattach), which is what a session
    /// that was restarted — or a finished job put back to seed — sends.
    #[must_use]
    pub fn new(
        request: AddRequest,
        reattach: AddRequest,
        file_count: usize,
        file_len: Bytes,
        limits: EngineLimits,
    ) -> Self {
        Self {
            request,
            reattach,
            file_count,
            file_len,
            limits,
        }
    }

    /// The add the suite starts a download with.
    #[must_use]
    pub const fn request(&self) -> &AddRequest {
        &self.request
    }

    /// The same torrent, added as a reattach.
    #[must_use]
    pub const fn reattach_request(&self) -> &AddRequest {
        &self.reattach
    }

    /// How many files the engine holds for the probe torrent.
    #[must_use]
    pub const fn file_count(&self) -> usize {
        self.file_count
    }

    /// The length of each of those files, in bytes.
    #[must_use]
    pub const fn file_len(&self) -> Bytes {
        self.file_len
    }

    /// The limits the suite applies and reads back.
    #[must_use]
    pub const fn limits(&self) -> &EngineLimits {
        &self.limits
    }
}

/// Asserts that `engine` satisfies the [`TorrentEngine`] contract, and returns the handle it added.
///
/// The suite observes only the port, and every operation it drives is one `FLOWS` F6 and SM1 use,
/// so what a caller may rely on is what this walks:
///
/// - an allowed add is accepted and answers a handle, and the statistics for that handle are about
///   the torrent that was added;
/// - a handle no session holds — one that was never minted, and one that was just removed — is
///   refused with [`EngineError::UnknownTorrent`] rather than panicking or hanging, while `remove`
///   is idempotent for the reason the port documents;
/// - a reattach is accepted, reports state to persist, re-checks its pieces on demand, and pauses
///   and resumes like any other torrent;
/// - a reader opens on a selected file, never answers an empty chunk or a chunk longer than it was
///   asked for, advances its position by exactly what it answered, treats the end of the file as an
///   empty read, and refuses an offset past the end;
/// - an empty selection is refused — by `add` and by `select` — because a torrent that writes
///   nothing is a removal, and a file the torrent does not have is a different refusal from one it
///   has but is not downloading;
/// - a selection replaces the set, so a file outside it stops being readable;
/// - `pause` and `resume` are answered in every live state, including twice in a row;
/// - limits applied at runtime are the limits in force, on the same live session;
/// - after `shutdown`, every operation answers [`EngineError::SessionClosed`] rather than hanging or
///   panicking, while `shutdown` itself is idempotent and the descriptor still answers;
/// - no refusal's `Display` carries a path, a magnet, or an info-hash.
///
/// It asserts nothing about *which* files a torrent has, how fast it is moving, or what its peers
/// are: those are the caller's fixture, and the probe is where the suite is told them.
///
/// # Panics
///
/// Panics naming what the engine did when any of the above is broken.
pub async fn torrent_engine_satisfies_the_port(
    engine: &dyn TorrentEngine,
    probe: &EngineProbe,
) -> TorrentHandle {
    check_error_text_is_redacted();
    check_descriptor(engine);
    check_probe(probe);

    // The add every other probe hangs off, and the identity check the engine alone can make: the
    // counters it reports are about the torrent the caller asked for.
    let handle = add(engine, probe.request()).await;
    assert_eq!(
        stats_of(engine, handle).await.info_hash(),
        probe.request().info_hash(),
        "the statistics are for another torrent"
    );

    check_unknown_handle(engine, probe).await;
    check_recovery(engine, probe).await;
    check_streaming(engine, probe, handle).await;
    check_selection(engine, probe, handle).await;
    check_lifecycle(engine, probe, handle).await;
    check_limits(engine, probe).await;
    check_closed(engine, probe, handle).await;

    handle
}

/// One of every variant of [`EngineError`].
///
/// A new variant belongs here: the port's own test is what fails a build that forgets one, and this
/// list is what the leak scan below is run over.
fn every_engine_error() -> Vec<EngineError> {
    vec![
        EngineError::SessionClosed,
        EngineError::UnknownTorrent {
            handle: TorrentHandle::new(1),
        },
        EngineError::EmptySelection,
        EngineError::UnknownFile { index: 1 },
        EngineError::FileNotSelected { index: 1 },
        EngineError::OffsetPastEnd {
            offset: Bytes::new(9),
            len: Bytes::new(8),
        },
        EngineError::IdentityMismatch,
        EngineError::Failed,
    ]
}

/// The leak scan: a refusal is text a user reads and a diagnostics tail is exported, so no variant
/// may render a path, a magnet, or an info-hash (SECURITY T13, PG-E10-15).
fn check_error_text_is_redacted() {
    for error in every_engine_error() {
        let text = error.to_string();
        assert!(
            check_text(&text, 512).is_ok(),
            "a refusal is user-visible text and must pass the leak scanner: {text}"
        );
        for canary in [CANARY_PATH, CANARY_MAGNET, CANARY_HASH] {
            assert!(
                !text.contains(canary),
                "a refusal carried a forbidden shape: {text}"
            );
        }
    }
}

/// The engine names itself, and the two strings are ones a Diagnostics row draws.
fn check_descriptor(engine: &dyn TorrentEngine) {
    let descriptor = engine.descriptor();
    for text in [descriptor.name().as_str(), descriptor.version().as_str()] {
        assert!(
            check_text(text, 512).is_ok(),
            "a Diagnostics row is user-visible text: {text}"
        );
    }
}

/// The probe's own claims, checked before the engine is driven: a suite handed a fixture that
/// cannot express the rules it states would pass without testing them.
fn check_probe(probe: &EngineProbe) {
    let files = probe.file_count();
    assert!(
        files >= 2,
        "the suite probes a second file and an out-of-range one, so the fixture needs at least two"
    );
    assert!(
        !probe.request().selection().is_empty(),
        "the request the suite adds cannot be the empty selection the suite refuses"
    );
    for request in [probe.request(), probe.reattach_request()] {
        assert!(
            request
                .selection()
                .indices()
                .iter()
                .all(|index| *index < files),
            "every file a probe request selects must exist in the torrent the engine holds"
        );
    }
    assert_eq!(
        probe.request().info_hash(),
        probe.reattach_request().info_hash(),
        "the two requests describe one torrent"
    );
    assert!(
        probe.reattach_request().intent().verifies_existing_data(),
        "the reattach request must carry the reattach intent"
    );
    assert_ne!(
        probe.limits(),
        &EngineLimits::unlimited(),
        "a probe whose limits are all unlimited cannot tell an applied limit from a swallowed one"
    );
}

/// One add, with the refusal a conforming engine may not produce.
async fn add(engine: &dyn TorrentEngine, request: &AddRequest) -> TorrentHandle {
    match engine.add(request).await {
        Ok(handle) => handle,
        Err(error) => panic!("a conforming engine accepts an allowed add: {error}"),
    }
}

/// One statistics read, with the refusal a live torrent may not produce.
async fn stats_of(engine: &dyn TorrentEngine, handle: TorrentHandle) -> TorrentStats {
    match engine.stats(handle).await {
        Ok(stats) => stats,
        Err(error) => panic!("a live torrent answers statistics: {error}"),
    }
}

/// The suite's own reader request: the offset is the probe's, and the read-ahead window is the
/// caller's taste — what the port fixes is that a window exists and is carried.
fn stream_request(handle: TorrentHandle, file: usize, from: Bytes, window: Bytes) -> StreamRequest {
    StreamRequest::new(handle, file, from, window)
}

/// The refusal a stream attempt produced, or a panic when the contract says it must refuse.
async fn stream_refusal(engine: &dyn TorrentEngine, request: &StreamRequest) -> EngineError {
    match engine.stream(request).await {
        Ok(_) => panic!("a stream was answered where the contract refuses"),
        Err(error) => error,
    }
}

/// A handle no session holds answers every operation but `remove` with a typed refusal.
async fn check_unknown_handle(engine: &dyn TorrentEngine, probe: &EngineProbe) {
    // A handle no fixture mints: the fake counts up from one and a real engine indexes its own, so
    // the largest possible number is one no engine has handed out.
    let unknown = TorrentHandle::new(u64::MAX);
    let expected = EngineError::UnknownTorrent { handle: unknown };

    assert_eq!(engine.stats(unknown).await, Err(expected));
    assert_eq!(engine.state(unknown).await, Err(expected));
    assert_eq!(engine.verify(unknown).await, Err(expected));
    assert_eq!(engine.pause(unknown).await, Err(expected));
    assert_eq!(engine.resume(unknown).await, Err(expected));
    assert_eq!(
        engine.select(unknown, probe.request().selection()).await,
        Err(expected)
    );
    assert_eq!(
        stream_refusal(
            engine,
            &stream_request(unknown, 0, Bytes::new(0), probe.file_len())
        )
        .await,
        expected
    );
    assert_eq!(
        engine.remove(unknown).await,
        Ok(()),
        "removing a handle the session does not hold is idempotent, not a refusal"
    );
}

/// A session that is reattaching verifies what is on disk, reports state to persist, re-checks on
/// demand, and stops and starts like any other torrent.
async fn check_recovery(engine: &dyn TorrentEngine, probe: &EngineProbe) {
    let handle = add(engine, probe.reattach_request()).await;

    // What a caller persists and hands back through the reattach intent. The port promises that a
    // live torrent answers this; whether the engine has anything to remember is the engine's own
    // business, so an empty value is not a failure and the contents are not the suite's to judge.
    let _state = engine
        .state(handle)
        .await
        .expect("a live torrent reports state to persist");

    engine
        .verify(handle)
        .await
        .expect("a live torrent re-checks its pieces");
    engine
        .verify(handle)
        .await
        .expect("re-checking twice is idempotent");

    // Pausing a torrent that is seeding is how a user stops seeding early (PG-E10-09), so the two
    // are answered here as well as on the downloading torrent below.
    engine.pause(handle).await.expect("a live torrent pauses");
    engine
        .resume(handle)
        .await
        .expect("a paused torrent resumes");

    engine
        .remove(handle)
        .await
        .expect("a live torrent is removed");
}

/// A reader opens on a selected file, answers whole chunks up to its bound, and treats the end of
/// the file as an empty read rather than a failure or a hang.
async fn check_streaming(engine: &dyn TorrentEngine, probe: &EngineProbe, handle: TorrentHandle) {
    let selected = probe.request().selection().indices()[0];
    let max = NonZeroU32::new(1_024).expect("one thousand and twenty-four is not zero");
    let window = Bytes::new(8 * 1024 * 1024);

    let mut reader = match engine
        .stream(&stream_request(handle, selected, Bytes::new(0), window))
        .await
    {
        Ok(reader) => reader,
        Err(error) => panic!("a selected file opens a reader: {error}"),
    };
    assert_eq!(
        reader.position(),
        Bytes::new(0),
        "a reader opens where it was asked to"
    );
    assert_eq!(
        reader.len(),
        probe.file_len(),
        "a reader knows the length its metadata reports"
    );

    match reader.read(max).await.expect("a reader answers a read") {
        StreamChunk::Data(bytes) => {
            assert!(!bytes.is_empty(), "a chunk of data is never empty");
            assert!(
                bytes.len() <= max.get() as usize,
                "a read answers at most what it was asked for"
            );
            assert_eq!(
                reader.position().get(),
                bytes.len() as u64,
                "a read advances the position by exactly what it answered"
            );
        }
        StreamChunk::End => assert_eq!(
            reader.position(),
            reader.len(),
            "End arrived before the file ended"
        ),
    }

    // The two boundaries of the same rule: an offset equal to the length is an empty range, and one
    // past it is a refusal a range request can turn into "not satisfiable".
    let mut at_end = match engine
        .stream(&stream_request(handle, selected, probe.file_len(), window))
        .await
    {
        Ok(reader) => reader,
        Err(error) => {
            panic!("a reader at the end of a file is an empty read, not a refusal: {error}")
        }
    };
    assert_eq!(at_end.position(), probe.file_len());
    assert_eq!(
        at_end.read(max).await.expect("a reader answers a read"),
        StreamChunk::End
    );

    let past_end = Bytes::new(probe.file_len().get() + 1);
    assert_eq!(
        stream_refusal(engine, &stream_request(handle, selected, past_end, window)).await,
        EngineError::OffsetPastEnd {
            offset: past_end,
            len: probe.file_len()
        }
    );

    assert_eq!(
        stream_refusal(
            engine,
            &stream_request(handle, probe.file_count(), Bytes::new(0), window)
        )
        .await,
        EngineError::UnknownFile {
            index: probe.file_count()
        },
        "a file the torrent does not have is a different refusal from one it is not downloading"
    );
}

/// A selection is a set that is replaced, and the two ways to ask for nothing are both refused.
async fn check_selection(engine: &dyn TorrentEngine, probe: &EngineProbe, handle: TorrentHandle) {
    let selection = probe.request().selection();
    let first = selection.indices()[0];
    // In range and not `first`, which the probe's two-file minimum guarantees exists.
    let other = usize::from(first == 0);

    engine
        .select(handle, selection)
        .await
        .expect("a live torrent accepts a selection it can serve");

    assert_eq!(
        engine.select(handle, &FileSelection::empty()).await,
        Err(EngineError::EmptySelection),
        "a torrent with nothing selected writes nothing, and that is a removal"
    );

    let too_far = FileSelection::new([probe.file_count()]);
    assert_eq!(
        engine.select(handle, &too_far).await,
        Err(EngineError::UnknownFile {
            index: probe.file_count()
        })
    );

    // Replacing the set with a subset stops the other file being readable, which is the whole
    // difference between replacing and adding to.
    engine
        .select(handle, &FileSelection::new([first]))
        .await
        .expect("a subset of the torrent is a selection");
    assert_eq!(
        stream_refusal(
            engine,
            &stream_request(handle, other, Bytes::new(0), probe.file_len())
        )
        .await,
        EngineError::FileNotSelected { index: other }
    );
}

/// Pause, resume, and removal are answered in every live state, and a removed handle names nothing.
async fn check_lifecycle(engine: &dyn TorrentEngine, probe: &EngineProbe, handle: TorrentHandle) {
    // Twice in a row each: a toggle dispatched twice must not raise an error at a user who asked
    // for the state they are already in.
    engine.pause(handle).await.expect("a live torrent pauses");
    engine
        .pause(handle)
        .await
        .expect("pausing a paused torrent is idempotent");
    engine
        .resume(handle)
        .await
        .expect("a paused torrent resumes");
    engine
        .resume(handle)
        .await
        .expect("resuming a running torrent is idempotent");

    engine
        .remove(handle)
        .await
        .expect("a live torrent is removed");
    engine
        .remove(handle)
        .await
        .expect("removing a removed torrent is idempotent");

    // Once removed, the handle names nothing, and every operation but `remove` says so.
    let expected = EngineError::UnknownTorrent { handle };
    assert_eq!(engine.stats(handle).await, Err(expected));
    assert_eq!(engine.state(handle).await, Err(expected));
    assert_eq!(engine.verify(handle).await, Err(expected));
    assert_eq!(engine.pause(handle).await, Err(expected));
    assert_eq!(engine.resume(handle).await, Err(expected));
    assert_eq!(
        engine.select(handle, probe.request().selection()).await,
        Err(expected)
    );
    assert_eq!(
        stream_refusal(
            engine,
            &stream_request(handle, 0, Bytes::new(0), probe.file_len())
        )
        .await,
        expected
    );
}

/// Limits take effect on the live session: applied and read back without a restart, which is what
/// reading them on a session that still answers proves.
async fn check_limits(engine: &dyn TorrentEngine, probe: &EngineProbe) {
    engine
        .set_limits(probe.limits())
        .await
        .expect("a live session applies limits");
    assert_eq!(
        engine
            .limits()
            .await
            .expect("a live session reports its limits"),
        *probe.limits(),
        "a limit applied at runtime is the limit in force, without restarting the session"
    );
}

/// After `shutdown`, every operation answers the closed-session refusal, and nothing hangs.
async fn check_closed(engine: &dyn TorrentEngine, probe: &EngineProbe, handle: TorrentHandle) {
    engine.shutdown().await.expect("a live session shuts down");
    engine
        .shutdown()
        .await
        .expect("shutting down twice is idempotent");

    // The session-level rule is checked before every other one, so a removed handle and a malformed
    // selection are both `SessionClosed` here rather than their own refusals.
    assert_eq!(
        engine.add(probe.request()).await,
        Err(EngineError::SessionClosed)
    );
    assert_eq!(
        engine.select(handle, &FileSelection::empty()).await,
        Err(EngineError::SessionClosed)
    );
    assert_eq!(engine.stats(handle).await, Err(EngineError::SessionClosed));
    assert_eq!(engine.state(handle).await, Err(EngineError::SessionClosed));
    assert_eq!(engine.verify(handle).await, Err(EngineError::SessionClosed));
    assert_eq!(engine.limits().await, Err(EngineError::SessionClosed));
    assert_eq!(
        engine.set_limits(probe.limits()).await,
        Err(EngineError::SessionClosed)
    );
    assert_eq!(engine.pause(handle).await, Err(EngineError::SessionClosed));
    assert_eq!(engine.resume(handle).await, Err(EngineError::SessionClosed));
    assert_eq!(engine.remove(handle).await, Err(EngineError::SessionClosed));
    assert_eq!(
        stream_refusal(
            engine,
            &stream_request(handle, 0, Bytes::new(0), probe.file_len())
        )
        .await,
        EngineError::SessionClosed
    );

    // Which engine was shut down is not a question about the session, so the descriptor answers.
    check_descriptor(engine);
}

#[cfg(test)]
mod tests {
    use super::*;

    use futures::executor::block_on;
    use jiff::civil::Date;
    use peregrine_domain::id::{ExternalId, InfoHash, Provider};
    use peregrine_domain::text::NonEmptyText;
    use peregrine_domain::{Clock, IdGenerator, TitleKind};
    use peregrine_providers::{GenreRef, ProviderPage};
    use peregrine_torrent::{
        AddIntent, EngineDescriptor, EngineFuture, EngineState, TorrentStream,
    };
    use std::num::NonZeroU32;

    use crate::torrent::tests::{FILE_COUNT, file_len, limits, request};
    use crate::{CountingIds, FakeClock, FakeMetadataProvider, FakeTorrentEngine, fake_descriptor};

    /// A candidate identified by `provider`, which is what the suite's identity rule is about.
    fn candidate(provider: Provider, id: &str, name: &str) -> SearchCandidate {
        SearchCandidate::new(
            ExternalId::new(provider, id).expect("the test passes a valid id"),
            NonEmptyText::new(name).expect("the test passes real text"),
            TitleKind::Show,
            Some(Date::constant(2008, 1, 20)),
        )
    }

    /// The fake must pass the same suite the production adapter passes. If this fails, the
    /// fake is not a stand-in for the port and every test built on it is measuring
    /// something else.
    #[test]
    fn the_fake_clock_satisfies_the_port() {
        let clock = FakeClock::default();
        let observed = clock_satisfies_the_port(|| clock.now());
        assert_eq!(observed.as_millisecond(), clock.now_millis());
    }

    /// Determinism is what makes the suite itself runnable against the fake: the pinned
    /// instant survives the whole suite, so nothing here needed a real clock.
    #[test]
    fn the_fake_clock_leaves_time_where_it_was() {
        let clock = FakeClock::at(1_700_000_000_123);
        let observed = clock_satisfies_the_port(|| clock.now());
        assert_eq!(observed.as_millisecond(), 1_700_000_000_123);
        assert_eq!(clock.now_millis(), 1_700_000_000_123);
    }

    #[test]
    fn the_fake_ids_satisfy_the_port() {
        let generator = CountingIds::default();
        let drawn = id_generator_satisfies_the_port(|| generator.next_id());
        assert_eq!(drawn.len(), CONFORMING_DRAWS);
        assert_eq!(generator.issued(), CONFORMING_DRAWS);
    }

    /// The suite is only worth running if a broken implementation fails it, so both halves
    /// are pinned against one that is broken on purpose.
    #[test]
    fn a_repeating_id_generator_fails_the_suite() {
        let outcome = std::panic::catch_unwind(|| {
            id_generator_satisfies_the_port(|| Ulid::from_parts(1_700_000_000_000, 0));
        });
        assert!(outcome.is_err(), "a constant generator must not conform");
    }

    #[test]
    fn a_clock_repeating_the_same_instant_conforms() {
        // A coarse clock is not a broken clock: repeating an instant is permitted, and the
        // suite must not be the thing that forbids it.
        let outcome = std::panic::catch_unwind(|| {
            clock_satisfies_the_port(|| Timestamp::from_millisecond(1_700_000_000_000).unwrap())
        });
        assert!(outcome.is_ok(), "a coarse clock is still a clock");
    }

    /// A provider that declares `search` and answers the probe.
    fn searching_fake(candidates: Vec<SearchCandidate>) -> FakeMetadataProvider {
        FakeMetadataProvider::searching(Provider::Tmdb, candidates)
    }

    fn probe() -> SearchQuery {
        SearchQuery::new(
            NonEmptyText::new("breaking bad").expect("real text"),
            NonZeroU32::new(10).expect("ten is not zero"),
            None,
        )
    }

    fn catch_async<F: std::future::Future>(
        future: F,
    ) -> Result<F::Output, Box<dyn std::any::Any + Send>> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| block_on(future)))
    }

    #[test]
    fn the_fake_provider_satisfies_the_port() {
        // The fake must pass the same suite every adapter passes. If this fails, the fake is
        // not a stand-in for the port and every test built on it is measuring something else.
        // The fixture carries the fields a list answer adds, because a rule the suite states
        // about them has to be satisfied by the fake the suite is demonstrated with.
        let fake = searching_fake(vec![
            candidate(Provider::Tmdb, "1396", "Breaking Bad")
                .with_artwork_path(Some("/poster.jpg".to_owned()))
                .with_backdrop_path(Some("/backdrop.jpg".to_owned()))
                .with_rating(Some(9.2)),
        ]);
        let answered = block_on(metadata_provider_satisfies_the_port(
            &fake,
            &probe(),
            RailKind::PopularMovies,
            page(1),
        ));

        assert_eq!(answered.len(), 1);
    }

    /// One page, for the rail probe. `NonZeroU32` because that is what the port takes.
    fn page(number: u32) -> NonZeroU32 {
        NonZeroU32::new(number).expect("the test passes a non-zero page")
    }

    #[test]
    fn a_provider_without_the_capability_satisfies_the_port_by_refusing_it() {
        // The other branch of the same contract: a provider that does not declare search must
        // refuse it, and that refusal is what the suite accepts in place of an answer.
        let silent = FakeMetadataProvider::new(
            fake_descriptor(
                Provider::Tvdb,
                peregrine_providers::Capabilities::EMPTY.with(Capability::Trending),
            ),
            Ok(vec![candidate(Provider::Tvdb, "81189", "Breaking Bad")]),
        );

        let answered = block_on(metadata_provider_satisfies_the_port(
            &silent,
            &probe(),
            RailKind::PopularMovies,
            page(1),
        ));

        assert_eq!(answered, Vec::new());
    }

    #[test]
    fn the_suite_is_only_worth_running_if_a_broken_provider_fails_it() {
        // Four ways a provider can break the contract, each pinned against one that does.
        let liar = searching_fake(vec![candidate(Provider::Tvdb, "81189", "Breaking Bad")]);
        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &liar,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err(),
            "a provider answering with another provider's ids must not conform"
        );

        let over_answering = searching_fake(
            (0..11)
                .map(|index| candidate(Provider::Tmdb, &index.to_string(), "Breaking Bad"))
                .collect(),
        );
        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &over_answering,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err(),
            "a provider ignoring the limit must not conform"
        );

        let failing = FakeMetadataProvider::failing(
            Provider::Tmdb,
            ProviderError::Transport {
                provider: Provider::Tmdb,
                kind: peregrine_providers::TransportFailure::Timeout,
            },
        );
        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &failing,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err(),
            "a provider that declares search and then fails must not conform"
        );

        let blank_artwork = searching_fake(vec![
            candidate(Provider::Tmdb, "603", "The Matrix")
                .with_artwork_path(Some("   ".to_owned())),
        ]);
        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &blank_artwork,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err(),
            "artwork that is only whitespace names no image and must not conform"
        );

        let blank_backdrop = searching_fake(vec![
            candidate(Provider::Tmdb, "603", "The Matrix")
                .with_backdrop_path(Some("  ".to_owned())),
        ]);
        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &blank_backdrop,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err(),
            "a backdrop that is only whitespace names no image and must not conform"
        );
    }

    #[test]
    fn a_provider_that_declares_trending_must_answer_a_rail() {
        let publishing = FakeMetadataProvider::publishing_rails(
            Provider::Tmdb,
            vec![candidate(Provider::Tmdb, "603", "The Matrix")],
        );

        block_on(metadata_provider_satisfies_the_port(
            &publishing,
            &probe(),
            RailKind::PopularMovies,
            page(1),
        ));
    }

    #[test]
    fn a_provider_without_trending_satisfies_the_port_by_refusing_a_rail() {
        let silent = FakeMetadataProvider::searching(Provider::Tvdb, Vec::new());

        block_on(metadata_provider_satisfies_the_port(
            &silent,
            &probe(),
            RailKind::PopularMovies,
            page(1),
        ));
    }

    #[test]
    fn the_rail_half_of_the_suite_fails_a_provider_that_misattributes_or_over_answers() {
        // Two ways to break the rail contract: a candidate identified by another provider — the
        // coupling ADR 0007 rule 2 exists to prevent, which the fake can be handed and cannot
        // produce by accident — and a page larger than a page.
        let liar = FakeMetadataProvider::publishing_rails(
            Provider::Tmdb,
            vec![candidate(Provider::Tvdb, "81189", "Breaking Bad")],
        );
        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &liar,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err(),
            "a rail answering with another provider's ids must not conform"
        );

        let over_answering = FakeMetadataProvider::publishing_rails(
            Provider::Tmdb,
            (0..=RAIL_PAGE_LIMIT)
                .map(|index| candidate(Provider::Tmdb, &index.to_string(), "Breaking Bad"))
                .collect(),
        );
        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &over_answering,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err(),
            "a rail answering more than a page must not conform"
        );
    }

    #[test]
    // The length is the fixture rather than the logic: a misnaming provider has to implement every
    // operation the port has, and each one is a few lines of refusal. Moving it to a module-level
    // struct would move those lines rather than shorten the file, and the fixture means something only
    // beside the test that reads it.
    #[expect(
        clippy::too_many_lines,
        reason = "an inline provider fixture, not logic"
    )]
    fn a_rail_refusal_that_names_the_wrong_provider_fails_the_suite() {
        // The same rule the search refusal is held to, applied to the rail: an `Unsupported`
        // naming a provider nobody asked is a user sent to the wrong settings pane. The fixture
        // declares `trending` — so it must answer — and refuses in another provider's name.
        struct MisnamingRail {
            descriptor: peregrine_providers::ProviderDescriptor,
        }

        impl MetadataProvider for MisnamingRail {
            fn descriptor(&self) -> &peregrine_providers::ProviderDescriptor {
                &self.descriptor
            }

            fn check(&self) -> peregrine_providers::ProviderFuture<'_, ()> {
                Box::pin(async { Ok(()) })
            }

            fn image(
                &self,
                _path: &str,
                _kind: peregrine_domain::catalogue::ImageKind,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<u8>> {
                Box::pin(async { Ok(Vec::new()) })
            }

            fn title(
                &self,
                _id: &peregrine_domain::id::ExternalId,
                _kind: peregrine_domain::catalogue::TitleKind,
            ) -> peregrine_providers::ProviderFuture<'_, peregrine_domain::catalogue::TitleDetails>
            {
                Box::pin(async {
                    Ok(peregrine_domain::catalogue::TitleDetails {
                        kind: peregrine_domain::TitleKind::Movie,
                        name: NonEmptyText::new("A title").expect("the fixture is not empty"),
                        original_name: None,
                        overview: None,
                        tagline: None,
                        released: None,
                        runtime_minutes: None,
                        original_language: None,
                        origin_country: None,
                        rating: None,
                        votes: None,
                        popularity: None,
                        adult: None,
                        in_production: None,
                        season_count: None,
                        episode_count: None,
                        homepage: None,
                        poster_path: None,
                        backdrop_path: None,
                    })
                })
            }

            fn search(
                &self,
                _query: &SearchQuery,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<SearchCandidate>> {
                Box::pin(async { Ok(Vec::new()) })
            }

            fn rail(
                &self,
                _rail: RailKind,
                _page: NonZeroU32,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<SearchCandidate>> {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tvdb,
                        capability: Capability::Trending.as_str(),
                    })
                })
            }

            fn browse(
                &self,
                _request: &BrowseRequest,
            ) -> peregrine_providers::ProviderFuture<'_, peregrine_providers::ProviderPage>
            {
                // Declares `trending`, so this must answer or refuse in its own name: the fixture
                // misnames a *rail*, and a browse that failed for another rule would mask it.
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tmdb,
                        capability: Capability::Trending.as_str(),
                    })
                })
            }

            fn genres(
                &self,
                _kind: peregrine_domain::catalogue::TitleKind,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<peregrine_providers::GenreRef>>
            {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tmdb,
                        capability: Capability::Trending.as_str(),
                    })
                })
            }

            fn similar(
                &self,
                _id: &peregrine_domain::id::ExternalId,
                _kind: peregrine_domain::catalogue::TitleKind,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<SearchCandidate>> {
                // Declares `trending`, so this refuses in its own name and the rail misnaming below
                // is what the suite fails on.
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tmdb,
                        capability: Capability::Trending.as_str(),
                    })
                })
            }

            /// Declares only `search` and `trending`, so a season read is the port's refusal in this
            /// fixture's own name.
            fn seasons(
                &self,
                _id: &peregrine_domain::id::ExternalId,
                _kind: peregrine_domain::catalogue::TitleKind,
            ) -> peregrine_providers::ProviderFuture<
                '_,
                Vec<peregrine_domain::catalogue::SeasonDetails>,
            > {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tmdb,
                        capability: Capability::Season.as_str(),
                    })
                })
            }

            /// The same, for the season's episodes.
            fn season_episodes(
                &self,
                _id: &peregrine_domain::id::ExternalId,
                _kind: peregrine_domain::catalogue::TitleKind,
                _season_number: peregrine_domain::catalogue::SeasonNumber,
            ) -> peregrine_providers::ProviderFuture<
                '_,
                Vec<peregrine_domain::catalogue::EpisodeDetails>,
            > {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tmdb,
                        capability: Capability::Season.as_str(),
                    })
                })
            }
        }

        let wrong = MisnamingRail {
            descriptor: fake_descriptor(
                Provider::Tmdb,
                peregrine_providers::Capabilities::EMPTY
                    .with(Capability::Search)
                    .with(Capability::Trending),
            ),
        };

        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &wrong,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err(),
            "a rail refusal naming another provider must not conform"
        );
    }

    #[test]
    // The same reason as the rail fixture above: the provider body is a fixture, not logic.
    #[expect(
        clippy::too_many_lines,
        reason = "an inline provider fixture, not logic"
    )]
    fn a_refusal_that_names_the_wrong_provider_fails_the_suite() {
        // The refusal is checked, not merely accepted: an `Unsupported` naming a provider
        // that was not asked would send a user to the wrong settings pane. The fake cannot
        // produce this — it refuses with its own name, which is the behaviour the suite
        // requires — so the misnaming is written out here.
        struct Misnaming {
            descriptor: peregrine_providers::ProviderDescriptor,
        }

        impl MetadataProvider for Misnaming {
            fn descriptor(&self) -> &peregrine_providers::ProviderDescriptor {
                &self.descriptor
            }

            fn check(&self) -> peregrine_providers::ProviderFuture<'_, ()> {
                // Healthy here on purpose: this fixture exists to misname a *search* refusal, and a
                // probe that failed first would make the suite error for a different reason than the
                // one this test is named for. The probe's own misattribution has its own fixture.
                Box::pin(async { Ok(()) })
            }

            fn image(
                &self,
                _path: &str,
                _kind: peregrine_domain::catalogue::ImageKind,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<u8>> {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tmdb,
                        capability: "images",
                    })
                })
            }

            fn title(
                &self,
                _id: &peregrine_domain::id::ExternalId,
                _kind: peregrine_domain::catalogue::TitleKind,
            ) -> peregrine_providers::ProviderFuture<'_, peregrine_domain::catalogue::TitleDetails>
            {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tmdb,
                        capability: "title",
                    })
                })
            }

            fn search(
                &self,
                _query: &SearchQuery,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<SearchCandidate>> {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tmdb,
                        capability: Capability::Search.as_str(),
                    })
                })
            }

            fn rail(
                &self,
                _rail: RailKind,
                _page: NonZeroU32,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<SearchCandidate>> {
                // Healthy on purpose, for the reason the probe is: this fixture misnames a *search*
                // refusal, and a rail refined for a different reason would mask it.
                Box::pin(async { Ok(Vec::new()) })
            }

            fn browse(
                &self,
                _request: &BrowseRequest,
            ) -> peregrine_providers::ProviderFuture<'_, peregrine_providers::ProviderPage>
            {
                // Declares `trending`, so this refuses in its own name and the search misnaming
                // below is what the suite fails on.
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tvdb,
                        capability: Capability::Trending.as_str(),
                    })
                })
            }

            fn genres(
                &self,
                _kind: peregrine_domain::catalogue::TitleKind,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<peregrine_providers::GenreRef>>
            {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tvdb,
                        capability: Capability::Trending.as_str(),
                    })
                })
            }

            fn similar(
                &self,
                _id: &peregrine_domain::id::ExternalId,
                _kind: peregrine_domain::catalogue::TitleKind,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<SearchCandidate>> {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tvdb,
                        capability: Capability::Trending.as_str(),
                    })
                })
            }

            fn seasons(
                &self,
                _id: &peregrine_domain::id::ExternalId,
                _kind: peregrine_domain::catalogue::TitleKind,
            ) -> peregrine_providers::ProviderFuture<
                '_,
                Vec<peregrine_domain::catalogue::SeasonDetails>,
            > {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tvdb,
                        capability: Capability::Season.as_str(),
                    })
                })
            }

            fn season_episodes(
                &self,
                _id: &peregrine_domain::id::ExternalId,
                _kind: peregrine_domain::catalogue::TitleKind,
                _season_number: peregrine_domain::catalogue::SeasonNumber,
            ) -> peregrine_providers::ProviderFuture<
                '_,
                Vec<peregrine_domain::catalogue::EpisodeDetails>,
            > {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tvdb,
                        capability: Capability::Season.as_str(),
                    })
                })
            }
        }

        let wrong = Misnaming {
            descriptor: fake_descriptor(
                Provider::Tvdb,
                peregrine_providers::Capabilities::EMPTY.with(Capability::Trending),
            ),
        };

        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &wrong,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err()
        );
    }

    #[test]
    // The same reason as the rail fixture above: the provider body is a fixture, not logic.
    #[expect(
        clippy::too_many_lines,
        reason = "an inline provider fixture, not logic"
    )]
    fn a_probe_failure_that_names_the_wrong_provider_fails_the_suite() {
        // The probe's half of the same rule. A refused credential attributed to a provider nobody
        // asked would put the right error in front of the wrong settings row, which is worse than the
        // generic failure PG-E5-20 exists to remove.
        struct Misprobe {
            descriptor: peregrine_providers::ProviderDescriptor,
        }

        impl MetadataProvider for Misprobe {
            fn descriptor(&self) -> &peregrine_providers::ProviderDescriptor {
                &self.descriptor
            }

            fn check(&self) -> peregrine_providers::ProviderFuture<'_, ()> {
                Box::pin(async {
                    Err(ProviderError::Unauthorized {
                        provider: Provider::Tmdb,
                    })
                })
            }

            fn image(
                &self,
                _path: &str,
                _kind: peregrine_domain::catalogue::ImageKind,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<u8>> {
                Box::pin(async { Ok(Vec::new()) })
            }

            fn title(
                &self,
                _id: &peregrine_domain::id::ExternalId,
                _kind: peregrine_domain::catalogue::TitleKind,
            ) -> peregrine_providers::ProviderFuture<'_, peregrine_domain::catalogue::TitleDetails>
            {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tvdb,
                        capability: "title",
                    })
                })
            }

            fn search(
                &self,
                _query: &SearchQuery,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<SearchCandidate>> {
                Box::pin(async { Ok(Vec::new()) })
            }

            fn rail(
                &self,
                _rail: RailKind,
                _page: NonZeroU32,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<SearchCandidate>> {
                // Declares `trending` in its descriptor below, so this must answer rather than
                // refuse: the fixture exists to misname a *probe* failure, and a rail refusal here
                // would panic for a different rule than the one this test is named for.
                Box::pin(async { Ok(Vec::new()) })
            }

            fn browse(
                &self,
                _request: &BrowseRequest,
            ) -> peregrine_providers::ProviderFuture<'_, peregrine_providers::ProviderPage>
            {
                // The probe check panics before this is reached, so it only has to compile and to
                // refuse in its own name if it ever is.
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tvdb,
                        capability: Capability::Trending.as_str(),
                    })
                })
            }

            fn genres(
                &self,
                _kind: peregrine_domain::catalogue::TitleKind,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<peregrine_providers::GenreRef>>
            {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tvdb,
                        capability: Capability::Trending.as_str(),
                    })
                })
            }

            fn similar(
                &self,
                _id: &peregrine_domain::id::ExternalId,
                _kind: peregrine_domain::catalogue::TitleKind,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<SearchCandidate>> {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tvdb,
                        capability: Capability::Trending.as_str(),
                    })
                })
            }

            fn seasons(
                &self,
                _id: &peregrine_domain::id::ExternalId,
                _kind: peregrine_domain::catalogue::TitleKind,
            ) -> peregrine_providers::ProviderFuture<
                '_,
                Vec<peregrine_domain::catalogue::SeasonDetails>,
            > {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tvdb,
                        capability: Capability::Season.as_str(),
                    })
                })
            }

            fn season_episodes(
                &self,
                _id: &peregrine_domain::id::ExternalId,
                _kind: peregrine_domain::catalogue::TitleKind,
                _season_number: peregrine_domain::catalogue::SeasonNumber,
            ) -> peregrine_providers::ProviderFuture<
                '_,
                Vec<peregrine_domain::catalogue::EpisodeDetails>,
            > {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tvdb,
                        capability: Capability::Season.as_str(),
                    })
                })
            }
        }

        let wrong = Misprobe {
            descriptor: fake_descriptor(
                Provider::Tvdb,
                peregrine_providers::Capabilities::EMPTY.with(Capability::Search),
            ),
        };

        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &wrong,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err()
        );
    }

    #[test]
    fn a_provider_that_declares_trending_and_answers_a_browse_genres_and_similar_conforms() {
        // The positive half, so the tests below are known to fail for their own reason rather than
        // because every browse, genre list, and recommendation refuses.
        let answering = FakeMetadataProvider::publishing_rails(
            Provider::Tmdb,
            vec![candidate(Provider::Tmdb, "603", "The Matrix")],
        )
        .with_browse_answer(ProviderPage::new(
            vec![candidate(Provider::Tmdb, "603", "The Matrix")],
            true,
        ))
        .with_genres_answer(vec![GenreRef::new(
            28,
            NonEmptyText::new("Action").expect("the fixture is not empty"),
        )])
        .with_similar_answer(vec![candidate(
            Provider::Tmdb,
            "604",
            "The Matrix Reloaded",
        )]);

        block_on(metadata_provider_satisfies_the_port(
            &answering,
            &probe(),
            RailKind::PopularMovies,
            page(1),
        ));
    }

    #[test]
    fn the_similar_half_of_the_suite_fails_an_over_long_recommendation_page() {
        // A recommendation read is a list like any other, so it holds at most a page: a provider that
        // answered more would have a rail draw a strip longer than the strip the screen sized.
        let too_many = FakeMetadataProvider::publishing_rails(Provider::Tmdb, Vec::new())
            .with_similar_answer(
                (0..=RAIL_PAGE_LIMIT)
                    .map(|n| candidate(Provider::Tmdb, &n.to_string(), "A title"))
                    .collect(),
            );
        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &too_many,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err(),
            "a recommendation page larger than a page must not conform"
        );
    }

    #[test]
    fn the_browse_half_of_the_suite_fails_an_over_long_page_or_one_that_lies_about_more() {
        // Two ways to break the browse contract that a rail cannot express: a page larger than a
        // page, and an empty page that claims another one follows — the second is what a screen's
        // "load more" hangs off, and it sends a reader to nothing for ever.
        let too_long = FakeMetadataProvider::publishing_rails(Provider::Tmdb, Vec::new())
            .with_browse_answer(ProviderPage::new(
                (0..=RAIL_PAGE_LIMIT)
                    .map(|n| candidate(Provider::Tmdb, &n.to_string(), "A title"))
                    .collect(),
                false,
            ));
        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &too_long,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err(),
            "a browse page larger than a page must not conform"
        );

        let lying = FakeMetadataProvider::publishing_rails(Provider::Tmdb, Vec::new())
            .with_browse_answer(ProviderPage::new(Vec::new(), true));
        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &lying,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err(),
            "an empty page that claims another follows must not conform"
        );
    }

    #[test]
    fn the_genre_half_of_the_suite_fails_a_list_that_repeats_an_identifier() {
        // A repeated id is a control whose two options cannot be told apart. The other rule a
        // reader might expect here — that a genre must be named — is not one this suite can catch,
        // because `NonEmptyText` makes a nameless `GenreRef` unrepresentable at the adapter.
        let repeated = FakeMetadataProvider::publishing_rails(Provider::Tmdb, Vec::new())
            .with_genres_answer(vec![
                GenreRef::new(
                    28,
                    NonEmptyText::new("Action").expect("the fixture is not empty"),
                ),
                GenreRef::new(
                    28,
                    NonEmptyText::new("Adventure").expect("the fixture is not empty"),
                ),
            ]);
        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &repeated,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err(),
            "the same genre id twice must not conform"
        );
    }

    /// The probe the torrent suite is driven with: one fixture torrent, added fresh and added as a
    /// reattach, over a file list and limits the engine is told about.
    fn torrent_probe() -> EngineProbe {
        EngineProbe::new(
            request(AddIntent::Download),
            request(AddIntent::Reattach { state: None }),
            FILE_COUNT,
            file_len(),
            limits(),
        )
    }

    /// The fake must pass the same suite the librqbit adapter will pass (PG-E10-02). If this fails,
    /// the fake is not a stand-in for the port and every test built on it is measuring something
    /// else.
    #[test]
    fn the_fake_torrent_engine_satisfies_the_port() {
        let probe = torrent_probe();
        let engine = FakeTorrentEngine::new().with_files(probe.file_count(), probe.file_len());

        block_on(torrent_engine_satisfies_the_port(&engine, &probe));
    }

    /// An engine that answers about the wrong torrent, to prove the identity rule can fail.
    ///
    /// It delegates every operation to the fake and rewrites only `stats`, which is the smallest
    /// break the suite's identity check exists to catch: a suite that asked only "did `stats`
    /// answer" would pass this.
    struct Misreporting<'a> {
        inner: &'a FakeTorrentEngine,
        wrong: InfoHash,
    }

    impl TorrentEngine for Misreporting<'_> {
        fn descriptor(&self) -> &EngineDescriptor {
            self.inner.descriptor()
        }

        fn add(&self, request: &AddRequest) -> EngineFuture<'_, TorrentHandle> {
            self.inner.add(request)
        }

        fn select(&self, handle: TorrentHandle, selection: &FileSelection) -> EngineFuture<'_, ()> {
            self.inner.select(handle, selection)
        }

        fn stream(&self, request: &StreamRequest) -> EngineFuture<'_, Box<dyn TorrentStream>> {
            self.inner.stream(request)
        }

        fn stats(&self, handle: TorrentHandle) -> EngineFuture<'_, TorrentStats> {
            let inner = self.inner.stats(handle);
            let wrong = self.wrong;
            Box::pin(async move {
                inner.await.map(|stats| {
                    TorrentStats::new(
                        wrong,
                        stats.transfer(),
                        stats.total(),
                        stats.peers(),
                        stats.eta(),
                        stats.health(),
                    )
                })
            })
        }

        fn state(&self, handle: TorrentHandle) -> EngineFuture<'_, EngineState> {
            self.inner.state(handle)
        }

        fn verify(&self, handle: TorrentHandle) -> EngineFuture<'_, ()> {
            self.inner.verify(handle)
        }

        fn limits(&self) -> EngineFuture<'_, EngineLimits> {
            self.inner.limits()
        }

        fn set_limits(&self, limits: &EngineLimits) -> EngineFuture<'_, ()> {
            self.inner.set_limits(limits)
        }

        fn pause(&self, handle: TorrentHandle) -> EngineFuture<'_, ()> {
            self.inner.pause(handle)
        }

        fn resume(&self, handle: TorrentHandle) -> EngineFuture<'_, ()> {
            self.inner.resume(handle)
        }

        fn remove(&self, handle: TorrentHandle) -> EngineFuture<'_, ()> {
            self.inner.remove(handle)
        }

        fn shutdown(&self) -> EngineFuture<'_, ()> {
            self.inner.shutdown()
        }
    }

    #[test]
    fn an_engine_that_misreports_the_torrent_fails_the_suite() {
        let probe = torrent_probe();
        let fake = FakeTorrentEngine::new().with_files(probe.file_count(), probe.file_len());
        let misreporting = Misreporting {
            inner: &fake,
            wrong: InfoHash::from_hex("ffffffffffffffffffffffffffffffffffffffff")
                .expect("a valid hash"),
        };

        assert!(
            catch_async(torrent_engine_satisfies_the_port(&misreporting, &probe)).is_err(),
            "statistics about another torrent must not conform"
        );

        // The same probe against an honest engine conforms, so the failure above is the
        // misreporting rather than a probe the suite could never accept.
        let honest = FakeTorrentEngine::new().with_files(probe.file_count(), probe.file_len());
        block_on(torrent_engine_satisfies_the_port(&honest, &probe));
    }
}
