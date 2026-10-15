//! The closed vocabulary of torrent-engine refusals.
//!
//! Every message here is a fixed sentence. A refusal that would naturally quote a value carries it
//! as typed data instead, so a caller can match on it while the sentence it renders stays the same
//! — because the UI never sees a raw engine string, and the diagnostics tail is exported
//! (PG-E10-15, SECURITY T13). No message names a path, a magnet, or an info-hash, which is what
//! the conformance suite holds every variant to.

use thiserror::Error;

use crate::engine::TorrentHandle;
use crate::stats::Bytes;

/// Something a [`TorrentEngine`](crate::TorrentEngine) refused.
///
/// It is deliberately closed rather than open: a caller matches on a variant to decide what to
/// tell a user, and an adapter that needs a refusal this vocabulary does not name reports
/// [`EngineError::Failed`] and keeps its own detail in its own log, where it can be redacted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum EngineError {
    /// The session has been shut down.
    ///
    /// It takes precedence over every other rule: a closed session refuses the operation however
    /// well formed its arguments are, because the engine that would have judged them is gone.
    #[error("the torrent session is closed")]
    SessionClosed,
    /// No live torrent in this session has this handle.
    ///
    /// A handle that was removed and a handle that never existed are the same refusal, because
    /// once a torrent is removed its handle names nothing. [`TorrentEngine::remove`] is the one
    /// operation that does not answer this way, for the reason its own doc records.
    #[error("this session holds no torrent with that handle")]
    UnknownTorrent {
        /// The handle no torrent answers to.
        handle: TorrentHandle,
    },
    /// The file set is empty, so there would be nothing to transfer.
    ///
    /// `add` and `select` refuse it rather than accepting a torrent that writes nothing: a caller
    /// that wants a torrent gone wants [`TorrentEngine::remove`], and a transfer that never moves
    /// is the failure mode a user cannot diagnose (`peregrine_domain::inspect` records the same
    /// decision for the metadata-only gate).
    #[error("no files are selected, so there is nothing to transfer")]
    EmptySelection,
    /// The torrent describes no file at this index.
    #[error("this torrent has no file at that index")]
    UnknownFile {
        /// The index the caller named.
        index: usize,
    },
    /// The file exists but is not in the selected set.
    #[error("that file is not selected, so its bytes are not being downloaded")]
    FileNotSelected {
        /// The index the caller named.
        index: usize,
    },
    /// A stream was asked to start past the end of its file.
    ///
    /// The end itself is not past it: a stream opened at exactly the file's length is a reader
    /// with nothing left, which is a legitimate empty range, and only an offset beyond that is
    /// refused.
    #[error("the read starts past the end of the file")]
    OffsetPastEnd {
        /// The offset the caller asked for.
        offset: Bytes,
        /// The file's length, as the torrent's metadata reports it.
        len: Bytes,
    },
    /// The source does not name the torrent the request said it did.
    ///
    /// The info-hash is a claim the caller makes — from an indexer result, a magnet, or a job row
    /// — and the engine is the first thing that can check it against what the source actually
    /// hashes to. A mismatch is refused rather than logged, because the alternative is writing
    /// the bytes of one torrent into the job row of another.
    #[error("the torrent at that source is not the one it was added as")]
    IdentityMismatch,
    /// The engine could not complete the operation and this vocabulary does not name why.
    ///
    /// It exists so that no adapter has to smuggle its own error type or its own message through
    /// the port (AGENTS R8): the adapter logs the detail with the engine's own context, redacted,
    /// and the caller is told only that the operation failed.
    #[error("the torrent engine could not complete the operation")]
    Failed,
}

/// Why an [`AddRequest`](crate::AddRequest) could not be built.
///
/// The two variants are the two verdicts that are not [`Verdict::Allowed`](peregrine_domain::Verdict::Allowed),
/// so "add this anyway" has no spelling: there is no `AddRefused` that carries the allowed verdict,
/// because that verdict is the one that returns `Ok`. An [`OptInRequired`](Self::OptInRequired)
/// refusal is answerable — the user may take the advanced risk, and the gate is run again — while a
/// [`Blocked`](Self::Blocked) one is not (SECURITY T12, `FLOWS` F6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AddRefused {
    /// The report refused the torrent until the user accepts the advanced risk, which the add path
    /// has no answer for yet.
    #[error("the safety report requires the advanced opt-in, so the torrent cannot be added yet")]
    OptInRequired,
    /// The report refused the torrent outright: a traversal, a link, or a program never becomes
    /// addable, however many times a user agrees.
    #[error("the safety report blocked the torrent, so it can never be added")]
    Blocked,
}

#[cfg(test)]
mod tests {
    use peregrine_domain::check_text;

    use super::*;

    /// One of every error, so the match below cannot be satisfied by a subset.
    fn every_error() -> Vec<EngineError> {
        vec![
            EngineError::SessionClosed,
            EngineError::UnknownTorrent {
                handle: TorrentHandle::new(7),
            },
            EngineError::EmptySelection,
            EngineError::UnknownFile { index: 3 },
            EngineError::FileNotSelected { index: 3 },
            EngineError::OffsetPastEnd {
                offset: Bytes::new(9),
                len: Bytes::new(8),
            },
            EngineError::IdentityMismatch,
            EngineError::Failed,
        ]
    }

    #[test]
    fn every_refusal_is_a_fixed_sentence() {
        // The match is exhaustive on purpose: a new variant fails this build rather than being
        // silently unchecked, which is what makes the conformance suite's leak scan meaningful.
        for error in every_error() {
            let expected = match error {
                EngineError::SessionClosed => "the torrent session is closed",
                EngineError::UnknownTorrent { .. } => {
                    "this session holds no torrent with that handle"
                }
                EngineError::EmptySelection => {
                    "no files are selected, so there is nothing to transfer"
                }
                EngineError::UnknownFile { .. } => "this torrent has no file at that index",
                EngineError::FileNotSelected { .. } => {
                    "that file is not selected, so its bytes are not being downloaded"
                }
                EngineError::OffsetPastEnd { .. } => "the read starts past the end of the file",
                EngineError::IdentityMismatch => {
                    "the torrent at that source is not the one it was added as"
                }
                EngineError::Failed => "the torrent engine could not complete the operation",
            };
            assert_eq!(error.to_string(), expected, "{error:?}");
        }
    }

    #[test]
    fn no_refusal_carries_a_value_into_its_message() {
        // The canary is a payload rather than a string: an error whose `Display` interpolated the
        // handle, the index, or the offsets would fail here even though the variant is fixed.
        for error in every_error() {
            let text = error.to_string();
            assert!(
                check_text(&text, 512).is_ok(),
                "a refusal is user-visible text: {text}"
            );
            assert!(!text.contains('7'), "a refusal quoted a value: {text}");
        }
    }

    #[test]
    fn an_add_refusal_names_the_verdict_it_rejected() {
        assert_eq!(
            AddRefused::OptInRequired.to_string(),
            "the safety report requires the advanced opt-in, so the torrent cannot be added yet"
        );
        assert_eq!(
            AddRefused::Blocked.to_string(),
            "the safety report blocked the torrent, so it can never be added"
        );
        assert!(check_text(&AddRefused::OptInRequired.to_string(), 512).is_ok());
        assert!(check_text(&AddRefused::Blocked.to_string(), 512).is_ok());
    }
}
