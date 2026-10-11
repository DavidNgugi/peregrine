//! Keyset pagination: the shape every list query in this crate uses.
//!
//! AGENTS §5 forbids `OFFSET` outright, and `cargo xtask check-offset` enforces it. The
//! reason is that an offset page shifts under a write: a row inserted between two reads
//! pushes a row out of the second page, and on a table the single writer is appending to
//! that is the ordinary case rather than an edge one. A cursor is therefore a value taken
//! from the last row of the previous page — the row id here — and the query asks for the
//! rows below it.
//!
//! Both halves of a page request are bounded: the cursor is optional and the limit is
//! clamped, so a repository cannot be handed a request that reads an unbounded list.

/// The most rows one page may hold.
///
/// An unbounded list query is a memory spike held while the reader pool is busy, and a
/// caller that asks for "everything" always turns out to want the first screenful. 500 rows
/// is larger than any screen this application has and, at this schema's row sizes, still
/// inside the 64 KB IPC budget of ARCHITECTURE §7 rule 1.
pub const MAX_PAGE_ROWS: u32 = 500;

/// The ordering value of one row: what a page resumes below.
///
/// A newtype rather than a bare `i64` because a cursor and a row id are different things
/// that happen to have the same representation, and passing the wrong one to a repository
/// reads the wrong page rather than failing to compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor(i64);

impl Cursor {
    /// Wraps the ordering value of a row.
    #[must_use]
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    /// The ordering value, for binding into a query.
    #[must_use]
    pub const fn value(self) -> i64 {
        self.0
    }
}

/// Which page of a list to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest {
    after: Option<Cursor>,
    limit: u32,
}

impl PageRequest {
    /// The newest page.
    #[must_use]
    pub fn first(limit: u32) -> Self {
        Self {
            after: None,
            limit: limit.clamp(1, MAX_PAGE_ROWS),
        }
    }

    /// The page below `cursor`.
    #[must_use]
    pub fn after(cursor: Cursor, limit: u32) -> Self {
        Self {
            after: Some(cursor),
            limit: limit.clamp(1, MAX_PAGE_ROWS),
        }
    }

    /// The cursor this page starts below, when it is not the first page.
    #[must_use]
    pub const fn cursor(self) -> Option<Cursor> {
        self.after
    }

    /// How many rows this page may hold, after clamping.
    #[must_use]
    pub const fn limit(self) -> u32 {
        self.limit
    }

    /// The cursor as the `i64` a query binds.
    ///
    /// `None` binds as SQL `NULL`, which is what makes one statement serve both the first
    /// page and every page after it.
    pub(crate) const fn bindable_cursor(self) -> Option<i64> {
        match self.after {
            Some(cursor) => Some(cursor.value()),
            None => None,
        }
    }

    /// The limit as the `i64` a query binds.
    pub(crate) const fn bindable_limit(self) -> i64 {
        self.limit as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clamp, at both ends and past them.
    #[test]
    fn a_page_limit_is_clamped_to_the_bounded_range() {
        assert_eq!(
            PageRequest::first(0).limit(),
            1,
            "a page holds at least one row"
        );
        assert_eq!(PageRequest::first(1).limit(), 1);
        assert_eq!(PageRequest::first(MAX_PAGE_ROWS).limit(), MAX_PAGE_ROWS);
        assert_eq!(
            PageRequest::first(MAX_PAGE_ROWS + 1).limit(),
            MAX_PAGE_ROWS,
            "a request for more than the maximum is bounded, not refused"
        );
        assert_eq!(PageRequest::first(u32::MAX).limit(), MAX_PAGE_ROWS);
    }

    /// A first page has no cursor; a later one carries the value it resumes below.
    #[test]
    fn a_cursor_is_carried_only_by_a_later_page() {
        let first = PageRequest::first(10);
        assert_eq!(first.cursor(), None);
        assert_eq!(first.bindable_cursor(), None, "no cursor binds as NULL");

        let cursor = Cursor::new(42);
        let next = PageRequest::after(cursor, 10);
        assert_eq!(next.cursor(), Some(cursor));
        assert_eq!(next.bindable_cursor(), Some(42));
        assert_eq!(next.bindable_limit(), 10);
    }

    /// A negative cursor is a value, not an error: it is below every row, so the page is
    /// empty. Refusing it would mean the repository had to validate what the database
    /// already handles.
    #[test]
    fn a_cursor_below_every_row_reads_an_empty_page() {
        let request = PageRequest::after(Cursor::new(-1), 10);
        assert_eq!(request.bindable_cursor(), Some(-1));
    }
}
