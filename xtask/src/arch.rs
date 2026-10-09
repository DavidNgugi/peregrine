//! `check-arch`: enforces the dependency direction from `arch-allowlist.toml`.
//!
//! Two things are checked. First, every declared dependency — runtime, build,
//! dev, and the target-specific tables that are easy to forget — is either an
//! internal edge the allowlist permits or a third-party crate the crate's policy
//! permits. Second, a crate marked `pure` has its sources scanned for IO, async,
//! clock, and randomness paths, because `peregrine-domain` must stay testable
//! without a database, a network, or a GUI (ARCHITECTURE P2, AGENTS R6).

use crate::report::{Report, Violation};
use crate::workspace::{self, Member};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The allowlist file, as data.
#[derive(Debug, Deserialize)]
pub struct Allowlist {
    pub workspace: WorkspacePolicy,
    #[serde(rename = "dev-dependencies")]
    pub dev_dependencies: DevPolicy,
    pub purity: PurityPolicy,
    pub crates: BTreeMap<String, CratePolicy>,
}

#[derive(Debug, Deserialize)]
pub struct WorkspacePolicy {
    pub members: Vec<String>,
    #[serde(default)]
    pub unpublished: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct DevPolicy {
    pub allowed: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct PurityPolicy {
    pub forbidden_paths: Vec<String>,
    #[serde(default)]
    pub allowed_files: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct CratePolicy {
    #[serde(default)]
    pub may_depend_on: Vec<String>,
    #[serde(default)]
    pub runtime: Vec<String>,
    #[serde(default)]
    pub build: Vec<String>,
    #[serde(default)]
    pub pure: bool,
}

pub const ALLOWLIST_FILE: &str = "arch-allowlist.toml";

/// Reads and parses the allowlist. A missing or malformed file is an error, not
/// a violation: without it there is nothing to check against.
pub fn load(root: &Path) -> Result<Allowlist> {
    let path = root.join(ALLOWLIST_FILE);
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!("cannot read {ALLOWLIST_FILE}; it is required at the repository root")
    })?;
    toml::from_str(&text).with_context(|| format!("cannot parse {ALLOWLIST_FILE}"))
}

/// Runs the gate. Returns the report and how many dependency edges were checked.
pub fn check(root: &Path) -> Result<(Report, usize)> {
    let allowlist = load(root)?;
    let members = workspace::expand_members(root, &allowlist.workspace.members)?;
    let member_names: Vec<String> = members.iter().map(|m| m.name.clone()).collect();

    let mut report = Report::new();
    let mut edges = 0_usize;

    for name in &member_names {
        if !allowlist.crates.contains_key(name) {
            report.push(Violation::new(
                ALLOWLIST_FILE,
                format!(
                    "workspace member `{name}` has no [crates.{name}] policy; \
                     add one so its edges are reviewable"
                ),
            ));
        }
    }
    for name in allowlist.crates.keys() {
        if !member_names.contains(name) {
            report.push(Violation::new(
                ALLOWLIST_FILE,
                format!("[crates.{name}] names a crate that is not a workspace member"),
            ));
        }
    }

    for member in &members {
        let Some(policy) = allowlist.crates.get(&member.name) else {
            continue;
        };
        edges += check_edges(member, policy, &allowlist, &member_names, &mut report);
        if policy.pure {
            check_purity(root, member, &allowlist.purity, &mut report);
        }
    }

    Ok((report, edges))
}

fn check_edges(
    member: &Member,
    policy: &CratePolicy,
    allowlist: &Allowlist,
    member_names: &[String],
    report: &mut Report,
) -> usize {
    let manifest_path = relative(member);
    let mut checked = 0_usize;
    for (kind, table) in workspace::dependency_tables(&member.manifest) {
        let Some(entries) = table.as_table() else {
            continue;
        };
        for (dep_name, spec) in entries {
            checked += 1;
            if let Some(target) = workspace::internal_target(dep_name, spec, member_names) {
                if !policy.may_depend_on.contains(&target) {
                    report.push(Violation::new(
                        &manifest_path,
                        format!(
                            "`{}` -> `{}` is not a permitted edge ({kind}); \
                             add it to [crates.{}].may_depend_on deliberately or remove it",
                            member.name, target, member.name
                        ),
                    ));
                }
                continue;
            }
            let permitted: &[String] = match kind {
                "runtime" => &policy.runtime,
                "build" => &policy.build,
                _ => &allowlist.dev_dependencies.allowed,
            };
            if !permitted.iter().any(|p| p == dep_name) {
                report.push(Violation::new(
                    &manifest_path,
                    format!(
                        "`{dep_name}` is not permitted in [{kind}-dependencies] for `{}`; \
                         a new dependency needs a stated reason and an allowlist change",
                        member.name
                    ),
                ));
            }
        }
    }
    checked
}

/// Best-effort source scan for a pure crate.
///
/// This is a guardrail, not a sandbox: an alias or a macro could hide a
/// forbidden path, and that is a review matter. Comments are stripped first so
/// that a doc comment explaining the rule does not trip it.
fn check_purity(root: &Path, member: &Member, policy: &PurityPolicy, report: &mut Report) {
    let src = member.dir.join("src");
    let mut files = Vec::new();
    collect_rust_files(&src, &mut files);
    for file in files {
        let file_name = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        if policy.allowed_files.iter().any(|a| a == &file_name) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let rel = file
            .strip_prefix(root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        for (number, line) in text.lines().enumerate() {
            let code = strip_comment(line);
            for forbidden in &policy.forbidden_paths {
                if code.contains(forbidden.as_str()) {
                    report.push(Violation::new(
                        &rel,
                        format!(
                            "line {}: `{forbidden}` appears in a pure crate; `{}` must stay free of IO, async, clock, and randomness",
                            number + 1,
                            member.name
                        ),
                    ));
                }
            }
        }
    }
}

fn strip_comment(line: &str) -> &str {
    match line.find("//") {
        Some(index) => &line[..index],
        None => line,
    }
}

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
    out.sort();
}

fn relative(member: &Member) -> String {
    member
        .dir
        .join("Cargo.toml")
        .to_string_lossy()
        .replace('\\', "/")
}

/// Convenience for the binary: checks a root and reports a top-level error.
pub fn run(root: &Path) -> Result<bool> {
    if !root.join(ALLOWLIST_FILE).is_file() {
        bail!("no {ALLOWLIST_FILE} at {}", root.display());
    }
    let (report, checked) = check(root)?;
    let ok = crate::report::finish("check-arch", &report, checked);
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/arch")
            .join(name)
    }

    #[test]
    fn real_tree_is_clean() {
        let root = workspace::find_root(&PathBuf::from(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let (report, checked) = check(&root).unwrap();
        assert!(
            report.is_clean(),
            "the real workspace violates the allowlist: {:#?}",
            report.sorted()
        );
        assert!(checked > 0, "no dependency edges were checked");
    }

    #[test]
    fn illegal_internal_edge_is_rejected() {
        let (report, _) = check(&fixture("illegal-internal-edge")).unwrap();
        assert!(
            report
                .sorted()
                .iter()
                .any(|v| v.message.contains("not a permitted edge")),
            "expected an internal-edge violation, got {:#?}",
            report.sorted()
        );
    }

    #[test]
    fn illegal_external_dependency_is_rejected() {
        let (report, _) = check(&fixture("illegal-external-dep")).unwrap();
        assert!(
            report
                .sorted()
                .iter()
                .any(|v| v.message.contains("is not permitted")),
            "expected a dependency violation, got {:#?}",
            report.sorted()
        );
    }

    #[test]
    fn impure_domain_source_is_rejected() {
        let (report, _) = check(&fixture("impure-domain")).unwrap();
        assert!(
            report
                .sorted()
                .iter()
                .any(|v| v.message.contains("std::fs") || v.message.contains("SystemTime")),
            "expected a purity violation, got {:#?}",
            report.sorted()
        );
    }

    #[test]
    fn member_without_policy_is_rejected() {
        let (report, _) = check(&fixture("member-without-policy")).unwrap();
        assert!(
            report
                .sorted()
                .iter()
                .any(|v| v.message.contains("has no [crates.")),
            "expected a missing-policy violation, got {:#?}",
            report.sorted()
        );
    }
}
