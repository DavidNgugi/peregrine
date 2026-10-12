//! The production identifier generator.
//!
//! `peregrine-domain` defines [`IdGenerator`] and forbids itself from touching a random
//! source, so the one place in shipping code that generates an identifier is here, behind
//! the port. It sits beside [`SystemClock`](crate::SystemClock) because both are the
//! machine-dependent half of the same rule: everything else takes a `dyn IdGenerator` and
//! is therefore predictable in a test.

use peregrine_domain::IdGenerator;
use ulid::Ulid;

/// Issues ULIDs from the process's random source and the system's wall clock.
///
/// A ULID's leading bits are its timestamp, so generated identifiers sort by issue order
/// to millisecond resolution. Anything that needs a strict ordering must not rely on that
/// resolution: two identifiers minted in the same millisecond order arbitrarily.
#[derive(Debug, Default, Clone, Copy)]
pub struct UlidGenerator;

impl IdGenerator for UlidGenerator {
    fn next_id(&self) -> Ulid {
        Ulid::generate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two draws must differ. The generator is called by the scheduler for every
    /// submission, and a repeated identifier would collide in the dedupe index.
    #[test]
    fn generated_identifiers_differ() {
        let generator = UlidGenerator;
        let first = generator.next_id();
        let second = generator.next_id();
        assert_ne!(first, second);
    }
}
