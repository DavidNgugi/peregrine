//! Fixtures shared by the sidecar tests.
//!
//! Every one of them writes into a temporary directory the caller owns, so the
//! suite never reads the machine's real components and never reaches the network
//! (AGENTS R14).

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256 as Sha256Hasher};

use crate::sidecar::verify::{Artifact, Sha256, VerifiedSidecar};

/// The bytes the `mpv` fixtures stand in for.
pub fn mpv_bytes() -> &'static [u8] {
    b"mpv-fixture-binary"
}

pub fn digest_of_bytes(bytes: &[u8]) -> Sha256 {
    let mut hasher = Sha256Hasher::new();
    hasher.update(bytes);
    Sha256::from_digest(hasher.finalize().into())
}

pub fn artifact_for(bytes: &[u8]) -> Artifact {
    Artifact {
        bytes: u64::try_from(bytes.len()).expect("a fixture is smaller than 2^64 bytes"),
        sha256: digest_of_bytes(bytes),
    }
}

/// Writes `bytes` to `directory/name` and returns the file's path.
pub fn fixture(directory: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = directory.join(name);
    fs::write(&path, bytes).expect("the fixture directory is writable");
    path
}

/// A verified token for a file that really was hashed, since no other constructor
/// exists.
pub fn verified_token(root: &Path, bytes: &[u8]) -> VerifiedSidecar {
    let staged = fixture(root, "staged-fixture", bytes);
    VerifiedSidecar::verify(&staged, &artifact_for(bytes)).expect("the fixture matches its pin")
}

/// A [`Sources`](crate::sidecar::Sources) whose managed directory is `directory`.
///
/// `install` only ever consults `managed`, so the beside-app path points at the same place
/// rather than inventing a second location the test does not mean. The resolution tests build
/// their own two-location sources.
pub fn sources(directory: &Path) -> crate::sidecar::Sources {
    crate::sidecar::Sources {
        managed: directory.to_path_buf(),
        beside_app: directory.to_path_buf(),
    }
}
