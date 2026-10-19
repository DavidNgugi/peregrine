//! Resolver order, the `PATH` proof, and the capability mapping.
//!
//! Every fixture is a file this test writes into a temporary directory, so the suite
//! is offline and deterministic.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

use super::test_support::{artifact_for, digest_of_bytes, fixture, mpv_bytes, verified_token};
use super::*;

/// The marker a child test process sets so it runs the `PATH` proof rather than its
/// parent's whole suite.
const PATH_PROOF_ENV: &str = "PEREGRINE_PLATFORM_PATH_PROOF";

#[test]
fn a_resolve_error_is_returned_rather_than_none_when_nothing_verifies() {
    let root = TempDir::new().expect("a temporary directory");
    let sources = Sources {
        managed: root.path().join("managed"),
        beside_app: root.path().join("beside"),
    };

    let refused = resolve(Component::Mpv, None, &sources, &artifact_for(mpv_bytes()))
        .expect_err("neither location holds the component");

    match refused {
        ResolveError::NotResolved {
            component,
            tried,
            source,
        } => {
            assert_eq!(component, Component::Mpv);
            assert_eq!(tried, 2, "managed and beside-app are always tried");
            assert!(matches!(*source, VerifyError::Missing { .. }));
        }
        other => panic!("expected a typed absence, got {other}"),
    }
}

#[test]
fn the_resolver_takes_the_override_first() {
    let root = TempDir::new().expect("a temporary directory");
    let override_directory = root.path().join("override");
    let managed = root.path().join("managed");
    fs::create_dir_all(&override_directory).expect("the fixture directory is writable");
    fs::create_dir_all(&managed).expect("the fixture directory is writable");

    // The override is deliberately a different artefact from the managed copy,
    // so which one won is visible in the digest rather than in a path we set.
    let override_bytes = b"the user's own build";
    let managed_bytes = b"the build we shipped";
    let override_path = fixture(&override_directory, "mpv", override_bytes);
    fixture(&managed, "mpv", managed_bytes);

    let sources = Sources {
        managed,
        beside_app: root.path().join("beside"),
    };
    let verified = resolve(
        Component::Mpv,
        Some(&override_path),
        &sources,
        &artifact_for(override_bytes),
    )
    .expect("the override verifies");

    assert_eq!(verified.path(), override_path);
    assert_eq!(verified.sha256(), digest_of_bytes(override_bytes));
}

#[test]
fn the_resolver_falls_back_to_managed_then_to_beside_the_application() {
    let root = TempDir::new().expect("a temporary directory");
    let managed = root.path().join("managed");
    let beside = root.path().join("beside");
    fs::create_dir_all(&managed).expect("the fixture directory is writable");
    fs::create_dir_all(&beside).expect("the fixture directory is writable");

    let bytes = mpv_bytes();
    let in_beside = fixture(&beside, "mpv", bytes);
    let in_managed = fixture(&managed, "mpv", bytes);
    let sources = Sources {
        managed: managed.clone(),
        beside_app: beside,
    };

    let from_managed = resolve(Component::Mpv, None, &sources, &artifact_for(bytes))
        .expect("the managed copy verifies");
    assert_eq!(from_managed.path(), in_managed);

    fs::remove_file(&in_managed).expect("the fixture is removable");
    let from_beside = resolve(Component::Mpv, None, &sources, &artifact_for(bytes))
        .expect("the bundled copy verifies");
    assert_eq!(from_beside.path(), in_beside);
}

#[test]
fn a_refused_override_does_not_fall_through_to_the_managed_copy() {
    let root = TempDir::new().expect("a temporary directory");
    let managed = root.path().join("managed");
    let override_directory = root.path().join("override");
    fs::create_dir_all(&managed).expect("the fixture directory is writable");
    fs::create_dir_all(&override_directory).expect("the fixture directory is writable");

    let bytes = mpv_bytes();
    fixture(&managed, "mpv", bytes);
    let override_path = fixture(&override_directory, "mpv", b"a different build entirely");

    let refused = resolve(
        Component::Mpv,
        Some(&override_path),
        &Sources {
            managed,
            beside_app: root.path().join("beside"),
        },
        &artifact_for(bytes),
    )
    .expect_err("the override does not match its pin");

    assert!(
        matches!(refused, ResolveError::NotResolved { tried: 1, .. }),
        "the override is final: {refused}"
    );
}

#[test]
fn an_empty_or_whitespace_override_is_a_typed_mistake_not_an_absence() {
    let root = TempDir::new().expect("a temporary directory");
    let sources = Sources {
        managed: root.path().join("managed"),
        beside_app: root.path().join("beside"),
    };
    let artifact = artifact_for(mpv_bytes());

    for override_path in [Path::new(""), Path::new("  "), Path::new("\t\n")] {
        let refused = resolve(Component::Mpv, Some(override_path), &sources, &artifact)
            .expect_err("a blank setting is not a valid path");
        assert!(
            matches!(refused, ResolveError::EmptyOverride { component } if component == Component::Mpv),
            "`{override_path:?}` must not be read as no override: {refused}"
        );
    }
}

#[test]
fn the_resolver_does_not_consult_path() {
    // The child process runs the proof; its parent sets the `PATH` the proof
    // needs. The parent does not mutate its own environment, which is both
    // `unsafe` in edition 2024 and unsound while other tests are running.
    if env::var_os(PATH_PROOF_ENV).is_some() {
        run_path_proof();
        return;
    }

    let root = TempDir::new().expect("a temporary directory");
    let on_path = root.path().join("on-path");
    fs::create_dir_all(&on_path).expect("the fixture directory is writable");
    let mock = fixture(&on_path, &Component::Mpv.file_name(), b"a mock on PATH");
    make_executable(&mock);

    let mut extended = on_path.into_os_string();
    if let Some(previous) = env::var_os("PATH") {
        extended.push(":");
        extended.push(previous);
    }

    let executable = env::current_exe().expect("the test binary knows its own path");
    let output = Command::new(executable)
        .args([
            "sidecar::tests::the_resolver_does_not_consult_path",
            "--exact",
        ])
        .env(PATH_PROOF_ENV, "1")
        .env("PATH", extended)
        .output()
        .expect("the test binary can be re-run");
    let report = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "the child process failed:\n{report}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        report.contains("1 passed"),
        "the child must have run the proof rather than nothing:\n{report}"
    );
    assert!(mock.exists(), "the mock outlives the child that ignored it");
}

/// The body of the `PATH` proof, run in the child process whose `PATH` the
/// parent extended with the mock's directory.
fn run_path_proof() {
    let root = TempDir::new().expect("a temporary directory");
    let managed = root.path().join("managed");
    let beside = root.path().join("beside");
    fs::create_dir_all(&managed).expect("the fixture directory is writable");
    fs::create_dir_all(&beside).expect("the fixture directory is writable");

    // The mock carries the exact name the resolver looks for, sits first on
    // `PATH`, and is executable. Nothing in the two explicit locations is: if
    // resolution ever consults `PATH`, it finds this file and returns it.
    let on_path = env::var_os("PATH")
        .and_then(|path| env::split_paths(&path).next())
        .expect("the parent put the mock's directory on PATH");
    let mock = on_path.join(Component::Mpv.file_name());
    assert!(mock.exists(), "the proof needs the mock the parent planted");

    let refused = resolve(
        Component::Mpv,
        None,
        &Sources {
            managed,
            beside_app: beside,
        },
        &artifact_for(mpv_bytes()),
    )
    .expect_err("a file on PATH is not a resolved sidecar");

    assert!(
        matches!(refused, ResolveError::NotResolved { tried: 2, .. }),
        "only the two explicit locations may be tried: {refused}"
    );
    assert!(
        mock.exists(),
        "the mock must still be there, or the proof proves nothing"
    );
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .expect("the fixture exists")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("the fixture is chmod-able");
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {
    // On Windows executability follows the extension, and the fixture already
    // carries the platform's executable name.
}

#[test]
fn a_missing_mpv_costs_playback_and_nothing_else() {
    let root = TempDir::new().expect("a temporary directory");
    let outcomes = [
        (
            Component::Mpv,
            Err(VerifyError::Missing {
                path: root.path().join("mpv"),
                source: io::Error::new(io::ErrorKind::NotFound, "no such file"),
            }),
        ),
        (
            Component::Ffprobe,
            Ok(verified_token(root.path(), b"ffprobe-fixture")),
        ),
        (
            Component::Ffmpeg,
            Ok(verified_token(root.path(), b"ffmpeg-fixture")),
        ),
        (
            Component::SevenZip,
            Ok(verified_token(root.path(), b"7zz-fixture")),
        ),
        (
            Component::Par2,
            Ok(verified_token(root.path(), b"par2-fixture")),
        ),
    ];
    let report = CapabilityReport::from_outcomes(&outcomes);

    let names: Vec<_> = report.lost.iter().map(|loss| loss.capability).collect();
    assert_eq!(
        names,
        vec![Capability::Playback],
        "a missing mpv may not cost a capability that does not need it"
    );

    let playback = report
        .loss(Capability::Playback)
        .expect("playback is the loss");
    assert_eq!(playback.components(), vec!["mpv"]);
    assert!(
        playback.reasons[0].1.contains("mpv"),
        "the reason names the component: {}",
        playback.reasons[0].1
    );

    assert_eq!(report.granted.len(), 5);
    assert!(report.loss(Capability::Probe).is_none());
    assert!(report.loss(Capability::Transcode).is_none());
    assert!(report.loss(Capability::SubtitleExtract).is_none());
    assert!(report.loss(Capability::Extract).is_none());
    assert!(report.loss(Capability::Repair).is_none());
}

#[test]
fn a_missing_ffmpeg_costs_transcode_and_subtitle_extraction() {
    let root = TempDir::new().expect("a temporary directory");
    let outcomes = [
        (
            Component::Mpv,
            Ok(verified_token(root.path(), b"mpv-fixture")),
        ),
        (
            Component::Ffprobe,
            Ok(verified_token(root.path(), b"ffprobe-fixture")),
        ),
        (
            Component::Ffmpeg,
            Err(VerifyError::Size {
                path: root.path().join("ffmpeg"),
                expected: 10,
                actual: 4,
            }),
        ),
    ];
    let report = CapabilityReport::from_outcomes(&outcomes);

    let names: Vec<_> = report.lost.iter().map(|loss| loss.capability).collect();
    assert_eq!(
        names,
        vec![
            Capability::Transcode,
            Capability::SubtitleExtract,
            Capability::Extract,
            Capability::Repair,
        ]
    );
    assert!(report.loss(Capability::Playback).is_none());
    assert!(report.loss(Capability::Probe).is_none());
}

#[test]
fn a_component_that_was_never_installed_is_named_rather_than_omitted() {
    let outcomes = [(
        Component::Mpv,
        Err(VerifyError::Missing {
            path: PathBuf::from("/managed/mpv"),
            source: io::Error::new(io::ErrorKind::NotFound, "no such file"),
        }),
    )];
    let report = CapabilityReport::from_outcomes(&outcomes);

    assert_eq!(report.lost.len(), 6, "every capability is accounted for");
    assert!(report.granted.is_empty());
    let probe = report.loss(Capability::Probe).expect("probe is lost");
    assert_eq!(probe.components(), vec!["ffprobe"]);
    assert_eq!(probe.reasons[0].1, "not installed");
}

#[test]
fn a_component_names_the_file_windows_needs() {
    let expected = if cfg!(windows) { "mpv.exe" } else { "mpv" };
    assert_eq!(Component::Mpv.file_name(), expected);
    assert_eq!(Component::Mpv.name(), "mpv");
    assert_eq!(Component::Ffprobe.to_string(), "ffprobe");
    assert_eq!(Capability::SubtitleExtract.to_string(), "subtitle-extract");
}

#[test]
fn every_capability_requires_a_component_that_exists() {
    for capability in Capability::ALL {
        assert!(
            !capability.requires().is_empty(),
            "{capability} would be silently free"
        );
        for component in capability.requires() {
            assert!(Component::ALL.contains(component));
            assert_eq!(component.name(), component.to_string());
        }
    }
}

#[test]
fn a_full_installation_grants_every_capability_once() {
    let root = TempDir::new().expect("a temporary directory");
    let outcomes = [
        (
            Component::Mpv,
            Ok(verified_token(root.path(), b"mpv-fixture")),
        ),
        (
            Component::Ffprobe,
            Ok(verified_token(root.path(), b"ffprobe-fixture")),
        ),
        (
            Component::Ffmpeg,
            Ok(verified_token(root.path(), b"ffmpeg-fixture")),
        ),
        (
            Component::SevenZip,
            Ok(verified_token(root.path(), b"7zz-fixture")),
        ),
        (
            Component::Par2,
            Ok(verified_token(root.path(), b"par2-fixture")),
        ),
    ];
    let report = CapabilityReport::from_outcomes(&outcomes);

    assert!(report.lost.is_empty(), "{:?}", report.lost);
    let capabilities: Vec<_> = report
        .granted
        .iter()
        .map(|granted| granted.capability)
        .collect();
    assert_eq!(capabilities, Capability::ALL);
    // `ffmpeg` provides two capabilities, and both hold a token for the same
    // verification rather than one of them losing the token to the other.
    let ffmpeg_grant = report
        .granted
        .iter()
        .find(|granted| granted.capability == Capability::Transcode)
        .expect("transcode is granted");
    assert!(
        ffmpeg_grant.sidecar.path().ends_with("staged-fixture"),
        "the grant carries the verified component's path"
    );
    // Each grant borrows the one token, so all six point at the five
    // verifications rather than at copies of them.
    assert_eq!(report.granted.len(), Capability::ALL.len());
}
