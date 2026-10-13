//! The library's services: the roots it scans, and the items it holds (PG-E14).
//!
//! Only root management is here so far. Matching, the unmatched queue, and the scan itself are
//! PG-E14-02 to PG-E14-09, and each lands in this module as it does.

pub mod index;
pub mod reconcile;
pub mod roots;
pub mod scan;

pub use index::{IndexError, IndexReport, MatchAs, Matched, index_candidates, match_unmatched};
pub use reconcile::Reconciliation;
pub use roots::{LibraryError, LibraryRootRow, LibraryService};
pub use scan::{CandidateFile, ScanError, ScanOutcome, Scanner, SkippedPath, UnreadableDirectory};
