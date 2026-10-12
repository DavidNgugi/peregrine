//! The verifier's refusal paths — one test per rule — and the install step that
//! follows a successful verification, including the two refusals the ADR names.
//!
//! Every fixture is a file this test writes into a temporary directory, so the suite
//! is offline and deterministic. The `PATH` proof, which belongs to resolution rather
//! than to verification, lives in the parent module's tests.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::*;
use crate::sidecar::test_support::{
    artifact_for, digest_of_bytes, fixture, mpv_bytes, sources, verified_token,
};

/// The failed move inside a refusal, or a panic.
///
/// These tests are about the rename; a `NotAName` here would mean the destination helper
/// rejected a plain name, which is a different rule with its own test below.
fn moved(refusal: InstallRefusal) -> InstallError {
    match refusal {
        InstallRefusal::Move(inner) => inner,
        // Named rather than a wildcard: a variant added later should fail to compile here,
        // not be swallowed by an arm whose message would then be wrong.
        InstallRefusal::NotAName { name } => {
            panic!("expected a failed move, got a refused name: `{name}`")
        }
    }
}

/// A name that is not a plain file name is refused, and nothing lands outside the managed
/// directory.
///
/// This is the containment control: `install` builds the destination from a name inside
/// `sources.managed`, so a name carrying a separator is refused rather than resolved. The
/// refusal is also a **different variant** from a failed move, and that distinction is the
/// reason this test asserts the variant rather than just the error —
/// `InstallError::is_target_unreplaceable` classifies a `PermissionDenied` as retryable,
/// which is sound advice for a component directory that is momentarily read-only and the
/// worst possible advice for a name that tried to leave it.
#[test]
fn a_name_that_is_not_a_plain_name_is_refused() {
    // The managed directory is nested inside a directory this test owns, so the traversal
    // assertion lands somewhere the test controls. Pointing it at the shared temporary
    // directory instead made the test depend on what an earlier run had left there — it
    // passed alone and failed in the suite, which is the whole flakiness this suite is
    // supposed to be free of.
    let outer = TempDir::new().expect("a temporary directory");
    let managed = outer.path().join("managed");
    fs::create_dir(&managed).expect("the fixture directory is writable");

    for name in [
        "",
        ".",
        "..",
        "../escaped",
        "sub/ffmpeg",
        "..\\escaped",
        "ff\0mpeg",
    ] {
        let token = verified_token(&managed, mpv_bytes());
        let refused = install(token, &sources(&managed), name)
            .expect_err("a name that is not a plain name cannot name a component");
        assert!(
            matches!(&refused, InstallRefusal::NotAName { name: refused_name } if refused_name == name),
            "`{name}` should be refused as a name, got {refused:?}"
        );
    }

    assert!(
        !outer.path().join("escaped").exists(),
        "a traversal attempt must not have created anything outside the managed directory"
    );
}

/// `set_len` mutates the open file rather than replacing the directory entry, so the
/// hook in the change-under-verification test leaves exactly the file the verifier is
/// about to read.
fn truncate_in_place(path: &Path, bytes: u64) {
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("the fixture is writable");
    file.set_len(bytes).expect("the fixture is truncatable");
}

#[test]
fn a_verified_sidecar_is_accepted_and_reports_its_path_and_digest() {
    let root = TempDir::new().expect("a temporary directory");
    let bytes = b"the real thing";
    let artifact = artifact_for(bytes);
    let path = fixture(root.path(), "mpv", bytes);

    let verified = VerifiedSidecar::verify(&path, &artifact).expect("the file matches its pin");

    assert_eq!(verified.path(), path);
    assert_eq!(verified.sha256(), artifact.sha256);
}

#[test]
fn an_absent_file_is_refused_as_missing() {
    let root = TempDir::new().expect("a temporary directory");
    let path = root.path().join("mpv");

    let refused = VerifiedSidecar::verify(&path, &artifact_for(mpv_bytes()))
        .expect_err("an absent file has no digest");

    assert!(
        matches!(refused, VerifyError::Missing { path: ref at, .. } if *at == path),
        "{refused}"
    );
}

#[test]
fn a_directory_where_a_file_is_expected_is_refused_as_not_regular() {
    let root = TempDir::new().expect("a temporary directory");
    let path = root.path().join("mpv");
    fs::create_dir(&path).expect("the fixture directory is writable");

    let refused = VerifiedSidecar::verify(&path, &artifact_for(mpv_bytes()))
        .expect_err("a directory is not an executable");

    assert!(
        matches!(refused, VerifyError::NotRegular { path: ref at } if *at == path),
        "{refused}"
    );
}

#[test]
fn a_symlink_is_refused_rather_than_followed() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().expect("a temporary directory");
    let real = fixture(root.path(), "real-mpv", mpv_bytes());
    let link = root.path().join("mpv");
    symlink(&real, &link).expect("the fixture directory is writable");

    let refused = VerifiedSidecar::verify(&link, &artifact_for(mpv_bytes()))
        .expect_err("a link is not the artefact it points at");

    assert!(
        matches!(refused, VerifyError::NotRegular { path: ref at } if *at == link),
        "{refused}"
    );
}

#[test]
fn a_size_that_differs_is_refused_before_hashing() {
    let root = TempDir::new().expect("a temporary directory");
    let path = fixture(root.path(), "mpv", b"short");
    let artifact = artifact_for(b"a much longer artefact");

    let refused = VerifiedSidecar::verify(&path, &artifact).expect_err("the lengths differ");

    assert!(
        matches!(
            refused,
            VerifyError::Size {
                expected,
                actual,
                ..
            } if expected == artifact.bytes && actual == 5
        ),
        "{refused}"
    );
}

#[test]
fn a_size_at_the_pin_is_accepted_and_the_hash_decides() {
    let root = TempDir::new().expect("a temporary directory");
    let path = fixture(root.path(), "mpv", b"abcd");
    let artifact = artifact_for(b"abce");

    let refused =
        VerifiedSidecar::verify(&path, &artifact).expect_err("four bytes of the wrong bytes");

    assert!(
        matches!(refused, VerifyError::Hash { .. }),
        "a matching length must not skip the digest: {refused}"
    );
}

#[test]
fn a_hash_that_differs_is_refused_and_both_digests_are_named() {
    let root = TempDir::new().expect("a temporary directory");
    // Same length, one byte different: the hash step must be what refuses it.
    let path = fixture(root.path(), "mpv", b"tampered");
    let artifact = artifact_for(b"genuine.");

    let refused = VerifiedSidecar::verify(&path, &artifact).expect_err("the digests differ");

    match refused {
        VerifyError::Hash {
            expected, actual, ..
        } => {
            assert_eq!(expected, artifact.sha256);
            assert_eq!(actual, digest_of_bytes(b"tampered"));
            assert_ne!(actual, expected);
        }
        other => panic!("expected a hash refusal, got {other}"),
    }
}

/// The window between the size check and the hash is the one a concurrent
/// writer can exploit. The hook rewrites the fixture in exactly that window,
/// deterministically, with no threads and no sleeping.

#[test]
fn a_file_whose_length_changes_between_the_size_check_and_the_hash_is_refused() {
    let root = TempDir::new().expect("a temporary directory");
    let bytes = b"the artefact as it was pinned";
    let path = fixture(root.path(), "mpv", bytes);
    let artifact = artifact_for(bytes);
    let mut truncated = false;

    let refused = VerifiedSidecar::verify_observing(&path, &artifact, &mut |path| {
        truncate_in_place(path, 4);
        truncated = true;
    })
    .expect_err("the bytes that were hashed are not the bytes that were sized");

    assert!(truncated, "the hook must have run between the two steps");
    assert!(
        matches!(refused, VerifyError::ChangedWhileHashing { actual: 4, .. }),
        "{refused}"
    );
}

#[test]
fn installing_moves_the_verified_file_and_the_token_follows_it() {
    let root = TempDir::new().expect("a temporary directory");
    let bytes = b"a verified artefact";
    let staged = fixture(root.path(), "ffmpeg.part", bytes);
    let token =
        VerifiedSidecar::verify(&staged, &artifact_for(bytes)).expect("the fixture matches");

    let final_path = root.path().join("ffmpeg");
    let installed = install(token, &sources(root.path()), "ffmpeg")
        .expect("same filesystem, no target in the way");

    assert_eq!(installed.path(), final_path);
    assert_eq!(installed.sha256(), digest_of_bytes(bytes));
    assert_eq!(
        fs::read(&final_path).expect("the artefact landed"),
        bytes,
        "the installed bytes are the verified bytes"
    );
    assert!(
        !staged.exists(),
        "the temporary name is gone rather than left behind"
    );
}

#[test]
fn installing_refuses_a_target_this_process_may_not_replace() {
    let root = TempDir::new().expect("a temporary directory");
    let bytes = b"a verified artefact";
    let staged = fixture(root.path(), "ffmpeg.part", bytes);
    let token =
        VerifiedSidecar::verify(&staged, &artifact_for(bytes)).expect("the fixture matches");

    // A directory carrying the target name is the portable stand-in for "the
    // OS will not replace this": renaming a file onto one fails on every
    // platform. The Windows case the ADR names — a running executable cannot be
    // renamed over — is not expressible in a test binary that is itself the
    // running executable, so this exercises the typed refusal and
    // `is_target_unreplaceable` carries the platform classification.
    let occupied = root.path().join("ffmpeg");
    fs::create_dir(&occupied).expect("the fixture directory is writable");
    fixture(&occupied, "in-use", b"something is here");

    let refused = moved(
        install(token, &sources(root.path()), "ffmpeg").expect_err("the target is not replaceable"),
    );

    assert_eq!(refused.from, staged);
    assert_eq!(refused.to, occupied);
    assert!(
        !refused.is_cross_device(),
        "a same-filesystem refusal must not be reported as a cross-device move"
    );
    assert!(
        staged.exists(),
        "a refused install leaves the verified temporary file for the caller to retry"
    );
    assert!(occupied.is_dir(), "the target is untouched");
}

#[test]
fn installing_into_a_directory_this_process_may_not_write_is_reported_as_retryable() {
    let root = TempDir::new().expect("a temporary directory");
    let bytes = b"a verified artefact";
    let staged = fixture(root.path(), "ffmpeg.part", bytes);
    let token =
        VerifiedSidecar::verify(&staged, &artifact_for(bytes)).expect("the fixture matches");

    let locked = root.path().join("component-directory");
    fs::create_dir(&locked).expect("the fixture directory is writable");
    make_read_only(&locked);

    // Root and a capability-carrying process ignore the directory's write bit, so
    // a write that succeeds is this host saying it cannot express the condition.
    // The probe is the write itself rather than a guess about who we are.
    if fs::write(locked.join(".write-probe"), b"").is_ok() {
        println!(
            "this host does not enforce the directory write bit; the retryable path is untested"
        );
        return;
    }

    let refused = moved(
        install(token, &sources(&locked), "ffmpeg")
            .expect_err("the directory cannot be written, so the name cannot be created"),
    );
    make_writable(&locked);

    assert!(
        refused.is_target_unreplaceable(),
        "{:?} should tell the caller to retry rather than to re-download",
        refused.source.kind()
    );
    assert!(
        staged.exists(),
        "the verified temporary file survives for the retry"
    );
}

#[cfg(unix)]
fn make_read_only(directory: &Path) {
    use std::os::unix::fs::PermissionsExt;

    // 0o555 removes write permission from the owner, and the rename that creates
    // an entry in the directory is a write on the directory itself.
    fs::set_permissions(directory, fs::Permissions::from_mode(0o555))
        .expect("the fixture is chmod-able");
}

#[cfg(not(unix))]
fn make_read_only(directory: &Path) {
    let _ = directory;
}

#[cfg(unix)]
fn make_writable(directory: &Path) {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(directory, fs::Permissions::from_mode(0o755))
        .expect("the fixture is chmod-able");
}

#[cfg(not(unix))]
fn make_writable(directory: &Path) {
    let _ = directory;
}

#[test]
fn a_cross_device_move_is_refused_rather_than_copied() {
    let Some(other) = directory_on_another_filesystem() else {
        // Honest skip: this host has a single filesystem, so the condition cannot
        // be created offline without mounting or formatting a volume (AGENTS
        // R14). `install` classifies the refusal from the OS error kind, and the
        // test below pins that classification.
        println!("no second filesystem on this host; the cross-device path is untested here");
        return;
    };

    let root = TempDir::new().expect("a temporary directory");
    let bytes = b"a verified artefact";
    let staged = fixture(root.path(), "ffmpeg.part", bytes);
    let token =
        VerifiedSidecar::verify(&staged, &artifact_for(bytes)).expect("the fixture matches");

    let destination = other.join("peregrine-cross-device-fixture");
    fs::create_dir_all(&destination).expect("the second filesystem is writable");
    let refused = moved(
        install(token, &sources(&destination), "ffmpeg")
            .expect_err("a cross-device move is refused"),
    );
    let _ = fs::remove_dir_all(&destination);

    assert!(
        refused.is_cross_device(),
        "{:?} should be reported as a cross-device move",
        refused.source.kind()
    );
    assert!(
        staged.exists(),
        "a refused move leaves the verified temporary file intact"
    );
}

/// A writable directory on a filesystem other than the test temporary
/// directory's, or `None` when the host has only one.
///
/// The candidates are the standard shared-memory and runtime mounts. A host that
/// has none of them gets `None` rather than a fake path, and the caller reports
/// the case instead of asserting something it cannot create offline.
#[cfg(unix)]
fn directory_on_another_filesystem() -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;

    let here = fs::metadata(env::temp_dir())
        .expect("the temporary directory exists")
        .dev();
    for candidate in ["/dev/shm", "/run", "/dev"] {
        let path = PathBuf::from(candidate);
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if metadata.is_dir() && metadata.dev() != here && probe_writable(&path) {
            return Some(path);
        }
    }
    None
}

/// A host with a single filesystem cannot express an atomic-move refusal.
#[cfg(not(unix))]
fn directory_on_another_filesystem() -> Option<PathBuf> {
    // Windows volumes are separate filesystems, but discovering one that is safe
    // to write to is a deployment question rather than a unit test's.
    None
}

#[cfg(unix)]
fn probe_writable(directory: &Path) -> bool {
    let probe = directory.join(".peregrine-write-probe");
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[test]
fn the_cross_device_and_busy_classifications_are_pinned_to_their_error_kinds() {
    let cross_device = InstallError {
        from: PathBuf::from("/tmp/ffmpeg.part"),
        to: PathBuf::from("/mnt/ffmpeg"),
        source: io::Error::from(io::ErrorKind::CrossesDevices),
    };
    assert!(cross_device.is_cross_device());
    assert!(!cross_device.is_target_unreplaceable());

    for kind in [
        io::ErrorKind::ExecutableFileBusy,
        io::ErrorKind::PermissionDenied,
    ] {
        let busy = InstallError {
            from: PathBuf::from("/tmp/ffmpeg.part"),
            to: PathBuf::from("/managed/ffmpeg"),
            source: io::Error::from(kind),
        };
        assert!(
            busy.is_target_unreplaceable(),
            "{kind:?} means the caller should retry after the holder exits"
        );
        assert!(!busy.is_cross_device());
    }
}

#[test]
fn a_sha256_pin_is_read_from_hex_in_either_case() {
    let lower = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
    let parsed = Sha256::try_from(lower).expect("64 lowercase hex characters");

    assert_eq!(parsed, digest_of_bytes(b"hello"), "a known SHA-256");
    assert_eq!(
        Sha256::try_from(lower.to_uppercase().as_str()).expect("64 uppercase hex characters"),
        parsed,
        "case is not part of the digest"
    );
    assert_eq!(parsed.to_string(), lower);
}

#[test]
fn a_pin_that_is_not_64_hex_characters_is_refused() {
    for bad in [
        "",
        "0",
        "00",
        "not-a-digest",
        // 64 characters, one of which is not hexadecimal.
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b982g",
        // 63 and 65 characters.
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b982",
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b98244",
    ] {
        assert_eq!(Sha256::try_from(bad), Err(InvalidSha256), "`{bad}`");
    }
}
