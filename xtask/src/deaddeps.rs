//! `check-dead-deps`: a manifest must not declare a dependency its sources never use.
//!
//! Wraps `cargo-machete`, which parses each manifest and searches the crate's
//! sources for the dependency's name. Two runs happen, and the second is what
//! makes this a gate rather than a habit: a fixture that is known to violate the
//! rule must be reported as a violation. If the tool ever stops detecting unused
//! dependencies, this command fails loudly instead of reporting a clean tree.
//!
//! The `check-arch` violation fixtures are deliberately not scanned. They declare
//! dependencies their sources never name, because that is the violation they
//! exist to prove. `xtask/Cargo.toml` is named as a file rather than passing the
//! `xtask` directory so those fixtures are excluded without an ignore list.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Paths scanned in the real repository.
pub const SCANNED: &[&str] = &["crates", "apps/desktop/src-tauri", "xtask/Cargo.toml"];

/// A package whose manifest declares a dependency its sources never use.
pub const VIOLATION_FIXTURE: &str = "xtask/fixtures/dead-deps/unused-dep";

/// `cargo-machete` exit codes, from its documented contract.
const CLEAN: i32 = 0;
const UNUSED_FOUND: i32 = 1;

/// What one `cargo-machete` run reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Every declared dependency is named somewhere in the crate's sources.
    Clean,
    /// At least one declared dependency is not.
    UnusedFound,
}

/// Reads a `cargo-machete` exit code.
///
/// `None` means the tool did not get far enough to answer rather than that it
/// answered "clean", which is why this is not a `bool`. Keeping the comparison
/// here rather than inline in [`run`] is what lets `cargo test` exercise it on a
/// machine where `cargo-machete` is not installed.
fn verdict(code: i32) -> Option<Verdict> {
    match code {
        CLEAN => Some(Verdict::Clean),
        UNUSED_FOUND => Some(Verdict::UnusedFound),
        _ => None,
    }
}

/// Runs the gate. Returns whether the repository is clean.
pub fn run(root: &Path) -> Result<bool> {
    let targets = scanned_targets(root)?;
    let (code, output) = machete(root, &targets)?;
    match verdict(code) {
        Some(Verdict::Clean) => {}
        Some(Verdict::UnusedFound) => {
            eprintln!("check-dead-deps: unused dependencies found:");
            eprintln!("{output}");
            eprintln!(
                "  Remove the dependency, or move it to the table where it is actually used."
            );
            return Ok(false);
        }
        None => bail!("cargo-machete exited {code} while scanning the workspace:\n{output}"),
    }

    let fixture = root.join(VIOLATION_FIXTURE);
    let (code, output) = machete(root, &[fixture])?;
    match verdict(code) {
        Some(Verdict::UnusedFound) => {}
        Some(Verdict::Clean) => bail!(
            "the gate is broken: {VIOLATION_FIXTURE} declares a dependency its sources never \
             use, but it was reported clean. Fix the gate before trusting a green run."
        ),
        None => bail!("cargo-machete exited {code} on the violation fixture:\n{output}"),
    }

    println!(
        "check-dead-deps: ok ({} path(s) scanned, violation fixture still detected)",
        targets.len()
    );
    Ok(true)
}

/// Resolves the scanned paths, failing if one is missing.
///
/// A silently skipped path is a gate that has quietly stopped covering something,
/// so a configured path that no longer exists is an error rather than a warning.
fn scanned_targets(root: &Path) -> Result<Vec<PathBuf>> {
    let mut targets = Vec::with_capacity(SCANNED.len());
    for entry in SCANNED {
        let path = root.join(entry);
        if !path.exists() {
            bail!(
                "configured scan path `{entry}` does not exist; update SCANNED in \
                 xtask/src/deaddeps.rs if the layout changed"
            );
        }
        targets.push(path);
    }
    Ok(targets)
}

/// Invokes `cargo-machete` and returns its exit code with combined output.
///
/// The binary is executed directly rather than through `cargo machete`, because
/// cargo forwards the subcommand name to the tool. The extra `machete` argument
/// is then read as a path to scan, so the gate fails with an IO error on a
/// directory that does not exist instead of scanning what it was given.
fn machete(root: &Path, targets: &[PathBuf]) -> Result<(i32, String)> {
    let binary = find_machete().context(
        "`cargo-machete` is not on PATH. Install it with:\n  \
         cargo install --locked cargo-machete",
    )?;
    let output = Command::new(&binary)
        .args(targets)
        .current_dir(root)
        .output()
        .with_context(|| format!("failed to run {}", binary.display()))?;

    // Findings go to stdout and diagnostics to stderr, so both are kept: a
    // failure that explains itself is worth more than a bare exit code.
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));

    let Some(code) = output.status.code() else {
        bail!(
            "{} was terminated by a signal:\n{combined}",
            binary.display()
        );
    };
    Ok((code, combined))
}

/// Locates `cargo-machete` on `PATH`.
fn find_machete() -> Option<PathBuf> {
    let name = format!("cargo-machete{}", std::env::consts::EXE_SUFFIX);
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|directory| directory.join(&name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_root() -> PathBuf {
        crate::workspace::find_root(&PathBuf::from(env!("CARGO_MANIFEST_DIR")))
            .expect("xtask lives inside the workspace")
    }

    #[test]
    fn every_configured_scan_path_exists() {
        let root = workspace_root();
        let targets = scanned_targets(&root).expect("configured paths must exist");
        assert_eq!(targets.len(), SCANNED.len());
    }

    #[test]
    fn a_missing_scan_path_is_an_error_not_a_silent_skip() {
        let missing = tempfile::TempDir::new().unwrap();
        let error = scanned_targets(missing.path()).unwrap_err().to_string();
        assert!(error.contains("does not exist"), "{error}");
    }

    /// The negative fixture is only useful while it still violates the rule, so
    /// that is asserted directly from the manifest and its sources. This runs
    /// without cargo-machete installed, which keeps the fixture honest in a
    /// plain `cargo test`.
    #[test]
    fn the_violation_fixture_still_declares_an_unused_dependency() {
        let root = workspace_root();
        let fixture = root.join(VIOLATION_FIXTURE);
        let manifest = std::fs::read_to_string(fixture.join("Cargo.toml")).unwrap();
        let parsed: toml::Value = toml::from_str(&manifest).unwrap();

        let declared: Vec<&String> = parsed
            .get("dependencies")
            .and_then(toml::Value::as_table)
            .expect("the fixture must declare dependencies")
            .keys()
            .collect();
        assert!(
            !declared.is_empty(),
            "the fixture declares no dependencies, so it proves nothing"
        );

        let mut sources = String::new();
        for entry in std::fs::read_dir(fixture.join("src")).unwrap().flatten() {
            if entry.path().extension().and_then(|e| e.to_str()) == Some("rs") {
                sources.push_str(&std::fs::read_to_string(entry.path()).unwrap());
            }
        }
        for dependency in declared {
            let needle = dependency.replace('-', "_");
            assert!(
                !sources.contains(dependency.as_str()) && !sources.contains(&needle),
                "the fixture's sources now use `{dependency}`, so it no longer proves the gate fails"
            );
        }
    }

    /// The gate's own comparison, as opposed to the fixture's contents: `CLEAN`
    /// and `UNUSED_FOUND` are the only exit codes that mean the tool answered,
    /// and the verdict the fixture run must reach is the same `UNUSED_FOUND`
    /// that fails the workspace run. Asserted here because a plain `cargo test`
    /// on a machine without `cargo-machete` would otherwise check the fixture
    /// but never the logic that reads it.
    #[test]
    fn the_only_verdicts_are_clean_and_unused_found() {
        assert_eq!(verdict(CLEAN), Some(Verdict::Clean));
        assert_eq!(verdict(UNUSED_FOUND), Some(Verdict::UnusedFound));
        assert_eq!(
            verdict(2),
            None,
            "a usage error must not read as a clean tree"
        );
        assert_eq!(verdict(-1), None, "a signal must not read as a clean tree");
    }

    /// The arch violation fixtures must stay outside the scanned set.
    #[test]
    fn the_scanned_paths_exclude_the_arch_violation_fixtures() {
        assert!(
            SCANNED.contains(&"xtask/Cargo.toml"),
            "xtask's own manifest must still be scanned"
        );
        for path in SCANNED {
            assert!(
                *path != "xtask" && !path.starts_with("xtask/fixtures"),
                "`{path}` would pull the check-arch violation fixtures into the dead-dependency scan"
            );
        }
    }
}
