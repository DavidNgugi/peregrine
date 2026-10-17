//! Deleting what a download wrote: the enumerated plan, and the port that carries it out.
//!
//! AGENTS R22 makes deletion a two-step act with a value in between: a plan enumerates exactly
//! what would go, the user confirms that list, and the act re-validates every path between the
//! plan and the move rather than trusting the list it was handed. This module holds both halves of
//! the vocabulary -- the plan and the port -- because a caller that could state one without the
//! other would be able to delete a path it never enumerated.
//!
//! The plan carries absolute paths rather than ids. What the user is agreeing to is a set of files
//! and a number of bytes, and an id the screen cannot read would make the confirmation an act of
//! faith. A path in a plan has already been through PathGuard; the act resolves it again anyway,
//! because the two moments are different and the filesystem can change between them.

use std::path::Path;

use thiserror::Error;

use crate::text::NonEmptyText;

/// One file a deletion would remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionEntry {
    /// The absolute path, as the plan enumerated it.
    pub path: NonEmptyText,
    /// The file's size in bytes, as the plan measured it.
    pub bytes: u64,
}

impl DeletionEntry {
    /// One file, from its path and measured size.
    #[must_use]
    pub const fn new(path: NonEmptyText, bytes: u64) -> Self {
        Self { path, bytes }
    }
}

/// Exactly what one destructive deletion will act on, and nothing else.
///
/// The total is computed from the entries rather than accepted from a caller, so a plan cannot
/// report a size its own list does not support -- the one number the confirmation screen shows
/// twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionPlan {
    /// The download job the plan belongs to.
    pub job_id: i64,
    /// The files, in the order the repository listed them.
    pub entries: Vec<DeletionEntry>,
    /// The sum of every entry's size.
    pub total_bytes: u64,
}

impl DeletionPlan {
    /// A plan over the entries a caller enumerated.
    #[must_use]
    pub fn new(job_id: i64, entries: Vec<DeletionEntry>) -> Self {
        let total_bytes = entries.iter().map(|entry| entry.bytes).sum();
        Self {
            job_id,
            entries,
            total_bytes,
        }
    }

    /// Whether the plan would remove nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many files the plan would remove.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Why a path could not be moved to the operating system's trash.
///
/// Two variants rather than one because a caller answers them differently: Refused is a file the
/// OS would not take and the user can retry, while Unavailable is a build with no trash adapter
/// and retrying will never help. Neither is a sentence that names a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TrashError {
    /// The operating system refused to move the file to its trash.
    #[error("the operating system refused to move the file to the trash")]
    Refused,
    /// This build has no way to reach the operating system's trash.
    #[error("this build has no trash it can use, so nothing was deleted")]
    Unavailable,
}

/// The operating system's trash, behind the boundary the domain may hold.
///
/// A synchronous method because moving a file into the trash is a short filesystem operation and
/// an adapter is not worth a future; the caller that owns an async runtime runs it on a blocking
/// pool.
pub trait Trash: Send + Sync {
    /// Moves one validated path to the trash, reversibly.
    ///
    /// # Errors
    ///
    /// [TrashError::Refused] when the OS would not take it, and [TrashError::Unavailable] when
    /// this build cannot reach a trash at all.
    fn move_to_trash(&self, path: &Path) -> Result<(), TrashError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> NonEmptyText {
        NonEmptyText::new(value).expect("a fixture path")
    }

    #[test]
    fn a_plan_totals_its_own_entries() {
        let plan = DeletionPlan::new(
            7,
            vec![
                DeletionEntry::new(text("/tmp/a.mkv"), 1_500),
                DeletionEntry::new(text("/tmp/b.nfo"), 500),
            ],
        );
        assert_eq!(plan.total_bytes, 2_000);
        assert_eq!(plan.len(), 2);
        assert!(!plan.is_empty());
    }

    #[test]
    fn an_empty_plan_is_empty_and_zeroed() {
        let plan = DeletionPlan::new(7, Vec::new());
        assert!(plan.is_empty());
        assert_eq!(plan.len(), 0);
        assert_eq!(plan.total_bytes, 0);
    }
}
