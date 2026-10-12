//! `check-determinism`: no shipping crate reads the wall clock or a random source.
//!
//! AGENTS R14 requires tests to be deterministic and offline, so time and identifiers are
//! injected through `peregrine-domain`'s `Clock` and `IdGenerator` ports. The production
//! implementations of both are leaf adapters (AGENTS R8), and `[determinism]` in
//! `arch-allowlist.toml` names the crates allowed to hold them. Every other shipping
//! crate is scanned for a wall-clock or random entry point and fails the build on one.
//!
//! This is a guardrail, not a sandbox, for the same reason `check-arch`'s purity scan is
//! one: an alias, a re-export, a macro, or a call assembled in a string could evade it,
//! and catching that is a review matter. Test-only code is skipped — a `#[cfg(test)]`
//! region, a `#[cfg(test)] mod name;` and the file it names, and anything under a
//! `tests/` directory — as are the `unpublished` crates, which never ship. The monotonic
//! clock is deliberately not a forbidden pattern: `Instant::now` measures a duration
//! rather than reporting a time, and `tokio::time` is what advances it in a test.

use crate::arch;
use crate::report::{Report, Violation};
use crate::workspace::{self, Member};
use anyhow::{Result, bail};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Runs the gate. Returns the report and how many source files were scanned.
pub fn check(root: &Path) -> Result<(Report, usize)> {
    let allowlist = arch::load(root)?;
    let policy = &allowlist.determinism;
    if policy.forbidden.is_empty() {
        bail!(
            "[determinism].forbidden is empty in {}; there is nothing to check for",
            arch::ALLOWLIST_FILE
        );
    }
    let members = workspace::expand_members(root, &allowlist.workspace.members)?;

    let mut report = Report::new();
    let mut checked = 0_usize;
    for member in &members {
        if allowlist.workspace.unpublished.contains(&member.name)
            || policy.adapters.contains(&member.name)
        {
            continue;
        }
        checked += check_member(root, member, &policy.forbidden, &mut report);
    }
    Ok((report, checked))
}

/// Convenience for the binary: checks a root and reports a top-level error.
pub fn run(root: &Path) -> Result<bool> {
    let (report, checked) = check(root)?;
    Ok(crate::report::finish("check-determinism", &report, checked))
}

/// Scans one crate and returns how many files it read.
fn check_member(root: &Path, member: &Member, forbidden: &[String], report: &mut Report) -> usize {
    let mut files = Vec::new();
    arch::collect_rust_files(&member.dir.join("src"), &mut files);
    files.retain(|file| !is_under_tests(file));
    let test_only = cfg_test_module_files(&files);

    let mut checked = 0_usize;
    for file in files {
        if test_only.contains(&file) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        checked += 1;
        let rel = file
            .strip_prefix(root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        let mut region = TestRegion::default();
        for (number, line) in text.lines().enumerate() {
            let code = arch::strip_comment(line);
            if region.covers(code) {
                continue;
            }
            for pattern in forbidden {
                if code.contains(pattern.as_str()) {
                    report.push(Violation::new(
                        &rel,
                        format!(
                            "line {}: `{pattern}` reads a wall clock or a random source; \
                             `{}` is not an adapter crate, so the value it needs must arrive \
                             through `dyn Clock` or `dyn IdGenerator`",
                            number + 1,
                            member.name
                        ),
                    ));
                }
            }
        }
    }
    checked
}

/// Whether a path belongs to an integration-test tree, which never ships.
fn is_under_tests(file: &Path) -> bool {
    file.components().any(|part| part.as_os_str() == "tests")
}

/// The files a crate compiles only under `cfg(test)`: the target of a
/// `#[cfg(test)] mod name;` declaration. The file itself carries no attribute, so it
/// would otherwise be read as shipping code.
fn cfg_test_module_files(files: &[PathBuf]) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        let Some(dir) = file.parent() else {
            continue;
        };
        let mut pending = false;
        for line in text.lines() {
            let code = arch::strip_comment(line).trim();
            if !pending && !code.contains("cfg(test)") {
                continue;
            }
            if let Some(name) = semicolon_module(code) {
                out.insert(dir.join(format!("{name}.rs")));
                out.insert(dir.join(name).join("mod.rs"));
                pending = false;
            } else if code.contains('{') || code.contains(';') {
                // The attribute annotated something else; stop looking for a module.
                pending = false;
            } else {
                pending = true;
            }
        }
    }
    out
}

/// The module name in a `mod name;` declaration, ignoring a leading attribute.
fn semicolon_module(code: &str) -> Option<&str> {
    let code = code.rsplit(']').next().unwrap_or(code).trim();
    let name = code.strip_prefix("mod ")?.strip_suffix(';')?.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    Some(name)
}

/// Whether a line falls inside a `#[cfg(test)]` item.
#[derive(Default)]
struct TestRegion {
    /// Brace depth, so a test block's end can be found.
    depth: i32,
    /// Set between `#[cfg(test)]` and the item it annotates.
    pending: bool,
    /// The depth at which the current test block opened.
    entry: Option<i32>,
}

impl TestRegion {
    /// Reports whether `code` belongs to a test item, advancing the state past it.
    fn covers(&mut self, code: &str) -> bool {
        if let Some(entry) = self.entry {
            self.depth += brace_delta(code);
            if self.depth <= entry {
                self.entry = None;
            }
            return true;
        }
        if !self.pending && !code.contains("cfg(test)") {
            self.depth += brace_delta(code);
            return false;
        }
        match code.find('{') {
            Some(open) => {
                let before = self.depth + brace_delta(&code[..open]);
                self.depth = before + brace_delta(&code[open..]);
                self.pending = false;
                if self.depth > before {
                    self.entry = Some(before);
                }
            }
            // A declaration such as `#[cfg(test)] mod tests;` or `#[cfg(test)] use x;`.
            None if code.contains(';') => self.pending = false,
            None => self.pending = true,
        }
        true
    }
}

fn brace_delta(code: &str) -> i32 {
    code.chars().fold(0, |depth, ch| match ch {
        '{' => depth + 1,
        '}' => depth - 1,
        _ => depth,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/determinism")
            .join(name)
    }

    #[test]
    fn real_tree_is_clean() {
        let root = workspace::find_root(&PathBuf::from(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let (report, checked) = check(&root).unwrap();
        assert!(
            report.is_clean(),
            "a shipping crate reads a clock or a random source: {:#?}",
            report.sorted()
        );
        assert!(checked > 0, "no source files were scanned");
    }

    /// The gate is only as strong as its pattern list, so the entry points the
    /// acceptance names are pinned against the real allowlist: deleting one there
    /// fails this test, not merely a fixture.
    #[test]
    fn the_real_allowlist_names_the_required_entry_points() {
        let root = workspace::find_root(&PathBuf::from(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let allowlist = arch::load(&root).unwrap();
        for pattern in ["Timestamp::now", "Ulid::generate"] {
            assert!(
                allowlist.determinism.forbidden.iter().any(|p| p == pattern),
                "`{pattern}` is missing from [determinism].forbidden"
            );
        }
    }

    #[test]
    fn wall_clock_read_in_a_shipping_crate_is_rejected() {
        let (report, checked) = check(&fixture("wall-clock")).unwrap();
        assert!(checked > 0, "the fixture was not scanned");
        let messages: Vec<&str> = report
            .sorted()
            .iter()
            .map(|violation| violation.message.as_str())
            .collect();
        for pattern in ["`Timestamp::now`", "`Ulid::generate`", "`SystemTime::now`"] {
            assert!(
                messages.iter().any(|m| m.contains(pattern)),
                "expected a {pattern} violation, got {messages:#?}"
            );
        }
        // Two `Instant::now` readings sit in the same file; a gate that flagged the
        // monotonic clock would fail here rather than in review.
        assert_eq!(
            report.len(),
            3,
            "only the wall clock and the random source are violations, got {messages:#?}"
        );
    }

    /// The fixture's wall-clock reads are all inside test-only code or inside the
    /// adapter its allowlist names, and its shipping code reads only the monotonic
    /// clock.
    #[test]
    fn test_only_code_and_adapters_are_skipped() {
        let (report, checked) = check(&fixture("exempt")).unwrap();
        assert!(checked > 0, "the fixture's shipping file was not scanned");
        assert!(
            report.is_clean(),
            "expected an exempt fixture to pass, got {:#?}",
            report.sorted()
        );
    }

    #[test]
    fn a_missing_pattern_list_is_an_error_not_a_pass() {
        let error = check(&fixture("no-patterns")).unwrap_err();
        assert!(
            error.to_string().contains("[determinism].forbidden"),
            "got {error}"
        );
    }

    #[test]
    fn a_test_region_ends_with_its_block() {
        let mut region = TestRegion::default();
        assert!(!region.covers("fn shipping() {"));
        assert!(region.covers("#[cfg(test)]"));
        assert!(region.covers("mod tests {"));
        assert!(region.covers("    fn helper() {"));
        assert!(region.covers("    }"));
        assert!(region.covers("}"));
        assert!(!region.covers("    Timestamp::now();"));
    }

    #[test]
    fn a_semicolon_declaration_is_read_as_a_module() {
        assert_eq!(semicolon_module("mod tests;"), Some("tests"));
        assert_eq!(semicolon_module("#[cfg(test)] mod tests;"), Some("tests"));
        assert_eq!(semicolon_module("mod tests {"), None);
        assert_eq!(semicolon_module("use std::fs;"), None);
    }
}
