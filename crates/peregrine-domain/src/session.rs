//! The player session state machine: the seven states FLOWS SM3 draws and the events a
//! playback service may apply to one.
//!
//! [`Session`] owns the three things a playback carries between events: its state, the last
//! position it reported, and — once it has failed — what ARCHITECTURE §15 shows the user. The
//! states are SM3's, in the order the diagram draws them: [`SessionState`] carries the spelling
//! and each event's doc names the edge it is.
//!
//! Four rules are enforced here rather than left to a service, because each is cheap to get
//! wrong and expensive to notice:
//!
//! - **`playing` is entered by a rendered frame.** Every coalesced progress write and the
//!   watched decision key off it, so a session that claimed to be playing before the first
//!   frame would persist progress for something the user never saw, and F7's time-to-first-frame
//!   metric — the one number that means anything on a torrent stream — would measure nothing.
//!   [`SessionEvent::FrameRendered`] is the only event that reaches `playing` from a state that
//!   has not played yet; a cell test and a path property both pin that.
//! - **The position survives the session.** Every position report moves it and the failure
//!   transition keeps it, because a retry resumes from the last known position rather than from
//!   zero (ARCHITECTURE §15). Starting a crashed playback over at the beginning is a bug the
//!   user sees within seconds.
//! - **A failure carries what the UI shows**: the exit code mpv left — when it left one — and
//!   the tail of its stderr, bounded by [`MAX_STDERR_TAIL_BYTES`].
//! - **Terminal states absorb.** `ended` and `failed` refuse every event, including a duplicate
//!   of the one that ended them; a retry is a new session ([`Session::retry`]), because the mpv
//!   process the failed session owned is already gone.
//!
//! A stop is legal from every live state — F7's "user quits or file ends" — which is also how a
//! session parked in `preparing` or `stalled` gets out: a torrent with no peers must not leave a
//! player that cannot be closed. Each state matches the event set exhaustively, with no
//! catch-all arm, so an event added to [`SessionEvent`] has to have its cell decided in every
//! state instead of defaulting to "illegal".
//!
//! The row that has no home in this machine is a mid-playback starvation. F7 draws it as a
//! buffer-health indicator over a live session, so the machine stays in `playing` and keeps
//! writing progress; only the *load* has a [`SessionState::Stalled`], which is where a stream
//! that never produced a frame can be given up on.

use std::fmt;

use thiserror::Error;

use crate::library::PlaybackPosition;

/// How much of mpv's stderr a failure carries.
///
/// An mpv log is unbounded — a long session with debug output is megabytes — while the failure
/// travels to the UI on `player://state`, which the IPC budget holds under 64 KB
/// (ARCHITECTURE §6). The last four kilobytes are several dozen lines: the error mpv printed and
/// the line before it. The *tail* is the half worth keeping, because a crash explains itself at
/// the end and the beginning of a log is startup noise.
pub const MAX_STDERR_TAIL_BYTES: usize = 4 * 1024;

/// The end of a process's stderr, bounded by [`MAX_STDERR_TAIL_BYTES`].
///
/// It cannot be built unbounded: [`StderrTail::from_log`] is the only constructor, so a caller
/// that pipes a whole mpv log through a failure gets the cap whether it remembered one or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StderrTail {
    text: String,
    truncated: bool,
}

impl StderrTail {
    /// Keeps the end of a log, cut at a character boundary.
    ///
    /// A log longer than the cap loses its beginning rather than its end, and `truncated`
    /// records that, so the UI can say the log was cut instead of presenting a fragment as the
    /// whole error.
    #[must_use]
    pub fn from_log(log: &str) -> Self {
        if log.len() <= MAX_STDERR_TAIL_BYTES {
            return Self {
                text: log.to_owned(),
                truncated: false,
            };
        }
        let start = log.len() - MAX_STDERR_TAIL_BYTES;
        // A byte offset can land inside a multi-byte character — mpv logs file names in UTF-8 —
        // and slicing there would panic. Walking forward to the next boundary keeps the text a
        // valid `str` at the cost of a few bytes.
        let start = (start..log.len())
            .find(|at| log.is_char_boundary(*at))
            .unwrap_or(log.len());
        Self {
            text: log[start..].to_owned(),
            truncated: true,
        }
    }

    /// The kept text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the log was longer than the cap and lost its beginning.
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.truncated
    }
}

/// What the UI shows for a session that reached [`SessionState::Failed`] (ARCHITECTURE §15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFailure {
    exit_code: Option<i32>,
    stderr_tail: StderrTail,
}

impl SessionFailure {
    /// Builds the failure the crash panel shows.
    ///
    /// `exit_code` is `None` when there is no code to show, which is two of SM3's three failure
    /// edges: mpv never started or never loaded the file, and a stall outlasted its threshold
    /// while mpv kept running. A code invented for those would report a stall as a crash. It is
    /// also `None` for a process the platform killed with a signal, which reports no code.
    #[must_use]
    pub const fn new(exit_code: Option<i32>, stderr_tail: StderrTail) -> Self {
        Self {
            exit_code,
            stderr_tail,
        }
    }

    /// The exit code, when mpv exited with one.
    #[must_use]
    pub const fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    /// The tail of mpv's stderr.
    #[must_use]
    pub const fn stderr_tail(&self) -> &StderrTail {
        &self.stderr_tail
    }
}

/// A session's state, as FLOWS SM3 names it.
///
/// The variants are the diagram's nodes in the order it draws them, so the mapping from SM3 to
/// this enum is the declaration order: `preparing`, `buffering`, `playing`, `stalled`, `paused`,
/// `ended`, `failed`. Each doc names the edges SM3 draws *out of* the state, so an edge that a
/// change adds or drops is visible in review and not only in a failing matrix test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionState {
    /// mpv is being spawned and told to load the file; no frame of this session has rendered
    /// yet. SM3: `preparing --> buffering`, and `preparing --> failed` on a spawn or load error.
    Preparing,
    /// The file is loading, and the first frame is what leaves this state. This is where a
    /// torrent stream with no peers parks; SM3 gives it `buffering --> stalled` when data stops
    /// arriving.
    Buffering,
    /// Frames are rendering. Reached only by a rendered frame, and left only by a pause, the end
    /// of the stream, or an mpv that exited (SM3's `playing -->` edges).
    Playing,
    /// No data arrived while the file was loading (SM3's `stalled --> buffering` when it does,
    /// and `stalled --> failed` past the threshold).
    Stalled,
    /// Stopped by the user and resumable (SM3's `paused --> playing`).
    Paused,
    /// Terminal: the stream ended, or the user stopped the session (F7's "user quits").
    Ended,
    /// Terminal: the session was given up on — mpv could not start or load the file, mpv
    /// exited, or a stall outlasted its threshold. [`Session::failure`] carries the exit code
    /// and stderr tail the UI shows, and [`Session::retry`] starts a new session at the last
    /// position.
    Failed,
}

impl SessionState {
    /// Every state, in SM3's order.
    ///
    /// The matrix tests walk this list, so a state added here has to have every cell of its row
    /// decided in [`Session::apply`] and declared in the table the tests pin.
    pub const ALL: [Self; 7] = [
        Self::Preparing,
        Self::Buffering,
        Self::Playing,
        Self::Stalled,
        Self::Paused,
        Self::Ended,
        Self::Failed,
    ];

    /// Whether nothing may leave this state.
    ///
    /// SM3 ends both `ended` and `failed` at the terminal marker. `failed` is not a place to
    /// retry from: its mpv process is gone, so [`Session::retry`] starts a new session and the
    /// report the old one held is history.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Ended | Self::Failed)
    }
}

/// Something that happened to a session, which may or may not move it.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// mpv was spawned and the file is loading (SM3's `preparing --> buffering`).
    Loaded,
    /// A frame reached the surface (SM3's `buffering --> playing`).
    ///
    /// This is the only event that enters [`SessionState::Playing`] from a state that has not
    /// played, and it is what makes F7's time-to-first-frame metric meaningful: the adapter
    /// reports it when mpv has actually put pixels on the surface, not when it has parsed the
    /// container.
    FrameRendered,
    /// No data arrived while the file was loading (SM3's `buffering --> stalled`).
    ///
    /// Only the load stalls. A stream that starves *while playing* stays in `playing`, because
    /// F7 shows buffer health over a live session, offers a pause there — which only `playing`
    /// takes — and every progress write keys off `playing`; leaving `playing` would stop the
    /// writes exactly when the user is most likely to close the player and lose their place.
    Stalled,
    /// Data arrived again, so the load can continue (SM3's `stalled --> buffering`).
    DataArrived,
    /// The user paused (SM3's `playing --> paused`).
    ///
    /// Only `playing` takes a pause: F7 mounts the overlay on `playing`, so there is no control
    /// to press before the first frame, and a paused session that had never rendered one would
    /// offer a resume that leads nowhere.
    Paused,
    /// The user resumed (SM3's `paused --> playing`).
    Resumed,
    /// The stream reached its end (SM3's `playing --> ended`).
    EndOfStream,
    /// The user stopped or closed the player (F7's "user quits or file ends").
    Stop,
    /// mpv failed, or the session gave up on it (SM3's three `--> failed` edges).
    ///
    /// One event covers a spawn error, a load error, a stall past its threshold, and an mpv that
    /// exited, because all four reach `failed`, show the same panel, and offer the same retry;
    /// [`SessionFailure`] is where they differ. F7 treats an EPIPE on the IPC socket as a crash
    /// rather than a hang, and this is the event that EPIPE arrives as.
    BackendFailed(SessionFailure),
    /// A position report from the player. It moves the position and not the state.
    ///
    /// This is the event F7 sends every 250 ms while playing, and the value every coalesced
    /// progress write persists.
    PositionReported(PlaybackPosition),
}

impl SessionEvent {
    /// The event's name, without its payload.
    ///
    /// A rejection names the event it refused, and a failure carries the tail of an mpv log that
    /// quotes the user's file names; an error string or a log line is not the place to repeat
    /// them.
    fn name(&self) -> &'static str {
        match self {
            Self::Loaded => "loaded",
            Self::FrameRendered => "frame rendered",
            Self::Stalled => "stalled",
            Self::DataArrived => "data arrived",
            Self::Paused => "paused",
            Self::Resumed => "resumed",
            Self::EndOfStream => "end of stream",
            Self::Stop => "stop",
            Self::BackendFailed(_) => "backend failed",
            Self::PositionReported(_) => "position reported",
        }
    }
}

impl fmt::Display for SessionEvent {
    /// The same name `SessionEvent::name` gives, so an event can be interpolated into a
    /// message without its payload coming with it.
    ///
    /// Not a link: `name` is private, and rustdoc refuses a public doc that points at one.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Why an event did not move a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SessionRejected {
    /// Nothing leaves a terminal state (SM3 ends both `ended` and `failed` at the terminal
    /// marker). A duplicate stop, or an exit reported after the user stopped the player and mpv
    /// was reaped, is refused here rather than allowed to reinterpret an ending that has already
    /// been finalised.
    #[error("{state:?} is terminal, so {event} cannot leave it")]
    Terminal {
        /// The state the event arrived in.
        state: SessionState,
        /// The refused event's name.
        event: &'static str,
    },
    /// The event is legal somewhere in the machine but not from this state: a position report
    /// after the session is over, a second load, a pause of something already paused.
    #[error("{event} is out of order in {state:?}")]
    OutOfOrder {
        /// The state the event arrived in.
        state: SessionState,
        /// The refused event's name.
        event: &'static str,
    },
    /// A retry was asked of a session that has not failed, so there is nothing to start again.
    /// Retrying a live session would leave its mpv child unreaped.
    #[error("{state:?} has not failed, so there is nothing to retry")]
    NotFailed {
        /// The state the retry arrived in.
        state: SessionState,
    },
}

/// One playback: its state, the last position it reported, and its failure once it has one.
///
/// The position is a field of the session rather than of the state, so a transition that fails
/// the session (SM3's `--> failed`) changes the state and leaves the position where the last
/// report put it. A retry built from a failed session therefore resumes rather than restarting.
///
/// The exit path hands [`Session::position`] and [`Session::ended_naturally`] to
/// [`crate::watch::finalize`], which owns the watched decision; nothing here computes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    state: SessionState,
    position: PlaybackPosition,
    failure: Option<SessionFailure>,
    ended_naturally: bool,
}

impl Session {
    /// Starts a session at the position the watch policy chose
    /// ([`crate::watch::resume_point`]), in `preparing`.
    #[must_use]
    pub const fn new(start: PlaybackPosition) -> Self {
        Self {
            state: SessionState::Preparing,
            position: start,
            failure: None,
            ended_naturally: false,
        }
    }

    /// The state the session is in.
    #[must_use]
    pub const fn state(&self) -> SessionState {
        self.state
    }

    /// The last position the player reported, or the one the session started at.
    #[must_use]
    pub const fn position(&self) -> PlaybackPosition {
        self.position
    }

    /// What failed, once [`SessionState::Failed`] is the state.
    #[must_use]
    pub const fn failure(&self) -> Option<&SessionFailure> {
        self.failure.as_ref()
    }

    /// Whether the stream reached its own end rather than being stopped.
    ///
    /// This is the flag [`crate::watch::finalize`] takes as `ended_naturally`; it is `false` for
    /// every state that is not a naturally ended session, including a failed one, where the
    /// watched decision falls back to the position's threshold.
    #[must_use]
    pub const fn ended_naturally(&self) -> bool {
        self.ended_naturally
    }

    /// Applies one event, or explains why it cannot move this session.
    ///
    /// A refused event leaves the session exactly as it was — no partial state, no position
    /// written — so a caller whose coalescing is behind cannot corrupt the position a retry will
    /// resume from.
    ///
    /// # Errors
    ///
    /// Returns [`SessionRejected`] when the event is not legal from the state it arrived in.
    pub fn apply(&mut self, event: SessionEvent) -> Result<(), SessionRejected> {
        // The name is read before the event is matched, so every rejection can name what it
        // refused without having to move a failure payload back out of the pattern that
        // consumed it.
        let name = event.name();
        match self.state {
            // The absorbing rule is one arm, so it holds for every event, including one that is
            // legal in the state the session was in before it ended.
            SessionState::Ended | SessionState::Failed => {
                return Err(SessionRejected::Terminal {
                    state: self.state,
                    event: name,
                });
            }
            SessionState::Preparing => match event {
                SessionEvent::Loaded => self.state = SessionState::Buffering,
                // mpv answers `time-pos` while the file is still opening, and the position it
                // reports is the one a crash during the load resumes from.
                SessionEvent::PositionReported(position) => self.position = position,
                // F7's "user quits" is legal wherever the session is: a torrent with no peers
                // parks in `preparing`, and a player that cannot be closed from there is not one.
                SessionEvent::Stop => self.state = SessionState::Ended,
                SessionEvent::BackendFailed(failure) => self.fail(failure),
                // A frame cannot precede the load, and a stall needs something that is loading.
                SessionEvent::FrameRendered
                | SessionEvent::Stalled
                | SessionEvent::DataArrived
                | SessionEvent::Paused
                | SessionEvent::Resumed
                | SessionEvent::EndOfStream => return Err(self.out_of_order(name)),
            },
            SessionState::Buffering => match event {
                // The one entry into `playing` from a session that has not played, and the
                // reason `playing` means "a frame the user could see".
                SessionEvent::FrameRendered => self.state = SessionState::Playing,
                SessionEvent::Stalled => self.state = SessionState::Stalled,
                SessionEvent::PositionReported(position) => self.position = position,
                SessionEvent::Stop => self.state = SessionState::Ended,
                // SM3 draws `failed` out of `preparing` and `playing`; mpv dying while the file
                // is loading is the same event, and without this cell the session would sit in
                // `buffering` with a dead child behind it and never fail.
                SessionEvent::BackendFailed(failure) => self.fail(failure),
                // An end with no frame is refused because mpv reports a file it cannot play as a
                // load error, which fails the session: treating it as a natural end would mark
                // an unplayable file watched at position zero (F10).
                SessionEvent::Loaded
                | SessionEvent::DataArrived
                | SessionEvent::Paused
                | SessionEvent::Resumed
                | SessionEvent::EndOfStream => return Err(self.out_of_order(name)),
            },
            SessionState::Playing => match event {
                SessionEvent::Paused => self.state = SessionState::Paused,
                SessionEvent::EndOfStream => {
                    self.ended_naturally = true;
                    self.state = SessionState::Ended;
                }
                SessionEvent::PositionReported(position) => self.position = position,
                SessionEvent::Stop => self.state = SessionState::Ended,
                SessionEvent::BackendFailed(failure) => self.fail(failure),
                // A second frame is what a late first-frame report looks like once the session
                // is already playing; refusing it tells the adapter its edge detection is behind
                // instead of letting the duplicate pass for progress.
                //
                // `Stalled` is refused here on purpose. F7's mid-playback starvation is a
                // buffer-health indicator over a session that is still playing, and this
                // machine's `stalled` is the load that never produced a frame.
                SessionEvent::Loaded
                | SessionEvent::FrameRendered
                | SessionEvent::Stalled
                | SessionEvent::DataArrived
                | SessionEvent::Resumed => return Err(self.out_of_order(name)),
            },
            SessionState::Stalled => match event {
                SessionEvent::DataArrived => self.state = SessionState::Buffering,
                // The cache can recover and a frame can appear before the adapter reports the
                // recovery, and a rendered frame is stronger evidence that data arrived than the
                // cache statistic is. Refusing it would leave a live session with no way back to
                // `playing` except a stop, which is the hang this machine exists to prevent.
                SessionEvent::FrameRendered => self.state = SessionState::Playing,
                SessionEvent::PositionReported(position) => self.position = position,
                SessionEvent::Stop => self.state = SessionState::Ended,
                // SM3's `stalled --> failed: timeout beyond threshold`, and the same event an
                // adapter uses for an mpv that exits while the load is starved.
                SessionEvent::BackendFailed(failure) => self.fail(failure),
                SessionEvent::Loaded
                | SessionEvent::Stalled
                | SessionEvent::Paused
                | SessionEvent::Resumed
                | SessionEvent::EndOfStream => return Err(self.out_of_order(name)),
            },
            SessionState::Paused => match event {
                SessionEvent::Resumed => self.state = SessionState::Playing,
                SessionEvent::PositionReported(position) => self.position = position,
                SessionEvent::Stop => self.state = SessionState::Ended,
                // mpv can die while the user has it paused — a driver reset, a crash when the
                // display wakes — and a session left in `paused` would keep offering a resume
                // that can never happen.
                SessionEvent::BackendFailed(failure) => self.fail(failure),
                // Nothing is flowing while paused, so there is no frame, no stall, and no end.
                SessionEvent::Loaded
                | SessionEvent::FrameRendered
                | SessionEvent::Stalled
                | SessionEvent::DataArrived
                | SessionEvent::Paused
                | SessionEvent::EndOfStream => return Err(self.out_of_order(name)),
            },
        }
        Ok(())
    }

    /// The session to start when the user retries, or a refusal when there is nothing to retry.
    ///
    /// F7 offers the retry at the position the failed session last held, so this is the same
    /// position and a fresh state: SM3 ends `failed` at the terminal marker, and the mpv process
    /// the failed session owned is gone, so a retry cannot be an edge of it. Refusing a retry of
    /// a live session is what stops a service from spawning a second mpv behind the first one
    /// and leaking the child it no longer has a handle to.
    ///
    /// # Errors
    ///
    /// [`SessionRejected::NotFailed`] when the session has not failed.
    pub fn retry(&self) -> Result<Self, SessionRejected> {
        if self.state != SessionState::Failed {
            return Err(SessionRejected::NotFailed { state: self.state });
        }
        Ok(Self::new(self.position))
    }

    /// Records a failure, keeping the position the session last held.
    ///
    /// The position is deliberately not touched: it is the resume point the retry affordance
    /// needs, and zero would quietly restart the playback from the beginning.
    fn fail(&mut self, failure: SessionFailure) {
        self.failure = Some(failure);
        self.state = SessionState::Failed;
    }

    /// The rejection for an event that is legal elsewhere in the machine.
    fn out_of_order(&self, event: &'static str) -> SessionRejected {
        SessionRejected::OutOfOrder {
            state: self.state,
            event,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// A position for the fixtures.
    fn position(position_ms: i64, duration_ms: Option<i64>) -> PlaybackPosition {
        PlaybackPosition::new(position_ms, duration_ms).expect("the fixture position is valid")
    }

    /// An mpv that exited with a code, and the end of the log it left.
    fn crashed() -> SessionFailure {
        SessionFailure::new(
            Some(11),
            StderrTail::from_log("mpv: fatal error at frame 41"),
        )
    }

    /// What an event is without its payload: `Copy`, so the matrix can be a `const`, and separate
    /// from [`SessionEvent`], so a failure or a position is only built when a cell is exercised.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Kind {
        /// [`SessionEvent::Loaded`].
        Loaded,
        /// [`SessionEvent::FrameRendered`].
        FrameRendered,
        /// [`SessionEvent::Stalled`].
        Stalled,
        /// [`SessionEvent::DataArrived`].
        DataArrived,
        /// [`SessionEvent::Paused`].
        Paused,
        /// [`SessionEvent::Resumed`].
        Resumed,
        /// [`SessionEvent::EndOfStream`].
        EndOfStream,
        /// [`SessionEvent::Stop`].
        Stop,
        /// [`SessionEvent::BackendFailed`] with the crash fixture.
        BackendFailed,
        /// [`SessionEvent::PositionReported`] at the fixture position.
        PositionReported,
    }

    impl Kind {
        /// The real event.
        fn event(self) -> SessionEvent {
            match self {
                Self::Loaded => SessionEvent::Loaded,
                Self::FrameRendered => SessionEvent::FrameRendered,
                Self::Stalled => SessionEvent::Stalled,
                Self::DataArrived => SessionEvent::DataArrived,
                Self::Paused => SessionEvent::Paused,
                Self::Resumed => SessionEvent::Resumed,
                Self::EndOfStream => SessionEvent::EndOfStream,
                Self::Stop => SessionEvent::Stop,
                Self::BackendFailed => SessionEvent::BackendFailed(crashed()),
                Self::PositionReported => SessionEvent::PositionReported(position(0, None)),
            }
        }
    }

    /// Every event kind a caller can produce.
    const KINDS: &[Kind] = &[
        Kind::Loaded,
        Kind::FrameRendered,
        Kind::Stalled,
        Kind::DataArrived,
        Kind::Paused,
        Kind::Resumed,
        Kind::EndOfStream,
        Kind::Stop,
        Kind::BackendFailed,
        Kind::PositionReported,
    ];

    /// A path of events to each state, so the matrix builds a session the way a caller reaches
    /// one instead of writing the state down. Every state must appear exactly once, which is what
    /// makes a state added to SM3 fail the coverage test below until its path is given.
    const PATHS: &[(SessionState, &[Kind])] = &[
        (SessionState::Preparing, &[]),
        (SessionState::Buffering, &[Kind::Loaded]),
        (SessionState::Playing, &[Kind::Loaded, Kind::FrameRendered]),
        (SessionState::Stalled, &[Kind::Loaded, Kind::Stalled]),
        (
            SessionState::Paused,
            &[Kind::Loaded, Kind::FrameRendered, Kind::Paused],
        ),
        (
            SessionState::Ended,
            &[Kind::Loaded, Kind::FrameRendered, Kind::EndOfStream],
        ),
        (SessionState::Failed, &[Kind::Loaded, Kind::BackendFailed]),
    ];

    /// The session a caller reaches by applying every event of its path.
    fn drive(kinds: &[Kind]) -> Session {
        let mut session = Session::new(position(0, None));
        for kind in kinds {
            session
                .apply(kind.event())
                .expect("the declared path is legal");
        }
        session
    }

    /// A session in `state`, reached by the path [`PATHS`] declares.
    fn session_in(state: SessionState) -> Session {
        let path = PATHS
            .iter()
            .find(|(candidate, _)| *candidate == state)
            .map(|(_, path)| *path)
            .expect("every state has a path");
        drive(path)
    }

    /// The legal cells of the state × event matrix: `(from, event, to)`. Anything absent is
    /// illegal, which is the claim the matrix test checks against [`Session::apply`] both ways.
    ///
    /// Three groups of cells are deliberate rather than incidental:
    ///
    /// - `PositionReported` returns the state it was given from every live state, because it
    ///   carries no state; it is refused once the session is terminal, which tells a caller whose
    ///   coalescing is behind rather than hiding the fact behind an `Ok`.
    /// - `Stop` reaches `ended` from every live state (F7's "user quits") and is refused by the
    ///   two terminal states.
    /// - `BackendFailed` reaches `failed` from every live state: a crash has no favourite state,
    ///   and a session that could not report one would hang with a dead child behind it.
    const LEGAL: &[(SessionState, Kind, SessionState)] = &[
        (
            SessionState::Preparing,
            Kind::Loaded,
            SessionState::Buffering,
        ),
        (
            SessionState::Preparing,
            Kind::PositionReported,
            SessionState::Preparing,
        ),
        (SessionState::Preparing, Kind::Stop, SessionState::Ended),
        (
            SessionState::Preparing,
            Kind::BackendFailed,
            SessionState::Failed,
        ),
        (
            SessionState::Buffering,
            Kind::FrameRendered,
            SessionState::Playing,
        ),
        (
            SessionState::Buffering,
            Kind::Stalled,
            SessionState::Stalled,
        ),
        (
            SessionState::Buffering,
            Kind::PositionReported,
            SessionState::Buffering,
        ),
        (SessionState::Buffering, Kind::Stop, SessionState::Ended),
        (
            SessionState::Buffering,
            Kind::BackendFailed,
            SessionState::Failed,
        ),
        (SessionState::Playing, Kind::Paused, SessionState::Paused),
        (
            SessionState::Playing,
            Kind::EndOfStream,
            SessionState::Ended,
        ),
        (
            SessionState::Playing,
            Kind::PositionReported,
            SessionState::Playing,
        ),
        (SessionState::Playing, Kind::Stop, SessionState::Ended),
        (
            SessionState::Playing,
            Kind::BackendFailed,
            SessionState::Failed,
        ),
        (
            SessionState::Stalled,
            Kind::DataArrived,
            SessionState::Buffering,
        ),
        (
            SessionState::Stalled,
            Kind::FrameRendered,
            SessionState::Playing,
        ),
        (
            SessionState::Stalled,
            Kind::PositionReported,
            SessionState::Stalled,
        ),
        (SessionState::Stalled, Kind::Stop, SessionState::Ended),
        (
            SessionState::Stalled,
            Kind::BackendFailed,
            SessionState::Failed,
        ),
        (SessionState::Paused, Kind::Resumed, SessionState::Playing),
        (
            SessionState::Paused,
            Kind::PositionReported,
            SessionState::Paused,
        ),
        (SessionState::Paused, Kind::Stop, SessionState::Ended),
        (
            SessionState::Paused,
            Kind::BackendFailed,
            SessionState::Failed,
        ),
    ];

    /// The matrix's own answer for one cell, or `None` when the cell is illegal.
    fn declared(state: SessionState, kind: Kind) -> Option<SessionState> {
        LEGAL
            .iter()
            .find(|(from, candidate, _)| *from == state && *candidate == kind)
            .map(|(_, _, to)| *to)
    }

    fn state_strategy() -> impl Strategy<Value = SessionState> {
        prop::sample::select(&SessionState::ALL[..])
    }

    fn kind_strategy() -> impl Strategy<Value = Kind> {
        prop::sample::select(KINDS)
    }

    /// Whether the event was refused for the absorbing reason, as a function so that
    /// `prop_assert!` can name it without a struct pattern in its format string.
    fn is_terminal_rejection(state: SessionState, kind: Kind) -> bool {
        matches!(
            session_in(state).apply(kind.event()),
            Err(SessionRejected::Terminal { .. })
        )
    }

    #[test]
    fn the_path_table_reaches_every_state_exactly_once() {
        assert_eq!(
            PATHS.iter().map(|(state, _)| *state).collect::<Vec<_>>(),
            SessionState::ALL.to_vec(),
            "SM3's states and the paths to them have drifted apart"
        );
    }

    /// Acceptance: every state is reachable, and it is reached by applying events to a session
    /// rather than by naming the variant.
    #[test]
    fn every_state_is_reachable_from_the_initial_state() {
        let mut reached = vec![SessionState::Preparing];
        let mut frontier = vec![SessionState::Preparing];
        while let Some(state) = frontier.pop() {
            for kind in KINDS {
                let mut session = session_in(state);
                if session.apply(kind.event()).is_ok() {
                    let next = session.state();
                    if !reached.contains(&next) {
                        reached.push(next);
                        frontier.push(next);
                    }
                }
            }
        }
        for state in SessionState::ALL {
            assert!(reached.contains(&state), "{state:?} is unreachable");
        }
        assert_eq!(reached.len(), SessionState::ALL.len());
    }

    /// Every cell of the state × event matrix, in both directions: a cell the table declares
    /// legal must move to exactly that state, a cell it does not declare must be refused with
    /// the right reason, and a refusal must leave the session untouched.
    #[test]
    fn only_the_declared_cells_of_the_matrix_are_legal() {
        for state in SessionState::ALL {
            for kind in KINDS {
                let mut session = session_in(state);
                let before = session.clone();
                let outcome = session.apply(kind.event());
                match (outcome, declared(state, *kind)) {
                    (Ok(()), Some(expected)) => {
                        assert_eq!(
                            session.state(),
                            expected,
                            "{state:?} + {kind:?} moved to {:?}, not {expected:?}",
                            session.state()
                        );
                    }
                    (Err(rejection), None) => {
                        assert_eq!(
                            session, before,
                            "{state:?} + {kind:?} was refused but changed the session"
                        );
                        let expected = if state.is_terminal() {
                            matches!(rejection, SessionRejected::Terminal { .. })
                        } else {
                            matches!(rejection, SessionRejected::OutOfOrder { .. })
                        };
                        assert!(
                            expected,
                            "{state:?} + {kind:?} was refused with the wrong reason: {rejection}"
                        );
                    }
                    (Ok(()), None) => panic!(
                        "{state:?} + {kind:?} moved to {:?}, but the table calls it illegal",
                        session.state()
                    ),
                    (Err(rejection), Some(expected)) => panic!(
                        "{state:?} + {kind:?} should move to {expected:?}, but was refused: \
                         {rejection}"
                    ),
                }
            }
        }
    }

    /// A table with a duplicate cell could hide a missing one, and an entry naming an event
    /// outside [`KINDS`] would never be exercised by the matrix test above.
    #[test]
    fn the_declared_matrix_has_no_duplicate_cell_and_no_stray_kinds() {
        for (index, (state, kind, _)) in LEGAL.iter().enumerate() {
            assert!(
                !LEGAL[..index]
                    .iter()
                    .any(|(seen_state, seen_kind, _)| seen_state == state && seen_kind == kind),
                "{state:?} + {kind:?} is declared twice"
            );
            assert!(
                SessionState::ALL.contains(state),
                "{state:?} is not a state"
            );
            assert!(
                KINDS.contains(kind),
                "{kind:?} is not a kind the matrix tests"
            );
        }
    }

    /// Rejection is not one bucket: being finished, racing the session, and asking to retry
    /// something that never failed are different answers.
    #[test]
    fn a_rejection_names_why_it_was_refused() {
        assert_eq!(
            session_in(SessionState::Playing).apply(SessionEvent::Loaded),
            Err(SessionRejected::OutOfOrder {
                state: SessionState::Playing,
                event: "loaded",
            })
        );
        assert_eq!(
            session_in(SessionState::Ended).apply(SessionEvent::Stop),
            Err(SessionRejected::Terminal {
                state: SessionState::Ended,
                event: "stop",
            })
        );
        assert_eq!(
            session_in(SessionState::Playing)
                .retry()
                .expect_err("a playing session has nothing to retry"),
            SessionRejected::NotFailed {
                state: SessionState::Playing,
            }
        );
    }

    /// A terminal state absorbs every event, not just the late one a test thought of.
    #[test]
    fn a_terminal_state_absorbs_every_event() {
        for state in SessionState::ALL
            .iter()
            .copied()
            .filter(|state| state.is_terminal())
        {
            for kind in KINDS {
                assert_eq!(
                    session_in(state).apply(kind.event()),
                    Err(SessionRejected::Terminal {
                        state,
                        event: kind.event().name(),
                    }),
                    "{state:?} let {kind:?} out"
                );
            }
        }
    }

    /// The load-bearing rule, as a cell test: `playing` is reached by a rendered frame, and by a
    /// resume from `paused` — a state that only a session that has already played can be in,
    /// which the path property below proves.
    #[test]
    fn playing_is_entered_by_a_rendered_frame_or_a_resume_only() {
        for state in SessionState::ALL {
            for kind in KINDS {
                if declared(state, *kind) != Some(SessionState::Playing) {
                    continue;
                }
                // A report that leaves the state where it is is not an entry into it: the
                // position moves and the session is still playing.
                if state == SessionState::Playing {
                    assert_eq!(
                        *kind,
                        Kind::PositionReported,
                        "{state:?} + {kind:?} is declared as a move inside playing"
                    );
                    continue;
                }
                let from_a_frame = *kind == Kind::FrameRendered;
                let a_resume_from_paused = state == SessionState::Paused && *kind == Kind::Resumed;
                assert!(
                    from_a_frame || a_resume_from_paused,
                    "{state:?} + {kind:?} reaches playing without a rendered frame"
                );
            }
        }

        // The negative control: the cells that would break the rule if they were legal are
        // refusals rather than transitions some other test happens not to look at.
        for (state, kind) in [
            (SessionState::Preparing, Kind::FrameRendered),
            (SessionState::Buffering, Kind::Resumed),
            (SessionState::Playing, Kind::FrameRendered),
            (SessionState::Paused, Kind::FrameRendered),
            (SessionState::Ended, Kind::FrameRendered),
            (SessionState::Failed, Kind::Resumed),
        ] {
            assert!(
                session_in(state).apply(kind.event()).is_err(),
                "{state:?} + {kind:?} moved a session that never played into playing"
            );
        }
    }

    /// ARCHITECTURE §15: the failure panel shows the exit code and the tail of what mpv printed.
    #[test]
    fn a_failure_carries_the_exit_code_and_a_bounded_tail_of_stderr() {
        let log = format!(
            "{}mpv: fatal error: could not open the stream\n",
            "noise\n".repeat(200_000)
        );
        let failure = SessionFailure::new(Some(11), StderrTail::from_log(&log));
        assert_eq!(failure.exit_code(), Some(11));
        assert!(failure.stderr_tail().truncated());
        assert!(failure.stderr_tail().text().len() <= MAX_STDERR_TAIL_BYTES);
        assert!(
            failure
                .stderr_tail()
                .text()
                .ends_with("could not open the stream\n"),
            "the tail keeps the end of the log, where a crash explains itself"
        );

        let mut session = drive(&[Kind::Loaded, Kind::FrameRendered]);
        session
            .apply(SessionEvent::PositionReported(position(
                42_000,
                Some(100_000),
            )))
            .expect("a position report is legal while playing");
        session
            .apply(SessionEvent::BackendFailed(failure.clone()))
            .expect("an mpv that exited fails the session");
        assert_eq!(session.state(), SessionState::Failed);
        assert_eq!(session.failure(), Some(&failure));
        assert_eq!(
            session.position().position_ms(),
            42_000,
            "the failure must not move the position the retry offers"
        );
    }

    /// The boundary of the cap, including the character the cut can land inside.
    #[test]
    fn an_over_long_log_is_cut_to_its_last_bytes_at_a_character_boundary() {
        let at_the_cap = StderrTail::from_log(&"a".repeat(MAX_STDERR_TAIL_BYTES));
        assert_eq!(at_the_cap.text().len(), MAX_STDERR_TAIL_BYTES);
        assert!(
            !at_the_cap.truncated(),
            "exactly at the cap is not truncated"
        );

        let one_over = StderrTail::from_log(&"a".repeat(MAX_STDERR_TAIL_BYTES + 1));
        assert_eq!(one_over.text().len(), MAX_STDERR_TAIL_BYTES);
        assert!(one_over.truncated());

        // The cut lands inside the three-byte `€`, so the kept text starts at the next boundary
        // instead of slicing a character in half.
        let log = format!("aaa€{}", "b".repeat(MAX_STDERR_TAIL_BYTES - 2));
        let tail = StderrTail::from_log(&log);
        assert!(tail.truncated());
        assert!(tail.text().starts_with('b'));
        assert!(!tail.text().contains('€'));
        assert!(tail.text().len() <= MAX_STDERR_TAIL_BYTES);

        // An empty log is a failure with nothing to show, not a panic.
        let empty = StderrTail::from_log("");
        assert_eq!(empty.text(), "");
        assert!(!empty.truncated());
    }

    /// The two failure edges with no exit code behind them: a spawn or load error, and a stall
    /// that outlasted its threshold while mpv kept running.
    #[test]
    fn a_failure_without_an_exit_code_is_still_a_failure() {
        let failure = SessionFailure::new(None, StderrTail::from_log(""));
        assert_eq!(failure.exit_code(), None);

        // SM3's `preparing --> failed: spawn or load error`.
        let mut spawn = Session::new(position(0, None));
        spawn
            .apply(SessionEvent::BackendFailed(failure.clone()))
            .expect("a load error fails the session before it buffers");
        assert_eq!(spawn.state(), SessionState::Failed);

        // SM3's `stalled --> failed: timeout beyond threshold`.
        let mut stalled = drive(&[Kind::Loaded, Kind::Stalled]);
        stalled
            .apply(SessionEvent::BackendFailed(failure))
            .expect("a stall past its threshold fails the session");
        assert_eq!(stalled.state(), SessionState::Failed);
        assert_eq!(
            stalled.failure().and_then(SessionFailure::exit_code),
            None,
            "a stall has no exit code, and inventing one would report it as a crash"
        );
    }

    /// F7 and ARCHITECTURE §15: the retry resumes at the last known position, not at zero.
    #[test]
    fn a_retry_resumes_at_the_last_position_rather_than_zero() {
        let mut session = drive(&[Kind::Loaded, Kind::FrameRendered]);
        session
            .apply(SessionEvent::PositionReported(position(
                42_000,
                Some(100_000),
            )))
            .expect("a position report is legal while playing");
        session
            .apply(SessionEvent::BackendFailed(crashed()))
            .expect("the crash fails the session");

        let resumed = session.retry().expect("a failed session retries");
        assert_eq!(resumed.state(), SessionState::Preparing);
        assert_eq!(resumed.position().position_ms(), 42_000);
        assert_eq!(resumed.position().duration_ms(), Some(100_000));
        assert_ne!(
            resumed.position().position_ms(),
            0,
            "a retry that starts over at zero loses the user's place"
        );
        assert_eq!(resumed.failure(), None, "a new session has not failed");
        assert!(!resumed.ended_naturally());

        // A position report is also how a load that crashed before its first frame resumes.
        let mut early = drive(&[Kind::Loaded]);
        early
            .apply(SessionEvent::PositionReported(position(7_000, None)))
            .expect("time-pos answers while the file is opening");
        early
            .apply(SessionEvent::BackendFailed(crashed()))
            .expect("the crash fails the session");
        assert_eq!(
            early.retry().expect("a failed session retries").position(),
            position(7_000, None)
        );
    }

    /// Retry is refused until there is something to retry, so a live session cannot be
    /// restarted behind the back of the mpv child it still owns.
    #[test]
    fn a_retry_is_refused_unless_the_session_failed() {
        for state in SessionState::ALL {
            let session = session_in(state);
            match session.retry() {
                Ok(retried) => {
                    assert_eq!(state, SessionState::Failed);
                    assert_eq!(retried.state(), SessionState::Preparing);
                }
                Err(SessionRejected::NotFailed { state: refused }) => {
                    assert_eq!(refused, state);
                    assert_ne!(state, SessionState::Failed);
                }
                Err(other) => panic!("{state:?} refused a retry with {other}"),
            }
        }
    }

    /// The cancellation matrix: which states take a stop, and what a stop means in each.
    #[test]
    fn every_live_state_accepts_a_stop_and_the_terminal_states_refuse_it() {
        /// What a stop does in one state.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum Stop {
            /// The session ends, by the user's hand rather than the stream's.
            Ends,
            /// The session is already over, so nothing moves.
            Refused,
        }

        /// Every state's answer, in SM3's order. A state added to the machine has to be given
        /// one here rather than defaulting.
        const STOPS: &[(SessionState, Stop)] = &[
            (SessionState::Preparing, Stop::Ends),
            (SessionState::Buffering, Stop::Ends),
            (SessionState::Playing, Stop::Ends),
            (SessionState::Stalled, Stop::Ends),
            (SessionState::Paused, Stop::Ends),
            (SessionState::Ended, Stop::Refused),
            (SessionState::Failed, Stop::Refused),
        ];

        assert_eq!(
            STOPS.iter().map(|(state, _)| *state).collect::<Vec<_>>(),
            SessionState::ALL.to_vec(),
            "the cancellation matrix and SM3's states have drifted apart"
        );

        for (state, expected) in STOPS {
            let mut session = session_in(*state);
            let before = session.clone();
            match (session.apply(SessionEvent::Stop), *expected) {
                (Ok(()), Stop::Ends) => {
                    assert_eq!(session.state(), SessionState::Ended);
                    assert!(
                        !session.ended_naturally(),
                        "a stop is not the stream reaching its end, and the watched decision \
                         must see the difference"
                    );
                    assert_ne!(
                        session, before,
                        "a stop that accepted must have moved the session"
                    );
                }
                (Err(SessionRejected::Terminal { .. }), Stop::Refused) => {
                    assert_eq!(session, before, "a refused stop must change nothing");
                    assert!(
                        state.is_terminal(),
                        "{state:?} refused a stop but is not terminal"
                    );
                }
                (outcome, expected) => {
                    panic!("{state:?} answered a stop with {outcome:?}, not {expected:?}");
                }
            }
        }

        // A stop keeps the position, because the exit path finalises progress from it.
        let mut stopped = drive(&[Kind::Loaded, Kind::FrameRendered]);
        stopped
            .apply(SessionEvent::PositionReported(position(
                31_000,
                Some(100_000),
            )))
            .expect("a position report is legal while playing");
        stopped
            .apply(SessionEvent::Stop)
            .expect("a playing session stops");
        assert_eq!(stopped.state(), SessionState::Ended);
        assert_eq!(stopped.position().position_ms(), 31_000);
        assert!(!stopped.ended_naturally());
    }

    /// AGENTS §4's duplicate, interrupted, and slow rows: each of these is refused or ignored on
    /// purpose, and the choice is written down.
    #[test]
    fn duplicate_and_late_events_are_refused_or_ignored_by_rule() {
        // A second load, a second pause, and a resume of something that is not paused are out of
        // order rather than silently absorbed.
        assert_eq!(
            drive(&[Kind::Loaded]).apply(SessionEvent::Loaded),
            Err(SessionRejected::OutOfOrder {
                state: SessionState::Buffering,
                event: "loaded",
            })
        );
        assert_eq!(
            drive(&[Kind::Loaded, Kind::FrameRendered, Kind::Paused]).apply(SessionEvent::Paused),
            Err(SessionRejected::OutOfOrder {
                state: SessionState::Paused,
                event: "paused",
            })
        );
        assert_eq!(
            drive(&[Kind::Loaded]).apply(SessionEvent::Resumed),
            Err(SessionRejected::OutOfOrder {
                state: SessionState::Buffering,
                event: "resumed",
            })
        );
        // A duplicate first frame after the session is already playing: refused, so an adapter
        // whose edge detection is behind learns it.
        assert_eq!(
            drive(&[Kind::Loaded, Kind::FrameRendered]).apply(SessionEvent::FrameRendered),
            Err(SessionRejected::OutOfOrder {
                state: SessionState::Playing,
                event: "frame rendered",
            })
        );
        // A position report after the session ended is refused rather than written: the exit path
        // has already finalised the row, and a late tick belongs to the session that replaced
        // this one.
        assert_eq!(
            drive(&[Kind::Loaded, Kind::FrameRendered, Kind::EndOfStream])
                .apply(SessionEvent::PositionReported(position(99_000, None))),
            Err(SessionRejected::Terminal {
                state: SessionState::Ended,
                event: "position reported",
            })
        );
        // The second terminal event of each kind: a stop after a stop, an end after an end, and
        // the crash of a process the user already stopped and the service already reaped. The
        // last one matters most: mpv exiting after a deliberate stop is not a crash, and turning
        // it into one would show the user a failure panel for a player they closed.
        for (path, event) in [
            (
                &[Kind::Loaded, Kind::FrameRendered, Kind::Stop][..],
                SessionEvent::Stop,
            ),
            (
                &[Kind::Loaded, Kind::FrameRendered, Kind::EndOfStream][..],
                SessionEvent::EndOfStream,
            ),
            (
                &[Kind::Loaded, Kind::FrameRendered, Kind::Stop][..],
                SessionEvent::BackendFailed(crashed()),
            ),
            (&[Kind::Loaded, Kind::BackendFailed][..], SessionEvent::Stop),
        ] {
            let mut session = drive(path);
            let state = session.state();
            let name = event.name();
            assert_eq!(
                session.apply(event),
                Err(SessionRejected::Terminal { state, event: name }),
                "a duplicate terminal event must not move {state:?}"
            );
        }
    }

    /// SM3's stall edges: a load can recover, and a load that never recovers can be given up on.
    #[test]
    fn a_stalled_load_recovers_or_fails_by_rule() {
        let mut session = drive(&[Kind::Loaded, Kind::Stalled]);
        assert_eq!(session.state(), SessionState::Stalled);
        session
            .apply(SessionEvent::DataArrived)
            .expect("data arriving returns the load to buffering");
        assert_eq!(session.state(), SessionState::Buffering);
        session
            .apply(SessionEvent::FrameRendered)
            .expect("the first frame leaves buffering");
        assert_eq!(session.state(), SessionState::Playing);

        // The cache statistic can be behind the frame it caused: a rendered frame is stronger
        // evidence than the statistic, and it is the state a session has to be able to reach.
        let mut direct = drive(&[Kind::Loaded, Kind::Stalled]);
        direct
            .apply(SessionEvent::FrameRendered)
            .expect("a frame proves the stall is over");
        assert_eq!(direct.state(), SessionState::Playing);

        // F7's mid-playback starvation is not this state: the session keeps playing and keeps
        // writing progress while the UI shows buffer health.
        assert_eq!(
            drive(&[Kind::Loaded, Kind::FrameRendered]).apply(SessionEvent::Stalled),
            Err(SessionRejected::OutOfOrder {
                state: SessionState::Playing,
                event: "stalled",
            })
        );
    }

    /// The end of the stream is the one ending that reaches the watched policy as a natural end.
    #[test]
    fn only_the_end_of_the_stream_is_a_natural_end() {
        let mut natural = drive(&[Kind::Loaded, Kind::FrameRendered]);
        natural
            .apply(SessionEvent::EndOfStream)
            .expect("the file can end");
        assert_eq!(natural.state(), SessionState::Ended);
        assert!(natural.ended_naturally());

        // A file that ends before its first frame is a load error, not a natural end: mpv reports
        // it as one, and marking an unplayable file watched at position zero is the failure this
        // refusal prevents.
        assert_eq!(
            drive(&[Kind::Loaded]).apply(SessionEvent::EndOfStream),
            Err(SessionRejected::OutOfOrder {
                state: SessionState::Buffering,
                event: "end of stream",
            })
        );
        assert_eq!(
            drive(&[Kind::Loaded, Kind::Stalled]).apply(SessionEvent::EndOfStream),
            Err(SessionRejected::OutOfOrder {
                state: SessionState::Stalled,
                event: "end of stream",
            })
        );
    }

    proptest! {
        #[test]
        fn the_same_state_and_event_always_produce_the_same_result(
            state in state_strategy(),
            kind in kind_strategy(),
        ) {
            prop_assert_eq!(
                session_in(state).apply(kind.event()),
                session_in(state).apply(kind.event())
            );
        }

        #[test]
        fn a_transition_agrees_with_the_declared_matrix(
            state in state_strategy(),
            kind in kind_strategy(),
        ) {
            let mut session = session_in(state);
            prop_assert_eq!(
                session.apply(kind.event()).ok().map(|()| session.state()),
                declared(state, kind)
            );
        }

        #[test]
        fn no_event_leaves_a_terminal_state(
            state in state_strategy(),
            kind in kind_strategy(),
        ) {
            if state.is_terminal() {
                prop_assert!(is_terminal_rejection(state, kind), "{state:?} let {kind:?} out");
            }
        }

        /// The load-bearing rule along a whole path, not one step: `playing` and `paused` are
        /// reached only by a session that has rendered a frame. A refused event grants nothing.
        #[test]
        fn no_event_path_reaches_playing_without_a_rendered_frame(
            kinds in prop::collection::vec(kind_strategy(), 0..24),
        ) {
            let mut session = Session::new(position(0, None));
            let mut rendered = false;
            for kind in kinds {
                if session.apply(kind.event()).is_ok() {
                    rendered |= kind == Kind::FrameRendered;
                }
                if matches!(session.state(), SessionState::Playing | SessionState::Paused) {
                    prop_assert!(
                        rendered,
                        "reached {:?} without a rendered frame",
                        session.state()
                    );
                }
            }
        }

        /// A tail of any log stays inside the cap and stays the end of that log, which is the
        /// promise the failure panel depends on: a huge log cannot become a huge event, and what
        /// is kept is the part that explains the crash.
        #[test]
        fn a_tail_of_any_log_is_bounded_and_ends_the_log(log in any::<String>()) {
            let tail = StderrTail::from_log(&log);
            prop_assert!(tail.text().len() <= MAX_STDERR_TAIL_BYTES);
            prop_assert!(log.ends_with(tail.text()));
            prop_assert_eq!(tail.truncated(), log.len() > MAX_STDERR_TAIL_BYTES);
        }
    }
}
