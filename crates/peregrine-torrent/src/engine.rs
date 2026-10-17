//! The `TorrentEngine` port.
//!
//! One trait, implemented once per `BitTorrent` engine, and talked to by the acquisition service and
//! the stream server. `FLOWS` F6 drives it: metadata is fetched and inspected first, and only then
//! is an allowed torrent added with an explicit file selection. `FLOWS` SM1's states are what the
//! job layer above it records; the engine itself holds a torrent, transfers it, and answers
//! counters.
//!
//! # Why the signature is a boxed future
//!
//! Every session operation reaches a runtime the adapter owns, so it is asynchronous, and the port
//! is `dyn`-compatible because the runtime composes one engine chosen from settings rather than a
//! `match` — and because ADR 0005 keeps a second adapter (libtorrent bindings) open as a leaf-crate
//! change. On this toolchain an `async fn` in a trait rules `dyn` out (`error[E0038]: the trait is
//! not dyn compatible`), and a concrete return type in its place would make every engine's future a
//! different type. So every method returns a boxed future through [`EngineFuture`], the same shape
//! `peregrine-providers` uses for `MetadataProvider`'s network calls.
//!
//! # Why the port's operations are these
//!
//! ADR 0005 names the surface — session lifecycle, add with file selection, sequential streaming
//! with a steerable priority window, statistics, limits, pause and resume, and removal — and each
//! operation here is one of those with the shape `FLOWS` F6, SM1, and `PG-E10-05` to `PG-E10-10`
//! already fix. Two of them are deliberately not what the ADR's nouns first suggest:
//!
//! - **`add` takes an [`AddRequest`], and that type cannot be built without an allowed
//!   [`SafetyReport`].** Inspection precedes any byte written is not a rule an adapter is asked to
//!   remember (SECURITY T12); it is the only way to spell an add.
//! - **The destination is a [`SafePath`].** The engine cannot be handed a path that did not come
//!   through `PathGuard`, because no other type in the signature can be built from arbitrary text
//!   (ADR 0005).
//!
//! The safety inspection itself is **not** here: it is `peregrine_domain::inspect`, it runs on
//! metadata only, and this port receives its verdict. The engine is also not where a job's state
//! machine lives — that is `peregrine_domain::download` — so an engine refusal and a rejected
//! transition are different things with different callers.
//!
//! # Recovery is an intent, not hidden behaviour
//!
//! A download survives a restart, and a finished one can be put back to seed, without a user
//! having to know which of those happened:
//!
//! - [`AddIntent::Reattach`] tells the engine that the payload may already be on disk, so it
//!   verifies what is there and transfers only what is missing rather than overwriting a verified
//!   byte (`PG-E10-08`). It is a typed intent rather than a flag the adapter infers, because
//!   "start this" and "this is already here" are different instructions and only the caller knows
//!   which it is giving.
//! - [`TorrentEngine::state`] hands the caller the session's own bytes for one torrent to persist
//!   (`download_job.engine_state_json`), and [`AddIntent::Reattach`] carries them back. The files
//!   on disk are the state that decides what verifies; this is the engine-private remainder.
//! - [`TorrentEngine::verify`] runs the same check on a torrent the session already holds, which is
//!   the on-demand half of a forced reseed (`PG-E10-09`).
//! - [`TorrentEngine::pause`] and [`TorrentEngine::resume`] work on a downloading torrent and on a
//!   seeding one, and a resume continues from the bytes already on disk.
//!
//! What is deliberately **not** here is a retry or a resurrect. `FLOWS` SM1 makes a retry after a
//! failure a *new job* with its own fresh inspection — `download_job_hash_uq` is unique only among
//! live jobs — so "start the download again" is the caller calling [`TorrentEngine::add`] once
//! more with a new report, and a retry operation here would be a second, weaker gate.
//!
//! Nor is policy here: ratio and time targets, when to seed, whether to reattach at launch, and
//! the value of every limit belong to the caller (`PG-E10-09`, `PG-E10-10`). The port exposes the
//! operations; it never decides when they run.
//!
//! # What every operation promises
//!
//! - **A closed session refuses.** After [`TorrentEngine::shutdown`], every operation answers
//!   [`EngineError::SessionClosed`] rather than hanging, panicking, or reaching a runtime that is
//!   gone. That rule is checked before any argument is judged, so it holds for a malformed call
//!   too.
//! - **A handle the session does not hold is refused** with [`EngineError::UnknownTorrent`], before
//!   the rest of the call is judged. The one exception is [`TorrentEngine::remove`], which is
//!   idempotent for the reason its doc gives.
//! - **An empty file set is refused**, by `add` and by `select`, with
//!   [`EngineError::EmptySelection`]. It is representable — the conformance suite drives it — and
//!   it is refused, because a torrent with nothing selected writes nothing and a caller that wants
//!   that wants removal.
//! - **Nothing panics and nothing blocks for ever.** An engine that cannot answer says so with a
//!   typed refusal; [`EngineError::Failed`] is the floor for anything this vocabulary does not
//!   name.

use std::fmt;
use std::future::Future;
use std::num::NonZeroU32;
use std::pin::Pin;

use peregrine_domain::id::InfoHash;
use peregrine_domain::inspect::{SafetyReport, TorrentDescription, Verdict};
use peregrine_domain::path::SafePath;
use peregrine_domain::text::NonEmptyText;

use crate::error::{AddRefused, EngineError};
use crate::stats::{ByteRate, Bytes, TorrentStats};

/// What a torrent-engine operation answers with, as a future the caller can await.
///
/// Named rather than written out because every method returns one, and a signature that spelled
/// `Pin<Box<dyn Future<Output = Result<…>> + Send + 'a>>` at each of them would be a wall of
/// punctuation around the part that matters.
pub type EngineFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, EngineError>> + Send + 'a>>;

/// Which engine this is, for Diagnostics and for a bug report.
///
/// Both fields are [`NonEmptyText`], so a Diagnostics row cannot draw a nameless engine or a blank
/// version: the type refuses the empty string where the adapter builds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineDescriptor {
    name: NonEmptyText,
    version: NonEmptyText,
}

impl EngineDescriptor {
    /// Names the engine and the version of it this build linked.
    #[must_use]
    pub const fn new(name: NonEmptyText, version: NonEmptyText) -> Self {
        Self { name, version }
    }

    /// What the engine calls itself.
    #[must_use]
    pub const fn name(&self) -> &NonEmptyText {
        &self.name
    }

    /// The version of the engine.
    #[must_use]
    pub const fn version(&self) -> &NonEmptyText {
        &self.version
    }
}

/// A handle to one torrent in one session.
///
/// Opaque to the caller and minted by the engine: the number is the session's own identifier for a
/// torrent it is holding, not an info-hash. It deliberately is not the info-hash, because a torrent
/// is held *before* its metadata has arrived — which is when a magnet's file list is still unknown
/// and when the safety gate has yet to run — and because a handle that formatted as a hash would be
/// a second value that leaks one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TorrentHandle(u64);

impl TorrentHandle {
    /// Wraps the session's own identifier.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The session's identifier, for a log line that must not carry an info-hash.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Where the torrent's bytes come from.
///
/// The two spellings are the two ways a torrent arrives: a magnet URI, which names its own
/// info-hash and fetches the rest from peers, or the bytes of a `.torrent` file that a local
/// indexer or a file picker produced. A `.torrent`'s info dictionary has to be hashed to know what
/// it is, which is the engine's work and not a parser's.
#[derive(Clone, PartialEq, Eq)]
pub enum TorrentSource {
    /// A magnet URI, as the indexer or the user gave it.
    Magnet(NonEmptyText),
    /// The bytes of a `.torrent` file.
    TorrentFile(Vec<u8>),
}

impl fmt::Debug for TorrentSource {
    /// Prints which kind it is and, for a file, how large it is — never the magnet.
    ///
    /// A magnet URI carries the info-hash in its `xt` and a tracker list besides, and
    /// `peregrine_domain::redaction` refuses `magnet:` in any user-visible text for exactly that
    /// reason (SECURITY T13). A derive here would put it into the first `tracing` field that
    /// formatted an add request.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Magnet(_) => formatter.write_str("Magnet(..)"),
            Self::TorrentFile(bytes) => write!(formatter, "TorrentFile({} bytes)", bytes.len()),
        }
    }
}

/// Which files of a torrent are being written.
///
/// A selection is a *set*: the constructor sorts and de-duplicates, so two spellings of one
/// selection compare equal and a caller can compare a live selection with a requested one. It is a
/// set of indices into the metadata's file list rather than a set of paths, because a path in a
/// torrent's metadata is untrusted text (`peregrine_domain::inspect`) and only the engine knows what
/// an index names.
///
/// It may be empty, and an empty one is refused by `add` and by `select`: a torrent with nothing
/// selected writes nothing, which is a removal, and a transfer that never moves is the failure a
/// user cannot diagnose.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileSelection {
    indices: Vec<usize>,
}

impl FileSelection {
    /// The selection naming `indices`, in canonical order.
    #[must_use]
    pub fn new(indices: impl IntoIterator<Item = usize>) -> Self {
        let mut indices: Vec<usize> = indices.into_iter().collect();
        indices.sort_unstable();
        indices.dedup();
        Self { indices }
    }

    /// The selection naming nothing.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            indices: Vec::new(),
        }
    }

    /// The selected file indices, ascending and without repetition.
    #[must_use]
    pub fn indices(&self) -> &[usize] {
        &self.indices
    }

    /// Whether no file is selected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// How many files are selected.
    #[must_use]
    pub fn len(&self) -> usize {
        self.indices.len()
    }

    /// Whether `index` is in the selection.
    #[must_use]
    pub fn contains(&self, index: usize) -> bool {
        self.indices.binary_search(&index).is_ok()
    }
}

/// The session's own state for one torrent, as the bytes a caller persists.
///
/// Opaque on purpose. `download_job.engine_state_json` stores it, and what is inside — a tracker
/// set, persistent options, peer bookkeeping — is the adapter's, so naming a shape here would be
/// inventing the format `PG-E10-08` owns (AGENTS R8). The payload files are the state that decides
/// what verifies; this is only what a session cannot reconstruct from them.
#[derive(Clone, PartialEq, Eq)]
pub struct EngineState(Vec<u8>);

impl EngineState {
    /// Wraps bytes the engine produced, or bytes it produced earlier and a caller kept.
    #[must_use]
    pub const fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// The bytes, to store or to hand back through [`AddIntent::Reattach`].
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// How many bytes of state there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there is no state at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for EngineState {
    /// Prints how much state there is and nothing of it.
    ///
    /// What an engine persists can carry tracker URLs and peer addresses, and a byte dump is
    /// unreadable in a log line besides.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "EngineState({} bytes)", self.0.len())
    }
}

/// What the caller intends the engine to do with the payload that may already be on disk.
///
/// The two intents are the two instructions a download button and a reseed button give. The
/// difference is not a hint the adapter could infer: whether a directory that happens to hold files
/// of the right names is this torrent's payload is a question only the caller's records can answer,
/// which is why the intent is typed rather than derived from the destination's contents.
///
/// **Neither intent relaxes the gate.** [`AddRequest::new`] refuses to build a request whose
/// report is not [`Verdict::Allowed`], so a forced reseed re-adds through the same inspection as a
/// first download and carries its own report (SECURITY T12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddIntent {
    /// A new transfer. Nothing of this torrent is assumed to be on disk.
    Download,
    /// The payload may already be on disk — a session reattaching after a restart, or a finished
    /// job being put back to seed (`PG-E10-08`, `PG-E10-09`).
    ///
    /// The engine verifies what is there and transfers only what fails verification, never
    /// overwriting a byte that verified. Verification and reseeding are one operation because the
    /// only difference between them is what the check finds.
    Reattach {
        /// What the previous session persisted for this torrent, when the caller still has it.
        ///
        /// `None` is a real case and not an error: a finished job's state may be gone, and its
        /// payload is still verifiable from the files and the metadata alone.
        state: Option<EngineState>,
    },
}

impl AddIntent {
    /// Whether the payload may already be on disk.
    #[must_use]
    pub const fn verifies_existing_data(&self) -> bool {
        matches!(self, Self::Reattach { .. })
    }
}

/// Everything [`TorrentEngine::add`] is given, after the safety gate has allowed the torrent.
///
/// **The only constructor takes a [`SafetyReport`] and refuses to build unless its verdict is
/// [`Verdict::Allowed`].** SECURITY T12 and `FLOWS` F6 sequence the two facts this encodes:
/// inspection completes before a byte is written, and the engine is handed an allowed torrent
/// rather than trusted to ask for one. A report that requires the advanced opt-in or blocks the
/// torrent is an [`AddRefused`] naming the verdict, so "add this anyway" has no spelling — and
/// that holds for a [`AddIntent::Reattach`] too, because a reseed writes bytes back into the same
/// directory a first download would.
///
/// The destination is a [`SafePath`] and never a `PathBuf` or a `String`, so the engine cannot
/// receive a path that did not come through `PathGuard` (ADR 0005).
///
/// The report is read for its verdict and dropped. It is not retained because a report carries the
/// entry paths of a torrent a stranger wrote, and a request holding one would put them one
/// `Debug` — or one forwarded field — away from a log line.
#[derive(Clone, PartialEq, Eq)]
pub struct AddRequest {
    info_hash: InfoHash,
    source: TorrentSource,
    destination: SafePath,
    selection: FileSelection,
    sequential: bool,
    intent: AddIntent,
}

impl AddRequest {
    /// Builds an add for an inspected torrent.
    ///
    /// # Errors
    ///
    /// [`AddRefused::OptInRequired`] when the report still needs the user's advanced opt-in, and
    /// [`AddRefused::Blocked`] when the report refused the torrent outright.
    pub fn new(
        info_hash: InfoHash,
        source: TorrentSource,
        destination: SafePath,
        selection: FileSelection,
        sequential: bool,
        intent: AddIntent,
        report: &SafetyReport,
    ) -> Result<Self, AddRefused> {
        match report.verdict() {
            Verdict::Allowed => Ok(Self {
                info_hash,
                source,
                destination,
                selection,
                sequential,
                intent,
            }),
            Verdict::OptInRequired => Err(AddRefused::OptInRequired),
            Verdict::Blocked => Err(AddRefused::Blocked),
        }
    }

    /// The identity the caller is adding the torrent as.
    ///
    /// It comes from the indexer result, the magnet, or the job row, and the engine is the first
    /// thing that can check it against what the source actually hashes to; a mismatch is
    /// [`EngineError::IdentityMismatch`] rather than a silently different torrent.
    #[must_use]
    pub const fn info_hash(&self) -> InfoHash {
        self.info_hash
    }

    /// Where the torrent comes from.
    #[must_use]
    pub const fn source(&self) -> &TorrentSource {
        &self.source
    }

    /// Where the bytes may be written.
    #[must_use]
    pub const fn destination(&self) -> &SafePath {
        &self.destination
    }

    /// Which files of it to write.
    #[must_use]
    pub const fn selection(&self) -> &FileSelection {
        &self.selection
    }

    /// Whether pieces are downloaded in file order, which is what makes a partly downloaded file
    /// playable (`PG-E10-05`).
    #[must_use]
    pub const fn sequential(&self) -> bool {
        self.sequential
    }

    /// Whether to start a transfer or to verify and reuse what is already on disk.
    #[must_use]
    pub const fn intent(&self) -> &AddIntent {
        &self.intent
    }
}

impl fmt::Debug for AddRequest {
    /// Prints what the request is about, and neither its source's URI nor its destination.
    ///
    /// The source can be a magnet (SECURITY T13) and the destination is a path under the user's
    /// home, which the diagnostics bundle is exported with. The info-hash and the persisted state
    /// appear only through their own redacting `Debug`s, so this line is safe to log.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AddRequest")
            .field("info_hash", &self.info_hash)
            .field("source", &self.source)
            .field("files", &self.selection.len())
            .field("sequential", &self.sequential)
            .field("intent", &self.intent)
            .finish_non_exhaustive()
    }
}

/// What [`TorrentEngine::stream`] asks for: one selected file, from one offset, with a read-ahead
/// window.
///
/// The window is the steering input `PG-E10-05` names. The playback service derives it from the
/// media's bitrate, so it arrives as data rather than as an engine setting. A window of zero means
/// no read-ahead beyond the piece the reader is waiting on, which is what the "download in order"
/// toggle leaves behind outside the window; the port does not refuse it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamRequest {
    handle: TorrentHandle,
    file: usize,
    from: Bytes,
    window: Bytes,
}

impl StreamRequest {
    /// Opens a reader over file `file`, starting at `from`, reading `window` bytes ahead.
    #[must_use]
    pub const fn new(handle: TorrentHandle, file: usize, from: Bytes, window: Bytes) -> Self {
        Self {
            handle,
            file,
            from,
            window,
        }
    }

    /// The torrent to read from.
    #[must_use]
    pub const fn handle(&self) -> TorrentHandle {
        self.handle
    }

    /// The index of the file to read, which must be in the torrent's selection.
    #[must_use]
    pub const fn file(&self) -> usize {
        self.file
    }

    /// The byte offset to read from. A reader opened at exactly the file's length has nothing left;
    /// one past it is refused.
    #[must_use]
    pub const fn from(&self) -> Bytes {
        self.from
    }

    /// How many bytes ahead of the read position to raise piece priority for.
    #[must_use]
    pub const fn window(&self) -> Bytes {
        self.window
    }
}

/// What one read answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamChunk {
    /// Bytes from the position the read started at.
    ///
    /// Never empty and never longer than the read's `max_bytes`: the engine waits for the first
    /// byte rather than answering nothing, because "no bytes yet" and "the file ended" are two
    /// different answers, and a caller that could not tell them apart would spin.
    Data(Vec<u8>),
    /// The read position reached the file's end.
    ///
    /// A read at the end answers this every time rather than failing, so a caller's loop can treat
    /// it as the terminator.
    End,
}

/// A sequential reader over one selected file of a live torrent.
///
/// Reading is what steers piece priority: the adapter moves the read-ahead window as
/// [`TorrentStream::position`] advances, so a player that has read the first megabyte has asked for
/// the next window's worth of pieces without naming a piece (`PG-E10-05`, `PG-E10-06`). There is
/// deliberately no seek — a caller that wants a different offset opens a fresh stream, which is
/// also how a reader tells the engine the read position jumped rather than advanced.
pub trait TorrentStream: Send {
    /// The file's length in bytes, as the torrent's metadata reports it.
    fn len(&self) -> Bytes;

    /// Whether the file has no bytes at all.
    ///
    /// Derived from [`TorrentStream::len`], so an implementation does not have to keep a second
    /// answer to the same question in step with the first.
    fn is_empty(&self) -> bool {
        self.len().get() == 0
    }

    /// The offset the next read answers from.
    fn position(&self) -> Bytes;

    /// Reads the next chunk, waiting for the bytes when they are not local yet.
    ///
    /// `max_bytes` is a [`NonZeroU32`], so "read nothing" is unrepresentable rather than a call
    /// that returns immediately and spins a caller.
    fn read(&mut self, max_bytes: NonZeroU32) -> EngineFuture<'_, StreamChunk>;
}

/// The session's transfer limits, applied at runtime.
///
/// Every field is optional and `None` means *no limit*, which is not a limit of zero: a
/// configuration with no number in it is unlimited, while a limit of zero would be a session
/// permitted to transfer nothing. Keeping the two apart in the type is what stops a settings screen
/// from disabling a transfer by clearing a field (`PG-E10-10`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineLimits {
    download_rate: Option<ByteRate>,
    upload_rate: Option<ByteRate>,
    connections: Option<u32>,
    active_downloads: Option<u32>,
}

impl EngineLimits {
    /// Limits of `None` leave the corresponding figure unlimited.
    #[must_use]
    pub const fn new(
        download_rate: Option<ByteRate>,
        upload_rate: Option<ByteRate>,
        connections: Option<u32>,
        active_downloads: Option<u32>,
    ) -> Self {
        Self {
            download_rate,
            upload_rate,
            connections,
            active_downloads,
        }
    }

    /// The limits a session with nothing configured runs under.
    #[must_use]
    pub const fn unlimited() -> Self {
        Self {
            download_rate: None,
            upload_rate: None,
            connections: None,
            active_downloads: None,
        }
    }

    /// The download ceiling, or `None` for unlimited.
    #[must_use]
    pub const fn download_rate(&self) -> Option<ByteRate> {
        self.download_rate
    }

    /// The upload ceiling, or `None` for unlimited.
    #[must_use]
    pub const fn upload_rate(&self) -> Option<ByteRate> {
        self.upload_rate
    }

    /// The peer connection ceiling, or `None` for unlimited.
    #[must_use]
    pub const fn connections(&self) -> Option<u32> {
        self.connections
    }

    /// How many torrents may transfer at once, or `None` for unlimited.
    #[must_use]
    pub const fn active_downloads(&self) -> Option<u32> {
        self.active_downloads
    }
}

/// What a metadata-only fetch answered: the torrent's identity and the file set to judge.
///
/// It is the input the safety gate needs and the shape `AcquisitionService::start` takes, so the
/// fetch and the engine speak one vocabulary rather than a caller translating between two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedMetadata {
    info_hash: InfoHash,
    description: TorrentDescription,
}

impl FetchedMetadata {
    /// Names the torrent and the files it says it will write.
    #[must_use]
    pub const fn new(info_hash: InfoHash, description: TorrentDescription) -> Self {
        Self {
            info_hash,
            description,
        }
    }

    /// The torrent's identity, which is also its dedupe key.
    #[must_use]
    pub const fn info_hash(&self) -> InfoHash {
        self.info_hash
    }

    /// The files the metadata describes.
    #[must_use]
    pub const fn description(&self) -> &TorrentDescription {
        &self.description
    }
}

/// The metadata-only fetch `FLOWS` F6 sequences before an add (`PG-E10-03`).
///
/// A second trait rather than a [`TorrentEngine`] method because it happens before a session holds
/// the torrent: it is the gate's input. The engine still owns digesting a magnet or a `.torrent`
/// into a file list — that is an adapter's work (AGENTS R8) — but no librqbit type crosses here.
pub trait TorrentMetadataFetch: Send + Sync {
    /// Resolves `source` far enough to describe its files, and writes nothing.
    ///
    /// # Errors
    ///
    /// [`EngineError::SessionClosed`] and [`EngineError::Failed`].
    fn fetch_metadata(&self, source: &TorrentSource) -> EngineFuture<'_, FetchedMetadata>;
}

/// One `BitTorrent` session: what the acquisition service and the stream server talk to.
///
/// The rules every method holds to are in the [module documentation](self); each method records
/// only what is particular to it.
pub trait TorrentEngine: Send + Sync {
    /// Which engine this is, and its version, for Diagnostics.
    ///
    /// Borrowed rather than returned by value, because it is fixed when the adapter is constructed
    /// and read on every Diagnostics refresh, so copying it would be an allocation for no gain.
    /// It answers on a closed session too: which engine was shut down is not a question about the
    /// session.
    fn descriptor(&self) -> &EngineDescriptor;

    /// Adds one allowed torrent and returns the handle the session holds it under.
    ///
    /// The port does not de-duplicate: adding one info-hash twice is two adds, and the rule that
    /// only one *live job* exists per hash is the partial unique index
    /// (`download_job_hash_uq`, `DATA_MODEL` I36) enforced where the job row is written, not an
    /// engine memory. The engine does check the hash against the source it was given, because it is
    /// the first thing that can.
    ///
    /// # Errors
    ///
    /// [`EngineError::SessionClosed`], [`EngineError::EmptySelection`],
    /// [`EngineError::UnknownFile`] when the selection names a file the torrent does not have,
    /// [`EngineError::IdentityMismatch`] when the source is not the torrent it was added as, and
    /// [`EngineError::Failed`].
    fn add(&self, request: &AddRequest) -> EngineFuture<'_, TorrentHandle>;

    /// Replaces the selected file set of a live torrent.
    ///
    /// **Replaces, rather than adds to**: a selection is the set of files being written, so a
    /// caller that wants one more file asks for the old set plus it. Deselecting a file stops
    /// writing it; what is already on disk is the library's business, and removing it is a
    /// library operation (`PG-E10-04`).
    ///
    /// # Errors
    ///
    /// [`EngineError::SessionClosed`], [`EngineError::UnknownTorrent`],
    /// [`EngineError::EmptySelection`], [`EngineError::UnknownFile`], and [`EngineError::Failed`].
    fn select(&self, handle: TorrentHandle, selection: &FileSelection) -> EngineFuture<'_, ()>;

    /// Opens a sequential reader over one selected file, from a byte offset.
    ///
    /// # Errors
    ///
    /// [`EngineError::SessionClosed`], [`EngineError::UnknownTorrent`],
    /// [`EngineError::UnknownFile`] when the torrent has no such file,
    /// [`EngineError::FileNotSelected`] when it has one that is not being downloaded —
    /// which is a different instruction to a user than "there is no such file" —
    /// [`EngineError::OffsetPastEnd`] when the offset is past the file's end, and
    /// [`EngineError::Failed`].
    fn stream(&self, request: &StreamRequest) -> EngineFuture<'_, Box<dyn TorrentStream>>;

    /// Reads the counters a progress row and the statistics stream draw (`PG-E10-07`).
    ///
    /// # Errors
    ///
    /// [`EngineError::SessionClosed`], [`EngineError::UnknownTorrent`], and
    /// [`EngineError::Failed`].
    fn stats(&self, handle: TorrentHandle) -> EngineFuture<'_, TorrentStats>;

    /// Reads the session's own state for one torrent, for a caller to persist.
    ///
    /// `download_job.engine_state_json` is where it goes (`FLOWS` F6), and the caller hands it back
    /// through [`AddIntent::Reattach`] after a restart. It is bytes rather than a value with named
    /// fields because what an engine must remember is the engine's business (AGENTS R8); a caller
    /// stores and returns it untouched.
    ///
    /// It may be empty: a torrent with nothing private to remember answers no bytes, which is a
    /// real answer and not a failure.
    ///
    /// # Errors
    ///
    /// [`EngineError::SessionClosed`], [`EngineError::UnknownTorrent`], and
    /// [`EngineError::Failed`].
    fn state(&self, handle: TorrentHandle) -> EngineFuture<'_, EngineState>;

    /// Re-checks a torrent's payload against the pieces it already has and transfers what fails.
    ///
    /// It is the on-demand half of what [`AddIntent::Reattach`] does at add time, for a torrent the
    /// session already holds: a caller that has reason to believe the bytes moved — a restore, a
    /// library rescan, a user asking to seed again — runs it and then reads [`TorrentEngine::stats`]
    /// for what was found. It never overwrites a byte that verifies and never discards verified
    /// data.
    ///
    /// **Idempotent**: running it twice in a row is `Ok(())` twice, and the second run has nothing
    /// to do.
    ///
    /// # Errors
    ///
    /// [`EngineError::SessionClosed`], [`EngineError::UnknownTorrent`], and
    /// [`EngineError::Failed`].
    fn verify(&self, handle: TorrentHandle) -> EngineFuture<'_, ()>;

    /// Reads the limits the session is running under.
    ///
    /// A getter as well as a setter, because "applied at runtime" is otherwise unobservable: the
    /// only way to tell an accepted limit from a swallowed one is to read it back, and reading it
    /// back on a live session is the whole of `PG-E10-10`'s acceptance.
    ///
    /// # Errors
    ///
    /// [`EngineError::SessionClosed`] and [`EngineError::Failed`].
    fn limits(&self) -> EngineFuture<'_, EngineLimits>;

    /// Applies transfer limits to the running session, without a restart.
    ///
    /// # Errors
    ///
    /// [`EngineError::SessionClosed`] and [`EngineError::Failed`].
    fn set_limits(&self, limits: &EngineLimits) -> EngineFuture<'_, ()>;

    /// Stops a torrent's transfer without forgetting the torrent.
    ///
    /// **Idempotent**: pausing a paused torrent is `Ok(())`, because the postcondition — this
    /// torrent is not transferring — is already satisfied, and a UI toggle that is dispatched
    /// twice must not raise an error at a user who asked for the state they are in. The job layer
    /// is where a repeated event is out of order (`peregrine_domain::download::transition`); the
    /// engine is a level, not the state machine.
    ///
    /// A paused torrent keeps everything it has already written, so [`TorrentEngine::resume`]
    /// continues rather than starting again. Pausing a *seeding* torrent is how a user stops
    /// seeding early (`PG-E10-09`), which is why this is not restricted to a transfer in progress.
    ///
    /// # Errors
    ///
    /// [`EngineError::SessionClosed`], [`EngineError::UnknownTorrent`], and
    /// [`EngineError::Failed`].
    fn pause(&self, handle: TorrentHandle) -> EngineFuture<'_, ()>;

    /// Restarts a stopped transfer from the bytes already on disk.
    ///
    /// **Idempotent**, for the reason [`TorrentEngine::pause`] gives, and it works for a torrent
    /// that was seeding when it was stopped. It resumes from what is on disk rather than from zero
    /// because the torrent's verified pieces are already there — a caller that needs the pieces
    /// *re-checked* first asks for [`TorrentEngine::verify`].
    ///
    /// # Errors
    ///
    /// [`EngineError::SessionClosed`], [`EngineError::UnknownTorrent`], and
    /// [`EngineError::Failed`].
    fn resume(&self, handle: TorrentHandle) -> EngineFuture<'_, ()>;

    /// Removes a torrent from the session.
    ///
    /// **Idempotent while the session is open**: a handle the session no longer holds is `Ok(())`,
    /// because the postcondition — this torrent is not in the session — already holds, and a
    /// double-clicked cancel or a `Drop` path that runs after a user action is not in error. Every
    /// *other* operation on a removed handle is [`EngineError::UnknownTorrent`]. A closed session
    /// still refuses, because the session-level rule is checked first.
    ///
    /// Removing a torrent is not deleting bytes: what is already on disk belongs to the library,
    /// and the seeding policy is what decides when a completed torrent stops (`PG-E10-09`).
    ///
    /// # Errors
    ///
    /// [`EngineError::SessionClosed`] and [`EngineError::Failed`].
    fn remove(&self, handle: TorrentHandle) -> EngineFuture<'_, ()>;

    /// Tears the session down: every transfer stops, resumable state is persisted, and the runtime
    /// is released.
    ///
    /// **Idempotent**, and the one operation a closed session answers with `Ok(())`: teardown is a
    /// level, not an edge, and a runtime that stops the engine from both a user action and a `Drop`
    /// path must be safe to ask twice. Every other operation on a closed session is
    /// [`EngineError::SessionClosed`], which is the refusal the conformance suite drives out of
    /// each of them.
    ///
    /// # Errors
    ///
    /// [`EngineError::Failed`] when the engine could not be stopped cleanly, which a caller treats
    /// as "the session may not reattach" rather than as a reason to keep running.
    fn shutdown(&self) -> EngineFuture<'_, ()>;
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::path::PathBuf;
    use std::sync::Arc;

    use peregrine_domain::inspect::{
        EntryKind, InspectionLimits, OptIn, TorrentDescription, TorrentEntry, TorrentInspector,
    };
    use peregrine_domain::path::PathGuard;
    use peregrine_domain::text::NonEmptyText;

    use super::*;

    /// The root every fixture path is judged against.
    pub(crate) fn root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\downloads")
        } else {
            PathBuf::from("/downloads")
        }
    }

    pub(crate) fn guard() -> PathGuard {
        PathGuard::new(root()).expect("the fixture root is absolute and canonical")
    }

    pub(crate) fn text(value: &str) -> NonEmptyText {
        NonEmptyText::new(value).expect("the fixture passes real text")
    }

    pub(crate) fn hash() -> InfoHash {
        InfoHash::from_hex("0123456789abcdef0123456789abcdef01234567").expect("a valid hash")
    }

    /// A report from the real inspector, so every one below is one production code could produce.
    pub(crate) fn report(path: &str, opt_in: OptIn) -> SafetyReport {
        let description =
            TorrentDescription::new(vec![TorrentEntry::new(path, 1, EntryKind::File)]);
        TorrentInspector::new(InspectionLimits::new(1_000, u64::MAX)).inspect(
            &description,
            &guard(),
            opt_in,
        )
    }

    fn allowed_request() -> AddRequest {
        AddRequest::new(
            hash(),
            TorrentSource::TorrentFile(vec![1, 2, 3]),
            guard()
                .resolve_relative("Movie.2024.1080p.mkv")
                .expect("a relative entry inside the root"),
            FileSelection::new([0]),
            true,
            AddIntent::Download,
            &report("Movie.2024.1080p.mkv", OptIn::NotGranted),
        )
        .expect("an allowed report builds a request")
    }

    /// A session whose only job is to prove the trait can be used behind `dyn`.
    struct Stub {
        descriptor: EngineDescriptor,
    }

    impl TorrentEngine for Stub {
        fn descriptor(&self) -> &EngineDescriptor {
            &self.descriptor
        }

        fn add(&self, _request: &AddRequest) -> EngineFuture<'_, TorrentHandle> {
            Box::pin(async { Ok(TorrentHandle::new(1)) })
        }

        fn select(
            &self,
            _handle: TorrentHandle,
            _selection: &FileSelection,
        ) -> EngineFuture<'_, ()> {
            Box::pin(async { Err(EngineError::SessionClosed) })
        }

        fn stream(&self, _request: &StreamRequest) -> EngineFuture<'_, Box<dyn TorrentStream>> {
            Box::pin(async {
                Err(EngineError::UnknownTorrent {
                    handle: TorrentHandle::new(1),
                })
            })
        }

        fn stats(&self, handle: TorrentHandle) -> EngineFuture<'_, TorrentStats> {
            Box::pin(async move { Err(EngineError::UnknownTorrent { handle }) })
        }

        fn state(&self, handle: TorrentHandle) -> EngineFuture<'_, EngineState> {
            Box::pin(async move { Err(EngineError::UnknownTorrent { handle }) })
        }

        fn verify(&self, handle: TorrentHandle) -> EngineFuture<'_, ()> {
            Box::pin(async move { Err(EngineError::UnknownTorrent { handle }) })
        }

        fn limits(&self) -> EngineFuture<'_, EngineLimits> {
            Box::pin(async { Ok(EngineLimits::unlimited()) })
        }

        fn set_limits(&self, _limits: &EngineLimits) -> EngineFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn pause(&self, handle: TorrentHandle) -> EngineFuture<'_, ()> {
            Box::pin(async move { Err(EngineError::UnknownTorrent { handle }) })
        }

        fn resume(&self, handle: TorrentHandle) -> EngineFuture<'_, ()> {
            Box::pin(async move { Err(EngineError::UnknownTorrent { handle }) })
        }

        fn remove(&self, _handle: TorrentHandle) -> EngineFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn shutdown(&self) -> EngineFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    fn stub() -> Stub {
        Stub {
            descriptor: EngineDescriptor::new(text("Stub"), text("0.0.0")),
        }
    }

    #[test]
    fn the_port_can_be_used_behind_a_trait_object() {
        // The runtime holds one engine chosen from settings, and ADR 0005 keeps a second adapter
        // open as a leaf change — which is why every operation returns a boxed future instead of
        // an `async fn`, a form that does not compile behind `dyn` on this toolchain.
        let engine: Arc<dyn TorrentEngine> = Arc::new(stub());

        assert_eq!(engine.descriptor().name().as_str(), "Stub");
        assert_eq!(engine.descriptor().version().as_str(), "0.0.0");
    }

    #[test]
    fn a_session_can_be_awaited_through_the_port() {
        // The future is `Send`, which is what lets an engine be driven from the scheduler's worker
        // pool rather than from the thread that happened to build it.
        fn assert_send<T: Send>(_: &T) {}

        let stub = stub();
        let future: Pin<Box<dyn Future<Output = _> + Send + '_>> = stub.limits();
        assert_send(&future);
    }

    #[test]
    fn a_blocked_report_cannot_become_an_add_request() {
        assert_eq!(
            AddRequest::new(
                hash(),
                TorrentSource::TorrentFile(vec![1]),
                guard()
                    .resolve_relative("Movie.mkv")
                    .expect("inside the root"),
                FileSelection::new([0]),
                false,
                AddIntent::Download,
                &report("../authorized_keys", OptIn::NotGranted),
            ),
            Err(AddRefused::Blocked)
        );
    }

    #[test]
    fn a_report_that_needs_the_advanced_opt_in_cannot_become_an_add_request() {
        // The strongest form of the claim: the report the *granted* opt-in produces is `Allowed`
        // and builds a request, while the one the first pass produced does not.
        let refused = report("extras.zip", OptIn::NotGranted);
        let granted = report("extras.zip", OptIn::Granted);
        assert_eq!(granted.verdict(), Verdict::Allowed);

        let build = |report: &SafetyReport| {
            AddRequest::new(
                hash(),
                TorrentSource::TorrentFile(vec![1]),
                guard()
                    .resolve_relative("extras.zip")
                    .expect("inside the root"),
                FileSelection::new([0]),
                false,
                AddIntent::Download,
                report,
            )
        };

        assert_eq!(build(&refused), Err(AddRefused::OptInRequired));
        assert!(build(&granted).is_ok());
    }

    #[test]
    fn a_reattach_is_gated_like_any_other_add() {
        // The recovery path is not a way around the gate: the reseed of a payload that is already
        // on disk carries its own report and is refused exactly as a first download would be.
        let reattach = AddIntent::Reattach {
            state: Some(EngineState::new(vec![1, 2, 3])),
        };
        assert!(reattach.verifies_existing_data());
        assert!(!AddIntent::Download.verifies_existing_data());

        let build = |report: &SafetyReport| {
            AddRequest::new(
                hash(),
                TorrentSource::TorrentFile(vec![1]),
                guard()
                    .resolve_relative("Movie.mkv")
                    .expect("inside the root"),
                FileSelection::new([0]),
                false,
                reattach.clone(),
                report,
            )
        };

        assert_eq!(
            build(&report("../escape.mkv", OptIn::NotGranted)),
            Err(AddRefused::Blocked)
        );
        let request = build(&report("Movie.mkv", OptIn::NotGranted))
            .expect("an allowed report builds a reattach request");
        assert!(request.intent().verifies_existing_data());
    }

    #[test]
    fn a_reattach_without_persisted_state_is_a_real_case() {
        // A finished job's state may be gone and its payload is still verifiable from the files and
        // the metadata, so `None` is an instruction and not an error.
        let intent = AddIntent::Reattach { state: None };

        assert!(intent.verifies_existing_data());
        assert_eq!(intent, AddIntent::Reattach { state: None });
    }

    #[test]
    fn persisted_state_prints_its_size_and_not_its_contents() {
        let state = EngineState::new(vec![0_u8; 1_024]);

        assert_eq!(state.len(), 1_024);
        assert_eq!(state.as_bytes().len(), 1_024);
        assert!(!state.is_empty());
        assert_eq!(format!("{state:?}"), "EngineState(1024 bytes)");
        assert!(EngineState::new(Vec::new()).is_empty());
    }

    #[test]
    fn an_allowed_report_builds_a_request_that_carries_what_was_asked_for() {
        let request = allowed_request();

        assert_eq!(request.info_hash(), hash());
        assert_eq!(request.selection().indices(), [0]);
        assert!(request.sequential());
        assert_eq!(
            request.destination().as_path(),
            root().join("Movie.2024.1080p.mkv")
        );
        assert!(matches!(request.source(), TorrentSource::TorrentFile(_)));
    }

    #[test]
    fn an_add_request_prints_neither_its_magnet_nor_its_destination() {
        // The diagnostics tail is exported, and a magnet carries the info-hash (SECURITY T13)
        // while the destination names a directory under the user's home.
        let magnet = format!(
            "magnet:?xt=urn:btih:{}",
            "0123456789abcdef0123456789abcdef01234567"
        );
        let request = AddRequest::new(
            hash(),
            TorrentSource::Magnet(text(&magnet)),
            guard()
                .resolve_relative("Show/Season 01/S01E01.mkv")
                .expect("inside the root"),
            FileSelection::new([0, 1]),
            false,
            AddIntent::Download,
            &report("S01E01.mkv", OptIn::NotGranted),
        )
        .expect("an allowed report builds a request");

        let debug = format!("{request:?}");

        assert!(!debug.contains("magnet:"), "{debug}");
        assert!(!debug.contains(&hash().to_hex()), "{debug}");
        assert!(!debug.contains("S01E01.mkv"), "{debug}");
        assert!(debug.contains("Magnet(..)"), "{debug}");
        assert!(debug.contains("files: 2"), "{debug}");
        assert!(debug.contains("Download"), "{debug}");
    }

    #[test]
    fn an_add_request_that_carries_state_prints_only_its_size() {
        let request = AddRequest::new(
            hash(),
            TorrentSource::TorrentFile(vec![1]),
            guard()
                .resolve_relative("Movie.mkv")
                .expect("inside the root"),
            FileSelection::new([0]),
            false,
            AddIntent::Reattach {
                state: Some(EngineState::new(
                    b"tracker=http://example.invalid/announce".to_vec(),
                )),
            },
            &report("Movie.mkv", OptIn::NotGranted),
        )
        .expect("an allowed report builds a request");

        let debug = format!("{request:?}");

        assert!(debug.contains("EngineState(39 bytes)"), "{debug}");
        assert!(!debug.contains("tracker"), "{debug}");
    }

    #[test]
    fn a_torrent_file_source_prints_only_its_size() {
        let source = TorrentSource::TorrentFile(vec![0; 42]);

        assert_eq!(format!("{source:?}"), "TorrentFile(42 bytes)");
    }

    #[test]
    fn a_selection_is_a_canonical_set() {
        let selection = FileSelection::new([3, 1, 3, 0, 1]);

        assert_eq!(selection.indices(), [0, 1, 3]);
        assert_eq!(selection.len(), 3);
        assert!(selection.contains(3));
        assert!(!selection.contains(2));
        assert_eq!(selection, FileSelection::new([0, 1, 3]));
        assert!(!selection.is_empty());
    }

    #[test]
    fn an_empty_selection_is_representable_and_knows_it() {
        // Representable because the conformance suite drives it, and refused by `add` and `select`
        // because a torrent with nothing selected writes nothing.
        let selection = FileSelection::empty();

        assert!(selection.is_empty());
        assert_eq!(selection.len(), 0);
        assert!(selection.indices().is_empty());
        assert!(!selection.contains(0));
        assert_eq!(selection, FileSelection::new(Vec::new()));
    }

    #[test]
    fn a_stream_request_carries_its_file_offset_and_window() {
        let handle = TorrentHandle::new(4);
        let request = StreamRequest::new(handle, 2, Bytes::new(1_000), Bytes::new(8 * 1024 * 1024));

        assert_eq!(request.handle(), handle);
        assert_eq!(request.file(), 2);
        assert_eq!(request.from(), Bytes::new(1_000));
        assert_eq!(request.window(), Bytes::new(8 * 1024 * 1024));
    }

    #[test]
    fn a_stream_chunk_tells_data_from_the_end() {
        assert_ne!(StreamChunk::Data(vec![1]), StreamChunk::End);
        assert_eq!(
            StreamChunk::Data(vec![1]).clone(),
            StreamChunk::Data(vec![1])
        );
        let _ = NonZeroU32::new(1).expect("one is not zero");
    }

    #[test]
    fn limits_distinguish_unlimited_from_zero() {
        // The two are different requests: `None` is a session with no ceiling, and a rate of zero
        // is a session permitted to transfer nothing. A settings screen clears a field to mean the
        // first.
        let unlimited = EngineLimits::unlimited();
        assert!(unlimited.download_rate().is_none());
        assert!(unlimited.upload_rate().is_none());
        assert!(unlimited.connections().is_none());
        assert!(unlimited.active_downloads().is_none());

        let stopped = EngineLimits::new(
            Some(ByteRate::per_second(0)),
            Some(ByteRate::per_second(0)),
            Some(0),
            Some(0),
        );
        assert_eq!(stopped.download_rate(), Some(ByteRate::per_second(0)));
        assert_ne!(stopped, unlimited);
        assert_eq!(
            EngineLimits::new(Some(ByteRate::per_second(1_000)), None, Some(200), Some(3))
                .active_downloads(),
            Some(3)
        );
    }

    #[test]
    fn a_handle_is_not_a_hash_and_prints_only_its_number() {
        let handle = TorrentHandle::new(12);

        assert_eq!(handle.get(), 12);
        assert_eq!(format!("{handle:?}"), "TorrentHandle(12)");
        assert!(!format!("{handle:?}").contains(&hash().to_hex()));
    }
}
