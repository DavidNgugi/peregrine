//! `check-todos`: every deferred-work marker names the work item that owns it.
//!
//! An unreferenced marker is a note to nobody. Each marker must be followed by a
//! reference — a task ID (`PG-E3-01`) or an issue (`#123`) — so the deferral is
//! visible in the backlog instead of in a comment nobody reads again (AGENTS R30).
//!
//! Only comments are scanned. A marker word inside a string literal or an
//! identifier is not a deferral, which also keeps this gate from flagging its own
//! marker list.

use crate::report::{Report, Violation};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub const MARKERS: [&str; 4] = ["TODO", "FIXME", "XXX", "HACK"];

const SOURCE_EXTENSIONS: [&str; 13] = [
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "sql", "sh", "py", "yml", "yaml", "toml",
];

const SKIP_DIRS: [&str; 9] = [
    "target",
    "node_modules",
    ".git",
    "dist",
    "fixtures",
    "docs",
    "coverage",
    ".venv",
    "vendor",
];

fn comment_prefix(extension: &str) -> Option<&'static str> {
    match extension {
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => Some("//"),
        "sql" => Some("--"),
        "sh" | "py" | "yml" | "yaml" | "toml" => Some("#"),
        _ => None,
    }
}

/// The text of the comments on a line, joined. Handles both line-comment styles
/// used here; block comments are treated line by line, which is sufficient
/// because a marker never spans a line.
fn comments_on(line: &str, prefix: &str) -> Option<String> {
    match line.find(prefix) {
        Some(index) => {
            let mut text = line[index + prefix.len()..].to_owned();
            // A doc comment adds a third slash; strip decoration so the marker is
            // found at the start of the comment text.
            while text.starts_with('/') || text.starts_with('-') || text.starts_with('#') {
                text.remove(0);
            }
            Some(text)
        }
        None => None,
    }
}

/// A valid reference is a task ID or an issue number inside parentheses
/// immediately after the marker.
fn has_reference(after_marker: &str) -> bool {
    let rest = after_marker.trim_start();
    let Some(rest) = rest.strip_prefix('(') else {
        return false;
    };
    let Some((inner, _)) = rest.split_once(')') else {
        return false;
    };
    let inner = inner.trim();
    is_task_id(inner) || is_issue(inner)
}

fn is_task_id(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("PG-E") else {
        return false;
    };
    let Some((epic, task)) = rest.split_once('-') else {
        return false;
    };
    !epic.is_empty()
        && !task.is_empty()
        && epic.chars().all(|c| c.is_ascii_digit())
        && task.chars().all(|c| c.is_ascii_digit())
}

fn is_issue(value: &str) -> bool {
    value
        .strip_prefix('#')
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// Finds markers in a comment body and reports the ones with no reference.
fn violations_in_comment(path: &str, line_number: usize, comment: &str, report: &mut Report) {
    for marker in MARKERS {
        let mut search_from = 0_usize;
        while let Some(offset) = comment[search_from..].find(marker) {
            let index = search_from + offset;
            let before_is_word = comment[..index]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
            let after = &comment[index + marker.len()..];
            if !before_is_word && !has_reference(after) {
                let shown = after.trim();
                let shown = if shown.is_empty() {
                    "<end of line>"
                } else {
                    shown
                };
                report.push(Violation::new(
                    path,
                    format!(
                        "line {line_number}: `{marker}` has no reference; \
                         write `{marker}(PG-E0-00):` naming the task that owns it \
                         (found: {shown})"
                    ),
                ));
            }
            search_from = index + marker.len();
        }
    }
}

pub fn check(root: &Path) -> Result<(Report, usize)> {
    let mut report = Report::new();
    let mut files = Vec::new();
    collect(root, &mut files)?;
    let mut checked = 0_usize;
    for file in &files {
        let Some(extension) = file.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let Some(prefix) = comment_prefix(extension) else {
            continue;
        };
        let text = std::fs::read_to_string(file)
            .with_context(|| format!("cannot read {}", file.display()))?;
        checked += 1;
        let rel = file
            .strip_prefix(root)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        for (number, line) in text.lines().enumerate() {
            if let Some(comment) = comments_on(line, prefix) {
                violations_in_comment(&rel, number + 1, &comment, &mut report);
            }
        }
    }
    Ok((report, checked))
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        if path.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            collect(&path, out)?;
        } else if let Some(extension) = path.extension().and_then(|e| e.to_str())
            && SOURCE_EXTENSIONS.contains(&extension)
        {
            out.push(path);
        }
    }
    out.sort();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Built from parts so this test file does not itself contain a bare marker.
    fn bare_marker() -> String {
        format!("{} fix this later", ["TO", "DO"].concat())
    }

    fn referenced_marker() -> String {
        format!("{}(PG-E0-07): tracked", ["TO", "DO"].concat())
    }

    fn write(root: &Path, name: &str, body: &str) {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[test]
    fn bare_marker_is_rejected() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "src/lib.rs", &format!("// {}\n", bare_marker()));
        let (report, _) = check(tmp.path()).unwrap();
        assert_eq!(report.len(), 1, "{:#?}", report.sorted());
        assert!(report.sorted()[0].message.contains("has no reference"));
    }

    #[test]
    fn referenced_markers_are_accepted() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "src/lib.rs",
            &format!("// {}\n// #123 also fine\n", referenced_marker()),
        );
        let (report, _) = check(tmp.path()).unwrap();
        assert!(report.is_clean(), "{:#?}", report.sorted());
    }

    #[test]
    fn marker_in_a_string_literal_is_not_a_deferral() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "src/lib.rs",
            &format!("fn f() -> &'static str {{ \"{}\" }}\n", bare_marker()),
        );
        let (report, _) = check(tmp.path()).unwrap();
        assert!(report.is_clean(), "{:#?}", report.sorted());
    }

    #[test]
    fn sql_and_hash_comments_are_scanned() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "0001_x.sql", &format!("-- {}\n", bare_marker()));
        write(tmp.path(), "ci.yml", &format!("# {}\n", bare_marker()));
        let (report, _) = check(tmp.path()).unwrap();
        assert_eq!(report.len(), 2, "{:#?}", report.sorted());
    }

    #[test]
    fn fixtures_and_docs_are_skipped() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "fixtures/x.rs",
            &format!("// {}\n", bare_marker()),
        );
        write(tmp.path(), "docs/x.md", &format!("// {}\n", bare_marker()));
        let (report, checked) = check(tmp.path()).unwrap();
        assert!(report.is_clean(), "{:#?}", report.sorted());
        assert_eq!(checked, 0);
    }

    #[test]
    fn reference_formats_are_validated() {
        assert!(is_task_id("PG-E0-07"));
        assert!(!is_task_id("PG-E-07"));
        assert!(!is_task_id("PG-E0-"));
        assert!(is_issue("#42"));
        assert!(!is_issue("#"));
        assert!(!is_issue("42"));
    }
}
