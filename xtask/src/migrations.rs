//! `check-migrations`: migrations are append-only and never edited after merge.
//!
//! The journal at `crates/peregrine-storage/migrations/journal.toml` records the
//! SHA-256 of every migration that has been merged. This gate fails when a
//! journal entry's file hash no longer matches, when a migration on disk is not
//! in the journal, or when the numbering is not a gapless sequence.
//!
//! Why a journal rather than a diff against a base branch: a hash check works
//! offline, on a detached checkout, and in a fork with no upstream configured,
//! and it cannot be defeated by a force-push to the base branch. The record of
//! what was merged lives in the repository, next to what it protects.
//!
//! `--update` adds entries for migrations that are not yet recorded. It never
//! rewrites an existing entry: a changed hash means an already-merged migration
//! was edited, and the fix is a new migration, not a refreshed hash.

use crate::report::{Report, Violation};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MIGRATIONS_DIR: &str = "crates/peregrine-storage/migrations";
pub const JOURNAL_FILE: &str = "crates/peregrine-storage/migrations/journal.toml";

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Journal {
    #[serde(default, rename = "migration")]
    pub migrations: Vec<Entry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Entry {
    pub file: String,
    pub sha256: String,
}

/// Hex-encoded SHA-256. The hex encoder is local so the gate does not need a
/// second crate for eight lines of formatting.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        // A nibble is 0..=15, so both indices are provably in range.
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

fn read_journal(root: &Path) -> Result<Journal> {
    let path = root.join(JOURNAL_FILE);
    if !path.is_file() {
        return Ok(Journal::default());
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("cannot read {JOURNAL_FILE}"))?;
    toml::from_str(&text).with_context(|| format!("cannot parse {JOURNAL_FILE}"))
}

/// Migration files present on disk, sorted by name.
fn migration_files(root: &Path) -> Result<Vec<PathBuf>> {
    let dir = root.join(MIGRATIONS_DIR);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .with_context(|| format!("cannot read {MIGRATIONS_DIR}"))?
        .flatten()
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("sql") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Parses the numeric prefix. `0001_core.sql` -> 1. A name without a
/// zero-padded four-digit prefix is a violation, not a skipped file.
// The comparison is deliberately case-sensitive: on macOS and Windows, where the
// filesystem is usually case-insensitive, accepting `.SQL` would let two "distinct"
// migration names resolve to one file and defeat the append-only journal.
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "migration file names are a stable lowercase contract, and case-insensitive filesystems would alias an uppercase variant onto it"
)]
fn numeric_prefix(file_name: &str) -> Option<u32> {
    let (prefix, rest) = file_name.split_once('_')?;
    if prefix.len() != 4 || !prefix.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if rest.is_empty() || !rest.ends_with(".sql") {
        return None;
    }
    prefix.parse().ok()
}

/// Verifies the journal against what is on disk.
pub fn check(root: &Path) -> Result<(Report, usize)> {
    let journal = read_journal(root)?;
    let files = migration_files(root)?;
    let mut report = Report::new();

    let mut expected = 1_u32;
    for path in &files {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        let rel = format!("{MIGRATIONS_DIR}/{file_name}");
        match numeric_prefix(&file_name) {
            Some(number) if number == expected => expected += 1,
            Some(number) => report.push(Violation::new(
                &rel,
                format!(
                    "migration number {number:04} is out of sequence; expected {expected:04}; \
                     migrations are numbered in merge order with no gaps"
                ),
            )),
            None => report.push(Violation::new(
                &rel,
                "name must be NNNN_snake_case.sql with a four-digit zero-padded prefix",
            )),
        }

        match journal.migrations.iter().find(|e| e.file == file_name) {
            None => report.push(Violation::new(
                &rel,
                "not listed in the journal; run `cargo xtask check-migrations --update` \
                 to record a new migration",
            )),
            Some(entry) => {
                let bytes = std::fs::read(path)
                    .with_context(|| format!("cannot read {}", path.display()))?;
                let actual = sha256_hex(&bytes);
                if actual != entry.sha256 {
                    report.push(Violation::new(
                        &rel,
                        format!(
                            "content changed after merge (journal {}, file {actual}); \
                             a merged migration is immutable — add a new migration instead",
                            entry.sha256
                        ),
                    ));
                }
            }
        }
    }

    for entry in &journal.migrations {
        let rel = format!("{MIGRATIONS_DIR}/{}", entry.file);
        if !root.join(MIGRATIONS_DIR).join(&entry.file).is_file() {
            report.push(Violation::new(
                &rel,
                "journal lists a migration file that does not exist",
            ));
        }
    }

    Ok((report, files.len()))
}

/// Records migrations that are not yet journaled. Refuses to touch an entry
/// whose file changed, so it can never be used to launder an edited migration.
pub fn update(root: &Path) -> Result<usize> {
    let mut journal = read_journal(root)?;
    let files = migration_files(root)?;
    let mut added = 0_usize;

    for path in &files {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        if let Some(entry) = journal.migrations.iter().find(|e| e.file == file_name) {
            let bytes = std::fs::read(path)?;
            let actual = sha256_hex(&bytes);
            if actual != entry.sha256 {
                bail!(
                    "{MIGRATIONS_DIR}/{file_name} changed after it was journaled. \
                     A merged migration is immutable; add a new migration instead of refreshing the hash."
                );
            }
            continue;
        }
        let bytes = std::fs::read(path)?;
        journal.migrations.push(Entry {
            file: file_name,
            sha256: sha256_hex(&bytes),
        });
        added += 1;
    }

    journal.migrations.sort_by(|a, b| a.file.cmp(&b.file));

    let mut text = String::from(
        "# Migration journal. Generated by `cargo xtask check-migrations --update`; reviewed in diffs.\n\
         #\n\
         # An entry records what was merged, so editing a merged migration fails CI. To change\n\
         # the schema, add a new migration file and record it — never edit an entry by hand.\n",
    );
    for entry in &journal.migrations {
        text.push_str("\n[[migration]]\nfile = \"");
        text.push_str(&entry.file);
        text.push_str("\"\nsha256 = \"");
        text.push_str(&entry.sha256);
        text.push_str("\"\n");
    }
    std::fs::write(root.join(JOURNAL_FILE), text)
        .with_context(|| format!("cannot write {JOURNAL_FILE}"))?;
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(root: &Path, name: &str, body: &str) {
        let dir = root.join(MIGRATIONS_DIR);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn empty_tree_is_clean() {
        let tmp = TempDir::new().unwrap();
        let (report, checked) = check(tmp.path()).unwrap();
        assert!(report.is_clean(), "{:#?}", report.sorted());
        assert_eq!(checked, 0);
    }

    #[test]
    fn unjournaled_migration_is_rejected() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "0001_core.sql", "CREATE TABLE t (id INTEGER);");
        let (report, _) = check(tmp.path()).unwrap();
        assert!(
            report
                .sorted()
                .iter()
                .any(|v| v.message.contains("not listed in the journal")),
            "{:#?}",
            report.sorted()
        );
    }

    #[test]
    fn update_then_check_is_clean() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "0001_core.sql", "CREATE TABLE t (id INTEGER);");
        write(tmp.path(), "0002_more.sql", "CREATE TABLE u (id INTEGER);");
        assert_eq!(update(tmp.path()).unwrap(), 2);
        let (report, checked) = check(tmp.path()).unwrap();
        assert!(report.is_clean(), "{:#?}", report.sorted());
        assert_eq!(checked, 2);
    }

    #[test]
    fn editing_a_journaled_migration_is_rejected() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "0001_core.sql", "CREATE TABLE t (id INTEGER);");
        update(tmp.path()).unwrap();
        write(
            tmp.path(),
            "0001_core.sql",
            "CREATE TABLE t (id INTEGER, extra TEXT);",
        );
        let (report, _) = check(tmp.path()).unwrap();
        assert!(
            report
                .sorted()
                .iter()
                .any(|v| v.message.contains("content changed after merge")),
            "{:#?}",
            report.sorted()
        );
    }

    #[test]
    fn update_refuses_to_launder_an_edited_migration() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "0001_core.sql", "CREATE TABLE t (id INTEGER);");
        update(tmp.path()).unwrap();
        write(
            tmp.path(),
            "0001_core.sql",
            "CREATE TABLE t (id INTEGER, extra TEXT);",
        );
        assert!(
            update(tmp.path()).is_err(),
            "update must refuse to rewrite a changed entry"
        );
    }

    #[test]
    fn gap_in_numbering_is_rejected() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "0001_core.sql", "SELECT 1;");
        write(tmp.path(), "0003_skipped.sql", "SELECT 3;");
        update(tmp.path()).unwrap();
        let (report, _) = check(tmp.path()).unwrap();
        assert!(
            report
                .sorted()
                .iter()
                .any(|v| v.message.contains("out of sequence")),
            "{:#?}",
            report.sorted()
        );
    }

    #[test]
    fn bad_file_name_is_rejected() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "initial.sql", "SELECT 1;");
        let (report, _) = check(tmp.path()).unwrap();
        assert!(
            report
                .sorted()
                .iter()
                .any(|v| v.message.contains("NNNN_snake_case.sql")),
            "{:#?}",
            report.sorted()
        );
    }

    #[test]
    fn journal_entry_without_file_is_rejected() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "0001_core.sql", "SELECT 1;");
        update(tmp.path()).unwrap();
        fs::remove_file(tmp.path().join(MIGRATIONS_DIR).join("0001_core.sql")).unwrap();
        let (report, _) = check(tmp.path()).unwrap();
        assert!(
            report
                .sorted()
                .iter()
                .any(|v| v.message.contains("does not exist")),
            "{:#?}",
            report.sorted()
        );
    }
}
