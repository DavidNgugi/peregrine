//! Staged verification of a sidecar binary, and the atomic install that follows it.
//!
//! A sidecar is an executable we hand the user's media to, so the check is the
//! difference between a media player and a remote-code-execution channel. The stages
//! are [adr/0022](../../../../../docs/adr/0022-component-packs-and-sidecar-verification.md)
//! §3's, in order, and each refusal names its stage.
//!
//! [`VerifiedSidecar`] is the capability token this module produces and the only
//! thing the spawn path may accept. It is deliberately not `Deserialize`, not
//! `Clone`, and has no public constructor, mirroring `SafePath` in
//! `peregrine-domain`: a path that skipped the check is not safe, and the only way
//! to hold a token is to do the check.
//!
//! The detached minisign signature of §2 is **not** verified here. The scheme needs a
//! dependency decision the repository has not made, and no per-OS artefact pin (URL,
//! exact size, SHA-256, signature) exists yet, so an implementation would have
//! nothing to check against. The stages that can be built honestly are built here.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256 as Sha256Hasher};
use thiserror::Error;

/// What a component must be, byte for byte, before it may run.
///
/// Produced by the table `sidecars.toml` is compiled into ([adr/0022](../../../../docs/adr/0022-component-packs-and-sidecar-verification.md)
/// §1); a manifest fetched at runtime is the trust that record rejects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Artifact {
    /// The exact byte length of the artefact.
    pub bytes: u64,
    /// The SHA-256 of the whole artefact.
    pub sha256: Sha256,
}

/// A SHA-256 digest.
///
/// The bytes are the digest, not its hex spelling, so a malformed pin cannot
/// survive to a comparison and the hex encoder lives in one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sha256([u8; 32]);

impl Sha256 {
    /// The digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The digest of a computed hash, for the verifier and its fixtures.
    ///
    /// Crate-visible rather than public: a caller outside this crate gets a digest
    /// from the manifest's hex pin, which is the only spelling a pin may use.
    pub(crate) const fn from_digest(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl fmt::Display for Sha256 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// The pin is not a SHA-256.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("a SHA-256 pin must be 64 hexadecimal characters")]
pub struct InvalidSha256;

impl TryFrom<&str> for Sha256 {
    type Error = InvalidSha256;

    fn try_from(hex: &str) -> Result<Self, Self::Error> {
        // 32 bytes is exactly 64 hex characters, so the length test rejects an
        // odd-length string's trailing nibble without a special case for it.
        if hex.len() != 64 {
            return Err(InvalidSha256);
        }
        let mut digits = [0_u8; 64];
        for (index, byte) in hex.bytes().enumerate() {
            digits[index] = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => return Err(InvalidSha256),
            };
        }
        // The array is exactly 32 pairs, so the whole of `digits` is consumed and no
        // bounds check is needed to index it.
        let mut digest = [0_u8; 32];
        for (byte, pair) in digest.iter_mut().zip(digits.as_chunks::<2>().0) {
            *byte = (pair[0] << 4) | pair[1];
        }
        Ok(Self(digest))
    }
}

/// Why a candidate could not become a [`VerifiedSidecar`].
///
/// One variant per staged step, so the refused step is named rather than
/// summarised.
#[derive(Debug, Error)]
pub enum VerifyError {
    /// Nothing is at the path, or the path cannot be examined.
    #[error("`{path}` is not installed: {source}")]
    Missing {
        /// The path that was examined.
        path: PathBuf,
        /// The OS error, so a permission problem is not read as an absence.
        source: io::Error,
    },
    /// Something is at the path but it is not a regular file.
    #[error("`{path}` is not a regular file")]
    NotRegular {
        /// The path that was examined.
        path: PathBuf,
    },
    /// The byte length is not the pinned one.
    #[error("`{path}` is {actual} bytes, but the pin declares {expected}")]
    Size {
        /// The path that was examined.
        path: PathBuf,
        /// The pinned length.
        expected: u64,
        /// The length on disk.
        actual: u64,
    },
    /// The SHA-256 is not the pinned one: a corrupted, truncated, or substituted
    /// file.
    #[error("`{path}` has SHA-256 {actual}, but the pin declares {expected}")]
    Hash {
        /// The path that was examined.
        path: PathBuf,
        /// The pinned digest.
        expected: Sha256,
        /// The digest of the file on disk.
        actual: Sha256,
    },
    /// The file changed while it was being hashed, so no one version of it was ever
    /// verified.
    #[error(
        "`{path}` changed while it was verified: {actual} bytes were read, the pin declares {expected}"
    )]
    ChangedWhileHashing {
        /// The path that was examined.
        path: PathBuf,
        /// The pinned length.
        expected: u64,
        /// The number of bytes that were actually hashed.
        actual: u64,
    },
    /// The file could not be read far enough to hash it.
    #[error("`{path}` could not be read: {source}")]
    Read {
        /// The path that was read.
        path: PathBuf,
        /// The OS error.
        source: io::Error,
    },
}

/// A path that exists, is a regular file, and hashed to its pinned digest.
///
/// This is the only token the spawn path may accept, and it cannot be built
/// without the check: the fields are private and there is no `From`, `Deserialize`,
/// `Default`, or `Clone` implementation. `Clone` is withheld deliberately, because a
/// copy would outlive the moment of verification with nothing to say when it stopped
/// being true; a caller that needs the same components in two shapes borrows the one
/// token, as [`crate::sidecar::CapabilityReport`] does.
///
/// Compare `SafePath`, which is this idea for a path the guard proved contained.
#[derive(Debug)]
pub struct VerifiedSidecar {
    path: PathBuf,
    sha256: Sha256,
}

impl VerifiedSidecar {
    /// Runs the staged verification in [adr/0022](../../../../docs/adr/0022-component-packs-and-sidecar-verification.md)
    /// §3 order against `artifact`.
    ///
    /// Blocking: hashing a 40–60 MB pack is IO, so an async caller runs this on a
    /// blocking task rather than on a runtime worker thread. Re-hashing a file that
    /// has not changed is the §5 optimisation and is not part of this step.
    pub fn verify(path: impl AsRef<Path>, artifact: &Artifact) -> Result<Self, VerifyError> {
        Self::verify_observing(path.as_ref(), artifact, &mut |_| {})
    }

    /// The path that was verified.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The digest the file was verified against.
    #[must_use]
    pub const fn sha256(&self) -> Sha256 {
        self.sha256
    }

    /// [`Self::verify`], with a hook that runs after the size check and before the
    /// first byte is hashed.
    ///
    /// The hook exists so the window between those two steps can be exercised
    /// deterministically: a file that changes there is the TOCTOU case the
    /// size-and-digest pair has to catch, and without a seam the race would need
    /// threads and wall-clock luck (AGENTS R14).
    fn verify_observing(
        path: &Path,
        artifact: &Artifact,
        hook: &mut dyn FnMut(&Path),
    ) -> Result<Self, VerifyError> {
        // Step 1: presence and file type. `symlink_metadata` deliberately does not
        // follow a link, and a directory, socket, or device that carries the
        // expected name is refused rather than opened.
        let metadata = fs::symlink_metadata(path).map_err(|source| VerifyError::Missing {
            path: path.to_owned(),
            source,
        })?;
        if !metadata.is_file() {
            return Err(VerifyError::NotRegular {
                path: path.to_owned(),
            });
        }

        // Step 2: reject on the byte length before reading, so a multi-gigabyte
        // impostor is refused without hashing it. The length is checked a second
        // time below, because this one describes the file as it was and not as it
        // is read.
        let on_disk = metadata.len();
        if on_disk != artifact.bytes {
            return Err(VerifyError::Size {
                path: path.to_owned(),
                expected: artifact.bytes,
                actual: on_disk,
            });
        }

        hook(path);

        // Steps 3 and 4: hash the bytes that are actually read and count them in
        // the same pass. A second, separate read would let a file whose length
        // changed be judged by a stale size against a fresh digest — a mixture of
        // two versions that matches neither.
        let file = File::open(path).map_err(|source| VerifyError::Read {
            path: path.to_owned(),
            source,
        })?;
        let digest = digest_of(file, path)?;
        if digest.bytes != artifact.bytes {
            return Err(VerifyError::ChangedWhileHashing {
                path: path.to_owned(),
                expected: artifact.bytes,
                actual: digest.bytes,
            });
        }
        if digest.sha256 != artifact.sha256 {
            return Err(VerifyError::Hash {
                path: path.to_owned(),
                expected: artifact.sha256,
                actual: digest.sha256,
            });
        }

        Ok(Self {
            path: path.to_owned(),
            sha256: digest.sha256,
        })
    }
}

/// What a verified artefact becomes once it is moved into its final location.
///
/// The final name is a parameter because whether a component is installed as `mpv`
/// or `ffmpeg-7.1` is a packaging decision, and the version is the same one the
/// manifest pins.
pub fn install(
    verified: VerifiedSidecar,
    final_path: impl AsRef<Path>,
) -> Result<VerifiedSidecar, InstallError> {
    let final_path = final_path.as_ref();
    // Taken apart so the token is consumed: a verification that has been installed
    // is the one at the new path, and there is no stale token left behind pointing
    // at the temporary name.
    let VerifiedSidecar {
        path: staged,
        sha256,
    } = verified;

    // A rename inside one filesystem is atomic, so the final name never exists in a
    // half-written state. Across filesystems it is not, and the copy `rename` would
    // otherwise degrade into is refused: a partially visible artefact is exactly
    // what staging exists to prevent.
    fs::rename(&staged, final_path).map_err(|source| InstallError {
        from: staged,
        to: final_path.to_owned(),
        source,
    })?;

    // The bytes at `final_path` are the ones that were hashed, because the rename
    // moved that inode. The token follows the file instead of being re-derived.
    Ok(VerifiedSidecar {
        path: final_path.to_owned(),
        sha256,
    })
}

/// Why a verified artefact could not be installed, and what the caller can do.
#[derive(Debug, Error)]
#[error("`{from}` could not be moved to `{to}`: {source}")]
pub struct InstallError {
    /// The verified temporary path.
    pub from: PathBuf,
    /// The requested final path.
    pub to: PathBuf,
    /// The OS error, classified by the predicates below.
    pub source: io::Error,
}

impl InstallError {
    /// Whether the move crossed a filesystem, which this step refuses rather than
    /// copying.
    #[must_use]
    pub fn is_cross_device(&self) -> bool {
        self.source.kind() == io::ErrorKind::CrossesDevices
    }

    /// Whether the caller should tell the user to retry, because the target is in
    /// use or its directory is not writable.
    ///
    /// On Windows an executable that is running cannot be renamed over
    /// ([adr/0022](../../../../docs/adr/0022-component-packs-and-sidecar-verification.md)
    /// §Consequences — a platform behaviour, not a bug), so replacing a sidecar mid
    /// session fails cleanly and succeeds after the process holding it exits. On
    /// Unix the same error is what a read-only component directory produces.
    /// Reverting to the previous version is correct in both cases: it verified once.
    #[must_use]
    pub fn is_target_unreplaceable(&self) -> bool {
        matches!(
            self.source.kind(),
            io::ErrorKind::ExecutableFileBusy | io::ErrorKind::PermissionDenied
        )
    }
}

/// What one pass of the hashing loop read.
struct Computed {
    bytes: u64,
    sha256: Sha256,
}

fn digest_of(mut file: File, path: &Path) -> Result<Computed, VerifyError> {
    let mut hasher = Sha256Hasher::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut bytes = 0_u64;
    loop {
        let read = file.read(&mut buffer).map_err(|source| VerifyError::Read {
            path: path.to_owned(),
            source,
        })?;
        if read == 0 {
            break;
        }
        hasher.update(buffer.get(..read).unwrap_or(&buffer));
        // A single `read` cannot return more than the 64 KiB buffer holds, so the
        // conversion is lossless; the fallback keeps the addition panic-free rather
        // than saturating silently on a platform where that ever changed.
        bytes += u64::try_from(read).unwrap_or(u64::MAX);
    }
    Ok(Computed {
        bytes,
        sha256: Sha256::from_digest(hasher.finalize().into()),
    })
}

#[cfg(test)]
mod tests;
