//! A scripted torrent engine.

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::{Mutex, MutexGuard, PoisonError};

use peregrine_domain::id::InfoHash;
use peregrine_domain::text::NonEmptyText;
use peregrine_torrent::{
    AddIntent, AddRequest, ByteRate, Bytes, EngineDescriptor, EngineError, EngineFuture,
    EngineLimits, EngineState, FileSelection, PeerCounts, StreamChunk, StreamRequest,
    TorrentEngine, TorrentHandle, TorrentHealth, TorrentStats, TorrentStream, Transfer,
};

/// One operation a [`FakeTorrentEngine`] can be told to fail.
///
/// It exists so a test can produce any [`EngineError`] the port names without a fixture per error:
/// the fake's own rules answer everything else, and a scripted refusal is what a test reaches for
/// when it is testing how a *caller* handles a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EngineOperation {
    /// [`TorrentEngine::add`].
    Add,
    /// [`TorrentEngine::select`].
    Select,
    /// [`TorrentEngine::stream`].
    Stream,
    /// [`TorrentEngine::stats`].
    Stats,
    /// [`TorrentEngine::state`].
    State,
    /// [`TorrentEngine::verify`].
    Verify,
    /// [`TorrentEngine::limits`].
    Limits,
    /// [`TorrentEngine::set_limits`].
    SetLimits,
    /// [`TorrentEngine::pause`].
    Pause,
    /// [`TorrentEngine::resume`].
    Resume,
    /// [`TorrentEngine::remove`].
    Remove,
    /// [`TorrentEngine::shutdown`].
    Shutdown,
}

/// One call a [`FakeTorrentEngine`] received, in the order it arrived.
///
/// The engine's own state is what the conformance suite asserts against; this is what a test that
/// is checking a *caller* reads — that the scheduler paused the job it removed, that a reattach was
/// sent with its stored state, that limits were applied once rather than on every tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineCall {
    /// An add, with everything about the request a caller can observe: the identity, the files it
    /// selected, whether it asked for pieces in file order, and whether it said the payload may
    /// already be on disk.
    Add {
        /// The identity the caller added the torrent as.
        info_hash: InfoHash,
        /// The files it asked for.
        selection: FileSelection,
        /// Whether it asked for a sequential transfer.
        sequential: bool,
        /// Whether it said the payload may already be on disk.
        intent: AddIntent,
    },
    /// A selection change.
    Select(TorrentHandle, FileSelection),
    /// A stream opened over a file, from an offset.
    Stream(TorrentHandle, usize, Bytes),
    /// A statistics read.
    Stats(TorrentHandle),
    /// A state read.
    State(TorrentHandle),
    /// A piece re-check.
    Verify(TorrentHandle),
    /// A limits read.
    Limits,
    /// A limits write.
    SetLimits(EngineLimits),
    /// A pause.
    Pause(TorrentHandle),
    /// A resume.
    Resume(TorrentHandle),
    /// A removal.
    Remove(TorrentHandle),
    /// A shutdown.
    Shutdown,
}

/// One torrent the fake's session holds.
#[derive(Debug)]
struct FakeTorrent {
    info_hash: InfoHash,
    selection: FileSelection,
    file_count: usize,
    file_len: Bytes,
    /// Payload bytes this torrent reports as moved, which is what a completion test drives.
    downloaded: Bytes,
    /// The state this torrent hands a caller to persist. Deterministic, like every other answer
    /// here, and derived from the handle so a round trip through it is observable.
    state: EngineState,
}

impl FakeTorrent {
    /// The counters this torrent reports: an idle, peerless transfer of the configured size.
    ///
    /// Deterministic rather than scripted, because a fake whose statistics came from a test's
    /// private state would make the conformance suite assert the test rather than the port. A test
    /// that needs a moving transfer is a test about a progress row, and the builder for it belongs
    /// beside that test.
    fn stats(&self) -> TorrentStats {
        TorrentStats::new(
            self.info_hash,
            Transfer::new(
                self.downloaded,
                Bytes::new(0),
                ByteRate::per_second(0),
                ByteRate::per_second(0),
            ),
            Bytes::new(
                self.file_len
                    .get()
                    .saturating_mul(u64::try_from(self.file_count).unwrap_or(u64::MAX)),
            ),
            PeerCounts::new(0, 0),
            None,
            TorrentHealth::NoPeers,
        )
    }
}

/// Everything a session holds and everything a test has told it to answer.
#[derive(Debug)]
struct Session {
    closed: bool,
    /// The next handle to mint. A session mints 1, 2, 3 … in order, so a test that needs to know a
    /// handle can count adds rather than read the engine's internals.
    next_handle: u64,
    torrents: BTreeMap<TorrentHandle, FakeTorrent>,
    limits: EngineLimits,
    calls: Vec<EngineCall>,
    refusals: BTreeMap<EngineOperation, EngineError>,
}

/// A torrent engine that answers exactly what a test told it to.
///
/// It is stateful, because the port is: an add is remembered, a selection change replaces a set, a
/// removal makes a handle name nothing, and a shutdown refuses everything afterwards. A fake that
/// answered each call in isolation could not stand in for the adapter, and the conformance suite —
/// which walks an engine through a sequence — would not be able to run against it.
///
/// The rules it holds to are the port's, not the adapter's: a closed session refuses before any
/// other rule, a scripted refusal is answered before a method's own behaviour, and everything else
/// is the documented contract. Nothing here reads a clock, a random source, or a network, so a
/// test that injects it is deterministic (AGENTS R14).
#[derive(Debug)]
pub struct FakeTorrentEngine {
    descriptor: EngineDescriptor,
    file_count: usize,
    file_len: Bytes,
    /// Payload bytes every torrent this session adds reports as moved.
    downloaded: Bytes,
    session: Mutex<Session>,
}

impl FakeTorrentEngine {
    /// A session that accepts adds, over a single zero-byte file.
    ///
    /// The degenerate file is the honest default for a fixture whose caller is testing something
    /// other than streaming; [`FakeTorrentEngine::with_files`] is what a streaming test builds.
    #[must_use]
    pub fn new() -> Self {
        Self {
            descriptor: EngineDescriptor::new(
                NonEmptyText::new("FakeTorrentEngine").expect("the fixture's name is real text"),
                NonEmptyText::new("0.0.0").expect("the fixture's version is real text"),
            ),
            file_count: 1,
            file_len: Bytes::new(0),
            downloaded: Bytes::new(0),
            session: Mutex::new(Session {
                closed: false,
                next_handle: 0,
                torrents: BTreeMap::new(),
                limits: EngineLimits::unlimited(),
                calls: Vec::new(),
                refusals: BTreeMap::new(),
            }),
        }
    }

    /// The same session, reporting `file_count` files of `file_len` bytes each.
    ///
    /// Every torrent it adds gets the same shape, which is what a fixture needs: the conformance
    /// suite is told the same numbers through its probe, and a difference between the two is what
    /// the suite's length and out-of-range assertions catch.
    #[must_use]
    pub fn with_files(mut self, file_count: usize, file_len: Bytes) -> Self {
        self.file_count = file_count;
        self.file_len = file_len;
        self
    }

    /// The same session, reporting `downloaded` payload bytes for every torrent it adds.
    ///
    /// A complete payload spells `downloaded == file_count * file_len`; a partial one is how a
    /// progress row is driven without a moving transfer.
    #[must_use]
    pub fn with_downloaded(mut self, downloaded: Bytes) -> Self {
        self.downloaded = downloaded;
        self
    }

    /// The same session, answering `operation` with `error` instead of its own behaviour.
    ///
    /// The refusal is checked *after* the session-level rules and *before* the operation's own: a
    /// closed session refuses whatever was scripted, and a scripted refusal answers a call that
    /// would otherwise have been refused for a different reason. That order is what lets a test
    /// produce any variant of [`EngineError`] from any operation.
    #[must_use]
    pub fn failing_with(self, operation: EngineOperation, error: EngineError) -> Self {
        self.session().refusals.insert(operation, error);
        self
    }

    /// The calls this session has received, oldest first.
    #[must_use]
    pub fn calls(&self) -> Vec<EngineCall> {
        self.session().calls.clone()
    }

    /// The session state, recovering from a poisoned lock.
    ///
    /// A poisoned lock only means a thread panicked while holding it — which in a test crate is
    /// another test's failure — and turning that into a second, unrelated panic would hide the
    /// first.
    fn session(&self) -> MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Notes one call, whether or not the operation goes on to succeed.
    fn record(&self, call: EngineCall) {
        self.session().calls.push(call);
    }

    /// Answers one operation under the session-level rules every method shares.
    fn with_session<T>(
        &self,
        operation: EngineOperation,
        answer: impl FnOnce(&mut Session) -> Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        let mut session = self.session();
        if session.closed {
            return Err(EngineError::SessionClosed);
        }
        if let Some(error) = session.refusals.get(&operation).copied() {
            return Err(error);
        }
        answer(&mut session)
    }
}

impl Default for FakeTorrentEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TorrentEngine for FakeTorrentEngine {
    fn descriptor(&self) -> &EngineDescriptor {
        &self.descriptor
    }

    fn add(&self, request: &AddRequest) -> EngineFuture<'_, TorrentHandle> {
        self.record(EngineCall::Add {
            info_hash: request.info_hash(),
            selection: request.selection().clone(),
            sequential: request.sequential(),
            intent: request.intent().clone(),
        });
        let file_count = self.file_count;
        let file_len = self.file_len;
        let downloaded = self.downloaded;
        let answer = self.with_session(EngineOperation::Add, move |session| {
            if request.selection().is_empty() {
                return Err(EngineError::EmptySelection);
            }
            if let Some(index) = request
                .selection()
                .indices()
                .iter()
                .copied()
                .find(|index| *index >= file_count)
            {
                return Err(EngineError::UnknownFile { index });
            }
            session.next_handle += 1;
            let handle = TorrentHandle::new(session.next_handle);
            session.torrents.insert(
                handle,
                FakeTorrent {
                    info_hash: request.info_hash(),
                    selection: request.selection().clone(),
                    file_count,
                    file_len,
                    downloaded,
                    state: state_of(handle),
                },
            );
            Ok(handle)
        });
        Box::pin(async move { answer })
    }

    fn select(&self, handle: TorrentHandle, selection: &FileSelection) -> EngineFuture<'_, ()> {
        self.record(EngineCall::Select(handle, selection.clone()));
        let answer = self.with_session(EngineOperation::Select, move |session| {
            let torrent = session
                .torrents
                .get_mut(&handle)
                .ok_or(EngineError::UnknownTorrent { handle })?;
            if selection.is_empty() {
                return Err(EngineError::EmptySelection);
            }
            if let Some(index) = selection
                .indices()
                .iter()
                .copied()
                .find(|index| *index >= torrent.file_count)
            {
                return Err(EngineError::UnknownFile { index });
            }
            // A selection replaces the set rather than adding to it, which is what the port
            // documents and what a caller comparing a live selection with a requested one relies on.
            torrent.selection = selection.clone();
            Ok(())
        });
        Box::pin(async move { answer })
    }

    fn stream(&self, request: &StreamRequest) -> EngineFuture<'_, Box<dyn TorrentStream>> {
        self.record(EngineCall::Stream(
            request.handle(),
            request.file(),
            request.from(),
        ));
        let answer = self.with_session(EngineOperation::Stream, move |session| {
            let torrent =
                session
                    .torrents
                    .get(&request.handle())
                    .ok_or(EngineError::UnknownTorrent {
                        handle: request.handle(),
                    })?;
            if request.file() >= torrent.file_count {
                return Err(EngineError::UnknownFile {
                    index: request.file(),
                });
            }
            if !torrent.selection.contains(request.file()) {
                return Err(EngineError::FileNotSelected {
                    index: request.file(),
                });
            }
            if request.from().get() > torrent.file_len.get() {
                return Err(EngineError::OffsetPastEnd {
                    offset: request.from(),
                    len: torrent.file_len,
                });
            }
            Ok(Box::new(FakeTorrentStream {
                len: torrent.file_len,
                position: request.from(),
            }) as Box<dyn TorrentStream>)
        });
        Box::pin(async move { answer })
    }

    fn stats(&self, handle: TorrentHandle) -> EngineFuture<'_, TorrentStats> {
        self.record(EngineCall::Stats(handle));
        let answer = self.with_session(EngineOperation::Stats, move |session| {
            let torrent = session
                .torrents
                .get(&handle)
                .ok_or(EngineError::UnknownTorrent { handle })?;
            Ok(torrent.stats())
        });
        Box::pin(async move { answer })
    }

    fn state(&self, handle: TorrentHandle) -> EngineFuture<'_, EngineState> {
        self.record(EngineCall::State(handle));
        let answer = self.with_session(EngineOperation::State, move |session| {
            let torrent = session
                .torrents
                .get(&handle)
                .ok_or(EngineError::UnknownTorrent { handle })?;
            Ok(torrent.state.clone())
        });
        Box::pin(async move { answer })
    }

    fn verify(&self, handle: TorrentHandle) -> EngineFuture<'_, ()> {
        self.record(EngineCall::Verify(handle));
        let answer = self.with_session(EngineOperation::Verify, move |session| {
            if !session.torrents.contains_key(&handle) {
                return Err(EngineError::UnknownTorrent { handle });
            }
            // Idempotent: a fake holds no bytes to re-check, so a second run has nothing to do and
            // answers the same way the first did.
            Ok(())
        });
        Box::pin(async move { answer })
    }

    fn limits(&self) -> EngineFuture<'_, EngineLimits> {
        self.record(EngineCall::Limits);
        let answer = self.with_session(EngineOperation::Limits, |session| Ok(session.limits));
        Box::pin(async move { answer })
    }

    fn set_limits(&self, limits: &EngineLimits) -> EngineFuture<'_, ()> {
        self.record(EngineCall::SetLimits(*limits));
        let limits = *limits;
        let answer = self.with_session(EngineOperation::SetLimits, move |session| {
            session.limits = limits;
            Ok(())
        });
        Box::pin(async move { answer })
    }

    fn pause(&self, handle: TorrentHandle) -> EngineFuture<'_, ()> {
        self.record(EngineCall::Pause(handle));
        let answer = self.with_session(EngineOperation::Pause, move |session| {
            // Idempotent: a paused torrent is already in the state a second pause asks for, so the
            // only thing left to answer is whether the handle names a torrent at all.
            if !session.torrents.contains_key(&handle) {
                return Err(EngineError::UnknownTorrent { handle });
            }
            Ok(())
        });
        Box::pin(async move { answer })
    }

    fn resume(&self, handle: TorrentHandle) -> EngineFuture<'_, ()> {
        self.record(EngineCall::Resume(handle));
        let answer = self.with_session(EngineOperation::Resume, move |session| {
            // Idempotent, for the reason `pause` gives.
            if !session.torrents.contains_key(&handle) {
                return Err(EngineError::UnknownTorrent { handle });
            }
            Ok(())
        });
        Box::pin(async move { answer })
    }

    fn remove(&self, handle: TorrentHandle) -> EngineFuture<'_, ()> {
        self.record(EngineCall::Remove(handle));
        let answer = self.with_session(EngineOperation::Remove, move |session| {
            // Idempotent: the postcondition is "this torrent is not in the session", which removing
            // it once already satisfies.
            session.torrents.remove(&handle);
            Ok(())
        });
        Box::pin(async move { answer })
    }

    fn shutdown(&self) -> EngineFuture<'_, ()> {
        self.record(EngineCall::Shutdown);
        let mut session = self.session();
        let refusal = session.refusals.get(&EngineOperation::Shutdown).copied();
        let answer = if let Some(error) = refusal {
            Err(error)
        } else {
            // Idempotent, and the one operation a closed session answers: teardown is a level, and
            // a runtime that stops the engine from two paths must be safe to ask twice.
            session.closed = true;
            Ok(())
        };
        Box::pin(async move { answer })
    }
}

/// A deterministic reader over a fake torrent's file: zeros, up to the length the fixture declared.
#[derive(Debug)]
struct FakeTorrentStream {
    len: Bytes,
    position: Bytes,
}

impl TorrentStream for FakeTorrentStream {
    fn len(&self) -> Bytes {
        self.len
    }

    fn position(&self) -> Bytes {
        self.position
    }

    fn read(&mut self, max_bytes: NonZeroU32) -> EngineFuture<'_, StreamChunk> {
        let remaining = self.len.get().saturating_sub(self.position.get());
        let answer = if remaining == 0 {
            StreamChunk::End
        } else {
            // `max_bytes` is a `u32`, so a chunk never needs more room than a `usize` has on any
            // platform this runs on.
            let take = remaining.min(u64::from(max_bytes.get())) as usize;
            self.position = Bytes::new(self.position.get() + take as u64);
            StreamChunk::Data(vec![0_u8; take])
        };
        Box::pin(async move { Ok(answer) })
    }
}

/// The state a fake reports for one torrent.
///
/// Derived from the handle rather than random, so a test that round-trips it through a reattach can
/// say what it saw, and so nothing here needs a random source.
fn state_of(handle: TorrentHandle) -> EngineState {
    EngineState::new(format!("fake-state:{}", handle.get()).into_bytes())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    use futures::executor::block_on;
    use peregrine_domain::inspect::{
        EntryKind, InspectionLimits, OptIn, SafetyReport, TorrentDescription, TorrentEntry,
        TorrentInspector,
    };
    use peregrine_domain::path::{PathGuard, SafePath};

    use super::*;

    /// How many files the fixture torrent has: enough for the conformance suite's two file probes.
    pub(crate) const FILE_COUNT: usize = 4;

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

    pub(crate) fn hash() -> InfoHash {
        InfoHash::from_hex("0123456789abcdef0123456789abcdef01234567").expect("a valid hash")
    }

    /// A report from the real inspector, so every fixture here is one production code could produce.
    pub(crate) fn allowed_report() -> SafetyReport {
        let description = TorrentDescription::new(vec![TorrentEntry::new(
            "Movie.2024.1080p.mkv",
            4_096,
            EntryKind::File,
        )]);
        TorrentInspector::new(InspectionLimits::new(1_000, u64::MAX)).inspect(
            &description,
            &guard(),
            OptIn::NotGranted,
        )
    }

    pub(crate) fn destination() -> SafePath {
        guard()
            .resolve_relative("Movie.2024.1080p.mkv")
            .expect("a relative entry inside the root")
    }

    pub(crate) fn file_len() -> Bytes {
        Bytes::new(4_096)
    }

    /// The limits the conformance suite applies and reads back.
    ///
    /// Deliberately not [`EngineLimits::unlimited`], which the suite refuses as a probe: a limit
    /// that was never set cannot tell an applied one from a swallowed one.
    pub(crate) fn limits() -> EngineLimits {
        EngineLimits::new(
            Some(ByteRate::per_second(1_000_000)),
            Some(ByteRate::per_second(500_000)),
            Some(200),
            Some(3),
        )
    }

    /// An add request for the fixture torrent, with the intent a caller would send.
    pub(crate) fn request(intent: AddIntent) -> AddRequest {
        AddRequest::new(
            hash(),
            peregrine_torrent::TorrentSource::TorrentFile(vec![1, 2, 3]),
            destination(),
            FileSelection::new([0, 1]),
            true,
            intent,
            &allowed_report(),
        )
        .expect("the fixture report allows the torrent")
    }

    fn engine() -> FakeTorrentEngine {
        FakeTorrentEngine::new().with_files(FILE_COUNT, file_len())
    }

    fn add(engine: &FakeTorrentEngine, request: &AddRequest) -> TorrentHandle {
        block_on(engine.add(request)).expect("the fake accepts an allowed add")
    }

    #[test]
    fn the_fake_answers_the_same_way_every_time() {
        // Two identical sessions doing the same things answer identically: no clock, no random
        // source, no map iteration order reaches an answer.
        let first = engine();
        let second = engine();
        let handle = add(&first, &request(AddIntent::Download));
        let other = add(&second, &request(AddIntent::Download));

        assert_eq!(handle, other);
        assert_eq!(
            block_on(first.stats(handle)),
            block_on(second.stats(other)),
            "a fixture whose answers differed between runs would make a suite flaky"
        );
        assert_eq!(first.calls(), second.calls());
    }

    #[test]
    fn the_fake_refuses_an_add_with_nothing_selected() {
        let engine = engine();
        let empty = AddRequest::new(
            hash(),
            peregrine_torrent::TorrentSource::TorrentFile(vec![1, 2, 3]),
            destination(),
            FileSelection::empty(),
            false,
            AddIntent::Download,
            &allowed_report(),
        )
        .expect("the report allows the torrent; the selection is what the engine judges");

        assert_eq!(
            block_on(engine.add(&empty)),
            Err(EngineError::EmptySelection)
        );
        assert_eq!(
            engine.calls(),
            [EngineCall::Add {
                info_hash: hash(),
                selection: FileSelection::empty(),
                sequential: false,
                intent: AddIntent::Download,
            }],
            "a refused call is still a call the fake received"
        );
    }

    /// The add call the fixture request records.
    fn recorded_add(intent: AddIntent) -> EngineCall {
        EngineCall::Add {
            info_hash: hash(),
            selection: FileSelection::new([0, 1]),
            sequential: true,
            intent,
        }
    }

    #[test]
    fn the_fake_remembers_what_it_was_asked_to_do() {
        let engine = engine();
        let handle = add(&engine, &request(AddIntent::Download));

        block_on(engine.pause(handle)).expect("a live torrent pauses");
        block_on(engine.resume(handle)).expect("a paused torrent resumes");
        block_on(engine.verify(handle)).expect("a live torrent re-checks");
        let _ = block_on(engine.state(handle)).expect("a live torrent reports state");
        block_on(engine.set_limits(&limits())).expect("a live session applies limits");
        block_on(engine.remove(handle)).expect("a live torrent is removed");

        let calls = engine.calls();
        assert_eq!(calls[0], recorded_add(AddIntent::Download));
        assert_eq!(calls[1], EngineCall::Pause(handle));
        assert_eq!(calls[2], EngineCall::Resume(handle));
        assert_eq!(calls[3], EngineCall::Verify(handle));
        assert_eq!(calls[4], EngineCall::State(handle));
        assert_eq!(calls[5], EngineCall::SetLimits(limits()));
        assert_eq!(calls[6], EngineCall::Remove(handle));
        assert_eq!(calls.len(), 7);
    }

    #[test]
    fn the_fake_holds_two_torrents_for_one_info_hash() {
        // The port does not de-duplicate: one live *job* per hash is the partial unique index
        // (`DATA_MODEL` I36), which is the acquisition service's rule and not an engine memory.
        let engine = engine();
        let first = add(&engine, &request(AddIntent::Download));
        let second = add(&engine, &request(AddIntent::Download));

        assert_ne!(first, second);
        assert_eq!(
            block_on(engine.stats(first)).map(|stats| stats.info_hash()),
            Ok(hash())
        );
        assert_eq!(
            block_on(engine.stats(second)).map(|stats| stats.info_hash()),
            Ok(hash())
        );
    }

    #[test]
    fn the_fake_reads_a_file_as_deterministic_zeros_up_to_its_length() {
        let engine = engine();
        let handle = add(&engine, &request(AddIntent::Download));
        let max = NonZeroU32::new(1_024).expect("one thousand and twenty-four is not zero");

        let mut reader =
            block_on(engine.stream(&StreamRequest::new(handle, 0, Bytes::new(0), Bytes::new(0))))
                .expect("a selected file opens a reader");

        assert_eq!(reader.len(), file_len());
        assert_eq!(reader.position(), Bytes::new(0));
        for _ in 0..4 {
            match block_on(reader.read(max)).expect("the fake answers a read") {
                StreamChunk::Data(bytes) => assert_eq!(bytes, vec![0_u8; 1_024]),
                StreamChunk::End => panic!("the file had not ended yet"),
            }
        }
        assert_eq!(reader.position(), file_len());
        assert_eq!(
            block_on(reader.read(max)).expect("the fake answers a read"),
            StreamChunk::End,
            "the fifth read is the end of a 4096-byte file read 1024 bytes at a time"
        );
    }

    #[test]
    fn the_fake_answers_a_scripted_refusal_before_its_own_rules() {
        // The script exists to produce variants the fake's own behaviour would not, so it has to
        // win over an unknown handle — while a closed session still wins over the script.
        let engine = engine().failing_with(EngineOperation::Pause, EngineError::Failed);
        let unknown = TorrentHandle::new(u64::MAX);

        assert_eq!(block_on(engine.pause(unknown)), Err(EngineError::Failed));
        block_on(engine.shutdown()).expect("the session shuts down");
        assert_eq!(
            block_on(engine.pause(unknown)),
            Err(EngineError::SessionClosed)
        );
    }

    #[test]
    fn the_fake_refuses_everything_but_shutdown_once_it_is_closed() {
        let engine = engine();
        let handle = add(&engine, &request(AddIntent::Download));

        block_on(engine.shutdown()).expect("a live session shuts down");
        block_on(engine.shutdown()).expect("shutting down twice is idempotent");

        assert_eq!(
            block_on(engine.stats(handle)),
            Err(EngineError::SessionClosed)
        );
        assert_eq!(
            block_on(engine.remove(handle)),
            Err(EngineError::SessionClosed),
            "a closed session refuses before removal's own idempotence"
        );
        assert_eq!(block_on(engine.limits()), Err(EngineError::SessionClosed));
    }
}
