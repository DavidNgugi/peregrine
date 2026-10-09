//! Locating the workspace root and expanding the member globs in the allowlist.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// A workspace member: its package name, its directory, and its parsed manifest.
#[derive(Debug)]
pub struct Member {
    pub name: String,
    pub dir: PathBuf,
    pub manifest: toml::Value,
}

/// Walks up from `start` to the first directory whose `Cargo.toml` declares a
/// `[workspace]`. Uses a text check rather than a full parse so a malformed
/// manifest still produces a useful "not a workspace" error instead of a parse
/// error from the wrong place.
pub fn find_root(start: &Path) -> Result<PathBuf> {
    let mut dir = start
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", start.display()))?;
    loop {
        let candidate = dir.join("Cargo.toml");
        if candidate.is_file() {
            let text = std::fs::read_to_string(&candidate)
                .with_context(|| format!("cannot read {}", candidate.display()))?;
            if text.contains("[workspace]") {
                return Ok(dir);
            }
        }
        if !dir.pop() {
            bail!("no Cargo workspace found at or above {}", start.display());
        }
    }
}

/// Expands member globs into the members that exist on disk.
///
/// Supported patterns are literal segments and a single `*` segment, which is
/// all `arch-allowlist.toml` uses. A pattern that matches nothing is skipped: a
/// crate that a later phase has not created yet is not a violation here, but a
/// member with no policy entry is (see `arch::check`).
pub fn expand_members(root: &Path, patterns: &[String]) -> Result<Vec<Member>> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for pattern in patterns {
        dirs.extend(match_pattern(root, pattern));
    }
    dirs.sort();
    dirs.dedup();

    let mut members = Vec::new();
    for dir in dirs {
        let manifest_path = dir.join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("cannot read {}", manifest_path.display()))?;
        let manifest: toml::Value = toml::from_str(&text)
            .with_context(|| format!("cannot parse {}", manifest_path.display()))?;
        let name = manifest
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(toml::Value::as_str)
            .with_context(|| {
                format!(
                    "{} has no [package] name; workspace members must be packages",
                    manifest_path.display()
                )
            })?
            .to_owned();
        members.push(Member {
            name,
            dir,
            manifest,
        });
    }
    Ok(members)
}

fn match_pattern(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let segments: Vec<&str> = pattern.split('/').collect();
    let mut current = vec![root.to_path_buf()];
    for segment in segments {
        let mut next = Vec::new();
        for dir in &current {
            if segment == "*" {
                let Ok(entries) = std::fs::read_dir(dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() && path.join("Cargo.toml").is_file() {
                        next.push(path);
                    }
                }
            } else {
                let path = dir.join(segment);
                if path.is_dir() {
                    next.push(path);
                }
            }
        }
        current = next;
    }
    current
}

/// Maps a dependency table name to the kind the allowlist distinguishes.
fn table_kind(key: &str) -> &'static str {
    match key {
        "dependencies" => "runtime",
        "build-dependencies" => "build",
        _ => "dev",
    }
}

/// Top-level dependency tables that carry runtime or build edges, including the
/// target-specific ones, which are easy to forget and just as real.
#[must_use]
pub fn dependency_tables(manifest: &toml::Value) -> Vec<(&'static str, &toml::Value)> {
    const TABLES: [&str; 3] = ["dependencies", "build-dependencies", "dev-dependencies"];
    let mut out = Vec::new();
    for key in TABLES {
        if let Some(table) = manifest.get(key) {
            out.push((table_kind(key), table));
        }
    }
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        for target in targets.values() {
            for key in TABLES {
                if let Some(table) = target.get(key) {
                    out.push((table_kind(key), table));
                }
            }
        }
    }
    out
}

/// The workspace crate a dependency table entry points at, if it is internal:
/// either the key is a member name, or the spec carries a `path` that resolves
/// to a member directory. Renaming the key does not hide the edge.
#[must_use]
pub fn internal_target(
    dep_name: &str,
    dep_spec: &toml::Value,
    member_names: &[String],
) -> Option<String> {
    if let Some(path) = dep_spec.get("path").and_then(toml::Value::as_str) {
        if let Some(last) = path.rsplit('/').find(|segment| !segment.is_empty())
            && let Some(found) = member_names
                .iter()
                .find(|n| n.trim_start_matches("peregrine-") == last || n.as_str() == last)
        {
            return Some(found.clone());
        }
        return Some(dep_name.to_owned());
    }
    member_names
        .iter()
        .find(|n| n.as_str() == dep_name)
        .cloned()
}
