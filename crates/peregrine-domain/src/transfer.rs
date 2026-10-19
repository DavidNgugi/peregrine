//! Protocol-neutral transfer units and file selections.
//!
//! These lived in `peregrine-torrent` while `BitTorrent` was the only engine. Usenet shares the
//! same byte counts, rates, and selection semantics, and a second copy of a newtype is a second
//! place for a unit to drift, so the units live in the domain and each engine re-exports them
//! under its own name.

/// A number of bytes: a transfer size, a file's length, or an offset into one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Bytes(u64);

impl Bytes {
    /// Wraps a byte count.
    #[must_use]
    pub const fn new(bytes: u64) -> Self {
        Self(bytes)
    }

    /// The count, for arithmetic and for a caller that has to spell it out.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A transfer rate in bytes per second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ByteRate(u64);

impl ByteRate {
    /// Wraps a rate of `bytes` per second.
    #[must_use]
    pub const fn per_second(bytes: u64) -> Self {
        Self(bytes)
    }

    /// The rate, in bytes per second.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Which files of a payload are being written.
///
/// A selection is a *set*: the constructor sorts and de-duplicates, so two spellings of one
/// selection compare equal and a caller can compare a live selection with a requested one. It is a
/// set of indices into the metadata's file list rather than a set of paths, because a path in
/// untrusted metadata and only the engine knows what an index names.
///
/// It may be empty, and an empty one is refused by an engine's add and select: a payload with
/// nothing selected writes nothing, which is a removal, and a transfer that never moves is the
/// failure a user cannot diagnose.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_and_rates_carry_their_unit() {
        assert_eq!(Bytes::new(1_500).get(), 1_500);
        assert_eq!(ByteRate::per_second(0).get(), 0);
        assert_eq!(Bytes::default(), Bytes::new(0));
        assert!(Bytes::new(1) < Bytes::new(2), "a byte count is ordered");
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
        let selection = FileSelection::empty();

        assert!(selection.is_empty());
        assert_eq!(selection.len(), 0);
        assert_eq!(selection.indices(), &[] as &[usize]);
        assert!(!selection.contains(0));
        assert_eq!(selection, FileSelection::new(Vec::new()));
    }
}
