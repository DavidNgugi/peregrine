//! `check-offset`: a list query is keyset-paginated, so no query contains `OFFSET`.
//!
//! AGENTS §5 states the rule and this gate is what makes it hold: an offset page shifts
//! under a write, because a row inserted between two reads pushes a row out of the second
//! page, and on a table the single writer is appending to that is the ordinary case.
//!
//! The gate reads SQL, not prose. In a `.sql` file everything outside a line comment and a
//! string literal is SQL; in a `.rs` file only the contents of string literals are, which
//! is why a function may be named `offset_for`, a column `offset_ms`, and a doc comment may
//! discuss paging, without any of them being a finding. That distinction is the difference
//! between a gate people keep and a gate people work around.

use crate::report::{Report, Violation};
use anyhow::{Context, Result};
use std::path::Path;

/// The clause this gate keeps out of queries.
const KEYWORD: &str = "offset";

const SOURCE_EXTENSIONS: [&str; 2] = ["rs", "sql"];

const SKIP_DIRS: [&str; 6] = [
    "target",
    "node_modules",
    ".git",
    "dist",
    "vendor",
    // The gates themselves: their fixtures are the negative cases the gates have to
    // reject, so scanning them would fail the repository on its own test data. `xtask`
    // never ships and holds no product query.
    "xtask",
];

/// Checks every SQL-bearing source under `root`.
///
/// Returns the report and how many files held SQL, so a green run says what it read rather
/// than only that it found nothing.
pub fn check(root: &Path) -> Result<(Report, usize)> {
    let mut report = Report::new();
    let mut checked = 0;
    let mut pending = vec![root.to_path_buf()];

    while let Some(next) = pending.pop() {
        if next
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| SKIP_DIRS.contains(&name))
        {
            continue;
        }
        // A directory that cannot be read is not this gate's finding, and a fixture tree
        // that holds only one file has none of these.
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries {
            let path = entry.context("cannot read a directory entry")?.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
                continue;
            };
            if !SOURCE_EXTENSIONS.contains(&extension) {
                continue;
            }
            let source = std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            let relative = relative_path(root, &path);
            let found = segments(&source, Syntax::of(extension));
            if found.is_empty() {
                continue;
            }
            checked += 1;
            for segment in &found {
                for (offset, line) in segment.text.lines().enumerate() {
                    if let Some(column) = keyword_column(line, KEYWORD) {
                        report.push(Violation::new(
                            format!("{relative}:{}", segment.line + offset),
                            format!(
                                "`{KEYWORD}` at column {column} is an offset page; list queries \
                                 are keyset-paginated through `PageRequest` (AGENTS §5)"
                            ),
                        ));
                    }
                }
            }
        }
    }

    Ok((report, checked))
}

/// Where a line's SQL comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Syntax {
    /// The file is SQL; everything outside comments and string literals counts.
    Sql,
    /// The file is Rust; only the contents of string literals count.
    Rust,
}

impl Syntax {
    fn of(extension: &str) -> Self {
        if extension == "sql" {
            Self::Sql
        } else {
            Self::Rust
        }
    }
}

/// One piece of SQL and the line it begins on.
#[derive(Debug, PartialEq, Eq)]
struct Segment {
    line: usize,
    text: String,
}

/// Every piece of SQL in `source`.
///
/// The two syntaxes differ in where SQL lives. A `.sql` file is SQL from the first line to
/// the last, so a segment is one line with its comments and string literals removed. A
/// `.rs` file holds SQL in string literals, so a segment is one literal — and only a literal
/// that reads like a statement is kept, which is what stops `"check-offset"` or a settings
/// key called `"offset"` from being findings.
///
/// State is carried across lines because both syntaxes can span one: a Rust literal
/// continued with a trailing backslash, and a block comment.
fn segments(source: &str, syntax: Syntax) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut current = Segment {
        line: 1,
        text: String::new(),
    };
    let mut line = 1;
    let mut string = false;
    let mut line_comment = false;
    let mut block_comment = false;
    let mut characters = source.chars().peekable();

    while let Some(character) = characters.next() {
        if character == '\n' {
            if syntax == Syntax::Sql {
                if !current.text.trim().is_empty() {
                    segments.push(current);
                }
                current = Segment {
                    line: line + 1,
                    text: String::new(),
                };
            } else if string {
                // A continued literal keeps the newline so offsets stay per line.
                current.text.push('\n');
            }
            line += 1;
            line_comment = false;
            continue;
        }
        if line_comment {
            continue;
        }
        if block_comment {
            if character == '*' && characters.peek() == Some(&'/') {
                characters.next();
                block_comment = false;
            }
            continue;
        }
        if string {
            match (syntax, character) {
                // A line continuation escapes nothing; the newline above still ends the
                // line for reporting, and the literal stays open.
                (Syntax::Rust, '\\') => {
                    if characters.peek() != Some(&'\n')
                        && let Some(escaped) = characters.next()
                    {
                        current.text.push(escaped);
                    }
                }
                (Syntax::Rust, '"') => {
                    string = false;
                    if looks_like_statement(&current.text) {
                        segments.push(std::mem::replace(
                            &mut current,
                            Segment {
                                line: line + 1,
                                text: String::new(),
                            },
                        ));
                    }
                }
                (Syntax::Rust, other) => current.text.push(other),
                // SQL has no backslash escapes; a doubled quote is how a quote is written.
                (Syntax::Sql, '\'') => string = false,
                (Syntax::Sql, _) => {}
            }
            continue;
        }

        match character {
            // `--` is SQL's line comment and `//` is Rust's, so each syntax takes the one
            // its language uses rather than both.
            '-' if syntax == Syntax::Sql && characters.peek() == Some(&'-') => {
                characters.next();
                line_comment = true;
            }
            '/' if syntax == Syntax::Rust && characters.peek() == Some(&'/') => {
                characters.next();
                line_comment = true;
            }
            '/' if characters.peek() == Some(&'*') => {
                characters.next();
                block_comment = true;
            }
            '"' if syntax == Syntax::Rust => {
                string = true;
                current = Segment {
                    line,
                    text: String::new(),
                };
            }
            '\'' if syntax == Syntax::Sql => string = true,
            other if syntax == Syntax::Sql => current.text.push(other),
            _ => {}
        }
    }

    if syntax == Syntax::Sql && !current.text.trim().is_empty() {
        segments.push(current);
    } else if syntax == Syntax::Rust && string && looks_like_statement(&current.text) {
        // An unterminated literal is a compiler error, but reporting what it holds is
        // better than silently ignoring the file.
        segments.push(current);
    }
    segments
}

/// Whether `text` reads like a statement rather than like a word that happens to occur in
/// one.
fn looks_like_statement(text: &str) -> bool {
    // Deliberately not "with": it is an ordinary English word, and a literal that merely
    // contains it would be scanned as if it were a query.
    ["select", "insert", "update", "delete", "create", "pragma"]
        .iter()
        .any(|keyword| keyword_column(text, keyword).is_some())
}

/// The column of `keyword` in `sql`, matched case-insensitively as a whole word.
///
/// The whole-word rule is what keeps `offset_ms` and `offset_for` out of the findings: an
/// underscore is an identifier character, so a keyword boundary is not there.
fn keyword_column(sql: &str, keyword: &str) -> Option<usize> {
    let haystack = sql.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(found) = haystack[search_from..].find(keyword) {
        let start = search_from + found;
        let end = start + keyword.len();
        let before_is_word = sql[..start]
            .chars()
            .next_back()
            .is_some_and(is_identifier_character);
        let after_is_word = sql[end..]
            .chars()
            .next()
            .is_some_and(is_identifier_character);
        if !before_is_word && !after_is_word {
            return Some(start + 1);
        }
        search_from = end;
    }
    None
}

fn is_identifier_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// The path as a report shows it: relative to the root, with a forward slash.
fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Checks one source file written into a fixture tree.
    fn check_source(name: &str, source: &str) -> (Report, usize) {
        let root = TempDir::new().unwrap();
        std::fs::write(root.path().join(name), source).unwrap();
        check(root.path()).unwrap()
    }

    /// The rule itself: a query with an offset page is a finding, in both syntaxes.
    #[test]
    fn an_offset_clause_is_a_violation_in_sql_and_in_rust() {
        let (report, checked) = check_source(
            "query.sql",
            "SELECT id FROM job ORDER BY id DESC LIMIT 20 OFFSET 40;\n",
        );
        assert_eq!(report.len(), 1);
        assert_eq!(checked, 1);
        assert!(
            report.sorted()[0].path.ends_with("query.sql:1"),
            "the finding must name the line: {:?}",
            report.sorted()[0]
        );

        let (report, _) = check_source(
            "query.rs",
            "let sql = \"SELECT id FROM job ORDER BY id DESC LIMIT ?1 OFFSET ?2\";\n",
        );
        assert_eq!(report.len(), 1);
        assert!(report.sorted()[0].message.contains("offset page"));
    }

    /// The false positives that would make the gate unusable: an identifier, a Rust
    /// binding, a documentation comment, and a SQL string value.
    #[test]
    fn identifiers_comments_and_values_are_not_findings() {
        for (name, source) in [
            ("schema.sql", "CREATE TABLE t (offset_ms INTEGER);\n"),
            ("query.rs", "let offset = offset_for(&values, raw.len());\n"),
            (
                "query.rs",
                "/// A cursor is an id rather than an offset.\nlet x = 1;\n",
            ),
            ("query.rs", "// OFFSET is banned by AGENTS §5\nlet x = 1;\n"),
            ("query.rs", "/* never use OFFSET */\nlet x = 1;\n"),
            ("seed.sql", "INSERT INTO t (note) VALUES ('OFFSET 40');\n"),
            ("seed.sql", "-- LIMIT 20 OFFSET 40\nSELECT 1;\n"),
            // A prose literal that mentions paging is not a query.
            (
                "notes.rs",
                "let help = \"a cursor with an offset is not used\";\n",
            ),
        ] {
            let (report, _) = check_source(name, source);
            assert!(
                report.is_clean(),
                "{name} must not be a finding: {:?}",
                report.sorted()
            );
        }
    }

    /// A query continued across lines is still one query, in both syntaxes.
    #[test]
    fn a_continued_query_is_still_read() {
        let (report, _) = check_source(
            "query.rs",
            "let sql = \"SELECT id FROM job \\\n           ORDER BY id DESC \\\n           LIMIT ?1 OFFSET ?2\";\n",
        );
        assert_eq!(report.len(), 1);
        assert!(
            report.sorted()[0].path.ends_with("query.rs:3"),
            "the finding must name the line the clause is on: {:?}",
            report.sorted()[0]
        );

        let (report, _) = check_source(
            "query.sql",
            "SELECT id\n  FROM job\n ORDER BY id DESC\n LIMIT ?1 /* comment */ OFFSET ?2;\n",
        );
        assert_eq!(report.len(), 1);
    }

    /// The empty cases: no files, and files with no SQL in them.
    #[test]
    fn a_tree_with_nothing_to_read_is_clean() {
        let root = TempDir::new().unwrap();
        let (report, checked) = check(root.path()).unwrap();
        assert!(report.is_clean());
        assert_eq!(checked, 0);

        let (report, checked) = check_source("README.md", "OFFSET 40\n");
        assert!(report.is_clean(), "only SQL-bearing extensions are read");
        assert_eq!(checked, 0);
    }

    /// A case-insensitive match, because SQL keywords are not case-sensitive either.
    #[test]
    fn the_keyword_is_matched_case_insensitively() {
        let (report, _) = check_source("query.sql", "select id from job offset 40;\n");
        assert_eq!(report.len(), 1);
    }

    /// A partial identifier that merely contains the keyword is not a clause.
    #[test]
    fn a_longer_identifier_is_not_a_match() {
        assert_eq!(keyword_column("SELECT offset_ms FROM t", "offset"), None);
        assert_eq!(keyword_column("SELECT xoffset FROM t", "offset"), None);
        assert_eq!(keyword_column("SELECT offsetting FROM t", "offset"), None);
        assert_eq!(keyword_column("LIMIT 20 OFFSET 40", "offset"), Some(10));
        assert_eq!(keyword_column("", "offset"), None);
    }
}
