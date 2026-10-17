//! The operating system trash, through the crate that implements it per platform.
//!
//! AGENTS R22 makes deletion reversible where the OS can hold it, and this is that hold. The
//! adapter is deliberately thin: the platform differences (Finder on macOS, the XDG trash spec on
//! Linux, SHFileOperation on Windows) are a solved problem with a maintained implementation, and a
//! second one here would be a second thing to keep correct. What this adds is the failure shape
//! the domain port promises: a refusal leaves the file exactly where it was.

use std::path::Path;

use peregrine_domain::{Trash, TrashError};

/// The shipping [`Trash`]: the platform own trash, through the `trash` crate.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemTrash;

impl SystemTrash {
    /// The adapter, which holds no state.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Trash for SystemTrash {
    fn move_to_trash(&self, path: &Path) -> Result<(), TrashError> {
        // `trash::delete` moves exactly the path it is given and never removes it in place, so
        // an error here is a file that is still there. The crate error is folded to a fixed
        // sentence because it can quote the platform reason but must not name a path (AGENTS R21).
        trash::delete(path).map_err(|_| TrashError::Refused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The acceptance is that the file is gone from where it was and its name is no longer in the
    /// directory: what the OS does with it after that is the OS own answer, and asserting a
    /// platform-specific trash layout is how a test breaks on a machine it was not written on.
    #[test]
    fn a_file_moved_to_the_trash_leaves_its_directory() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let file = directory.path().join("peregrine-trash-test.bin");
        std::fs::write(&file, b"bytes").expect("the fixture is written");
        SystemTrash::new()
            .move_to_trash(&file)
            .expect("the trash takes it");
        assert!(!file.exists(), "the file left its old path");
        assert!(
            std::fs::read_dir(directory.path())
                .expect("the directory is readable")
                .next()
                .is_none(),
            "nothing with that name is left behind"
        );
    }
}
