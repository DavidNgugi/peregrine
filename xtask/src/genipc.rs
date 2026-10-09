//! `gen-ipc`: the committed TypeScript bindings must match a fresh export.
//!
//! The bindings are generated from the Rust command surface, so the gate is a
//! byte comparison rather than a review: `cargo xtask gen-ipc --check` fails when
//! `packages/ipc-client/src/generated/bindings.ts` is not exactly what the export
//! entry point in `peregrine-ipc` produces. `--update` writes it instead, which is
//! the only supported way to change the file; editing it by hand turns the next
//! gate run red.
//!
//! The export runs as a subprocess (`peregrine-ipc`'s `gen-ipc` binary) because
//! `peregrine-ipc` depends on Tauri, and making `xtask` depend on it would put
//! the whole webview stack on the path of every gate. Following the convention of
//! `check-dead-deps`, the boundary is a process, not a crate edge.
//!
//! The comparison is a pure function over `(repository root, generated text)`, so
//! the drift logic is tested against a fixture tree without running Cargo.

use crate::report::{Report, Violation};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the generated client lives, relative to the repository root.
pub const BINDINGS: &str = "packages/ipc-client/src/generated/bindings.ts";

/// The crate that owns the export entry point.
const GENERATOR_PACKAGE: &str = "peregrine-ipc";

/// The generator binary in that crate.
const GENERATOR_BIN: &str = "gen-ipc";

/// Whether `gen-ipc` was asked to rewrite the bindings or to check them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Fail when the committed file differs from a fresh export.
    Check,
    /// Overwrite the committed file with a fresh export.
    Update,
}

/// Runs the gate or the generator. Returns whether the command succeeded.
pub fn run(root: &Path, mode: Mode) -> Result<bool> {
    let out = temporary_output();
    let generated = generate(root, &out)?;
    let _ = std::fs::remove_file(&out);

    if mode == Mode::Update {
        let path = root.join(BINDINGS);
        std::fs::write(&path, &generated)
            .with_context(|| format!("cannot write {}", path.display()))?;
        println!("gen-ipc: wrote {BINDINGS} ({} bytes)", generated.len());
        return Ok(true);
    }

    let (report, checked) = check(root, &generated);
    Ok(crate::report::finish("gen-ipc", &report, checked))
}

/// Compares the committed bindings against a fresh export.
///
/// Trailing whitespace is ignored so a Windows checkout that renormalises line
/// endings does not report drift; everything else is compared byte for byte.
pub fn check(root: &Path, generated: &str) -> (Report, usize) {
    let mut report = Report::new();
    let path = root.join(BINDINGS);
    match std::fs::read_to_string(&path) {
        Ok(committed) => {
            if committed.trim_end() != generated.trim_end() {
                report.push(Violation::new(
                    BINDINGS,
                    "does not match the Rust command surface; run `cargo xtask gen-ipc --update`",
                ));
            }
        }
        Err(error) => report.push(Violation::new(
            BINDINGS,
            format!("cannot be read ({error}); run `cargo xtask gen-ipc --update`"),
        )),
    }
    (report, 1)
}

/// A path in the system temporary directory that this process owns.
///
/// The process id keeps concurrent runs from colliding, and the timestamp keeps a
/// file left behind by a killed run from being reused. The file lives outside the
/// repository so a failed run cannot leave a stray artifact in a tree that
/// `git status` is expected to be clean in.
fn temporary_output() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let name = format!(".peregrine-gen-ipc-{}-{stamp}.ts", std::process::id());
    std::env::temp_dir().join(name)
}

/// Runs the generator binary and returns the text it produced.
fn generate(root: &Path, out: &Path) -> Result<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = Command::new(cargo)
        .current_dir(root)
        .args([
            "run",
            "--quiet",
            "--package",
            GENERATOR_PACKAGE,
            "--bin",
            GENERATOR_BIN,
            "--",
            "--out",
        ])
        .arg(out)
        .output()
        .context("cannot run the IPC binding generator")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("the IPC binding generator failed:\n{stderr}");
    }
    std::fs::read_to_string(out)
        .with_context(|| format!("the generator did not write {}", out.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const EXPORT: &str = "// generated\nexport const commands = {};\n";

    /// A fixture tree holding nothing but the committed bindings, so the check
    /// runs without Cargo and without the real packages.
    fn tree(committed: &str) -> TempDir {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join(BINDINGS);
        std::fs::create_dir_all(path.parent().expect("bindings directory"))
            .expect("create the fixture tree");
        std::fs::write(&path, committed).expect("write the committed bindings");
        dir
    }

    #[test]
    fn identical_bindings_are_clean() {
        let root = tree(EXPORT);
        let (report, checked) = check(root.path(), EXPORT);
        assert!(report.is_clean(), "{:?}", report.sorted());
        assert_eq!(checked, 1);
    }

    #[test]
    fn a_changed_line_is_a_violation() {
        let root = tree("// generated\nexport const commands = { buildInfo: () => {} };\n");
        let (report, _) = check(root.path(), EXPORT);
        assert_eq!(report.len(), 1);
        let violation = report.sorted()[0];
        assert_eq!(violation.path, BINDINGS);
        assert!(
            violation.message.contains("cargo xtask gen-ipc --update"),
            "the violation must say how to fix it: {}",
            violation.message
        );
    }

    /// A checkout that renormalised line endings must not fail the gate; the
    /// comparison trims the trailing whitespace on both sides.
    #[test]
    fn trailing_whitespace_is_not_drift() {
        let root = tree(&format!("{EXPORT}\n\n"));
        let (report, _) = check(root.path(), EXPORT.trim_end());
        assert!(report.is_clean(), "{:?}", report.sorted());
    }

    /// An empty committed file is drift, not a clean tree: the gate must never
    /// pass because it compared nothing.
    #[test]
    fn an_empty_committed_file_is_a_violation() {
        let root = tree("");
        let (report, _) = check(root.path(), EXPORT);
        assert_eq!(report.len(), 1);
    }

    #[test]
    fn a_missing_bindings_file_is_a_violation() {
        let root = tempfile::tempdir().expect("temporary directory");
        let (report, _) = check(root.path(), EXPORT);
        assert_eq!(report.len(), 1);
        assert_eq!(report.sorted()[0].path, BINDINGS);
    }
}
