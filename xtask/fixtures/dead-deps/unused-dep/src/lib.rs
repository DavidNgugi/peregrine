//! A fixture whose manifest declares a dependency the sources never use.

/// Returns a constant, using none of the manifest's declared dependencies.
pub const fn answer() -> u32 {
    42
}
