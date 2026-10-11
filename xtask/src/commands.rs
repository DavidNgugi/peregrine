//! `check-commands`: an IPC command stays a parse, a call, and a map.
//!
//! ARCHITECTURE §7 rule 2 and AGENTS R10 fix the rule: a command is twenty lines or
//! fewer, because a longer one is a service that has not been extracted yet. This gate
//! counts the body of every `#[tauri::command]` function in `peregrine-ipc` and fails on
//! one that is over, which is the only way the rule survives contact with a new command.
//!
//! The count is taken from the source rather than from a macro expansion, so it works
//! without a Rust compiler and a fixture tree is enough to test it. Braces inside string
//! and character literals are ignored, and a lifetime is not mistaken for a character
//! literal, because a gate that reports a false violation teaches people to work around it.

use crate::report::{Report, Violation};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The most body lines a command may have (ARCHITECTURE §7 rule 2, AGENTS R10).
pub const MAX_BODY_LINES: usize = 20;

/// The crate whose command modules are scanned, relative to the repository root.
const COMMAND_CRATE: &str = "crates/peregrine-ipc/src";

/// One command found in the sources.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Command {
    /// 1-based line of the `fn` keyword, which is what the violation points at.
    line: usize,
    /// The function's name.
    name: String,
    /// How many lines are between the body's braces.
    body_lines: usize,
}

/// Checks every command in `root`.
///
/// Returns the report and how many commands were measured, so a green run states what it
/// looked at rather than only that it found nothing.
pub fn check(root: &Path) -> Result<(Report, usize)> {
    let mut report = Report::new();
    let mut checked = 0;

    for path in sources(&root.join(COMMAND_CRATE))? {
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let relative = relative_path(root, &path);
        for command in commands_in(&source) {
            checked += 1;
            if command.body_lines > MAX_BODY_LINES {
                report.push(Violation::new(
                    format!("{relative}:{}", command.line),
                    format!(
                        "`{}` has a {} line body; a command parses, calls one service, and maps \
                         the result in {MAX_BODY_LINES} lines or fewer — move the rest into a \
                         service (ARCHITECTURE §7 rule 2)",
                        command.name, command.body_lines
                    ),
                ));
            }
        }
    }

    Ok((report, checked))
}

/// Every Rust source under `directory`, in a deterministic order.
fn sources(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(next) = pending.pop() {
        // A tree with no such crate has no commands to check, which is not a failure: the
        // fixture tests run against trees that hold only the file under test.
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries {
            let path = entry.context("cannot read a directory entry")?.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// The path as the report shows it: relative to the repository root, with a forward slash.
fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Every `#[tauri::command]` function in one source file.
fn commands_in(source: &str) -> Vec<Command> {
    const ATTRIBUTE: &str = "#[tauri::command]";
    /// How far after the attribute the `fn` keyword may be, so a malformed file cannot
    /// make the scan walk the whole file.
    const SIGNATURE_WINDOW: usize = 8;

    let lines: Vec<&str> = source.lines().collect();
    let mut commands = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        if !lines[index].trim_start().starts_with(ATTRIBUTE) {
            index += 1;
            continue;
        }

        let Some(declaration) = (index..lines.len().min(index + SIGNATURE_WINDOW))
            .find(|line| lines[*line].contains("fn "))
        else {
            index += 1;
            continue;
        };
        let name = function_name(lines[declaration]);

        // The body's opening brace is the first `{` in the signature: generics use angle
        // brackets and a where clause has none, so nothing else can open the body.
        let opening = lines
            .iter()
            .enumerate()
            .skip(declaration)
            .take(SIGNATURE_WINDOW)
            .find(|(_, line)| code_only(line).contains('{'))
            .map(|(line, _)| line);
        let Some(opening) = opening else {
            index = declaration + 1;
            continue;
        };

        let Some(closing) = closing_brace(&lines, opening) else {
            index = declaration + 1;
            continue;
        };

        commands.push(Command {
            line: declaration + 1,
            name,
            body_lines: closing.saturating_sub(opening + 1),
        });
        index = closing + 1;
    }

    commands
}

/// The name after the `fn` keyword on a declaration line.
fn function_name(line: &str) -> String {
    let after = line.split("fn ").nth(1).unwrap_or_default();
    after
        .trim_start()
        .chars()
        .take_while(|character| character.is_alphanumeric() || *character == '_')
        .collect()
}

/// The line that closes the body opened on `opening`.
///
/// Depth is counted over `{` and `}` outside literals, which is what makes a
/// `format!("{{")` inside a command not look like a nested block.
fn closing_brace(lines: &[&str], opening: usize) -> Option<usize> {
    let mut depth = 0_i32;
    for (offset, line) in lines[opening..].iter().enumerate() {
        for character in code_only(line).chars() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(opening + offset);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// A line with the contents of its string and character literals removed.
///
/// A lifetime (`&'a str`, `<'a>`) is not a character literal: treating one as a literal
/// would swallow the rest of the signature, including the brace that opens the body.
fn code_only(line: &str) -> String {
    let mut code = String::with_capacity(line.len());
    let mut characters = line.chars().peekable();

    while let Some(character) = characters.next() {
        match character {
            '"' => {
                code.push('"');
                loop {
                    match characters.next() {
                        Some('\\') => {
                            characters.next();
                        }
                        Some('"') | None => break,
                        Some(_) => {}
                    }
                }
                code.push('"');
            }
            '\'' => {
                let mut lookahead = characters.clone();
                let is_character = match lookahead.next() {
                    Some('\\') => true,
                    Some(_) => lookahead.next() == Some('\''),
                    None => false,
                };
                code.push('\'');
                if is_character {
                    loop {
                        match characters.next() {
                            Some('\\') => {
                                characters.next();
                            }
                            Some('\'') | None => break,
                            Some(_) => {}
                        }
                    }
                }
            }
            '/' if characters.peek() == Some(&'/') => break,
            other => code.push(other),
        }
    }

    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;
    use tempfile::TempDir;

    /// Writes one source file into a fixture tree and checks it.
    fn check_source(source: &str) -> (Report, usize) {
        let root = TempDir::new().unwrap();
        let directory = root.path().join(COMMAND_CRATE);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("commands.rs"), source).unwrap();
        check(root.path()).unwrap()
    }

    /// A command whose body is exactly `lines` lines.
    fn command_with_body(name: &str, lines: usize) -> String {
        let mut source = String::from("#[tauri::command]\n#[specta::specta]\npub async fn ");
        source.push_str(name);
        source.push_str("() -> Result<(), AppError> {\n");
        for index in 0..lines {
            let _ = writeln!(source, "    let _ = {index};");
        }
        source.push_str("}\n");
        source
    }

    /// The boundary: exactly the limit passes, one line more fails.
    #[test]
    fn the_limit_is_inclusive() {
        let (report, checked) = check_source(&command_with_body("at_limit", MAX_BODY_LINES));
        assert!(report.is_clean(), "{:?}", report.sorted());
        assert_eq!(checked, 1);

        let (report, checked) = check_source(&command_with_body("over_limit", MAX_BODY_LINES + 1));
        assert_eq!(report.len(), 1);
        assert_eq!(checked, 1);
        let violation = report.sorted()[0];
        assert!(
            violation.path.ends_with("commands.rs:3"),
            "the violation must point at the declaration: {}",
            violation.path
        );
        assert!(
            violation.message.contains("over_limit") && violation.message.contains("21 line body"),
            "the violation must name the command and the count: {}",
            violation.message
        );
    }

    /// The empty case: a tree with no commands is clean, not a failure.
    #[test]
    fn a_tree_with_no_commands_is_clean() {
        let (report, checked) = check_source("pub fn helper() -> u32 {\n    1\n}\n");
        assert!(report.is_clean());
        assert_eq!(checked, 0);
    }

    /// Braces inside literals are not the body's braces.
    #[test]
    fn braces_in_literals_do_not_change_the_count() {
        let source = "\
#[tauri::command]
pub async fn braces() -> Result<(), AppError> {
    let open = \"{\";
    let close = \"}\";
    let nested = format!(\"{{{open}{close}}}\");
    let character = '}';
    consume(open, close, nested, character);
}
";
        let (report, checked) = check_source(source);
        assert!(report.is_clean(), "{:?}", report.sorted());
        assert_eq!(checked, 1);
        // Five body lines, which a naive brace count would always get wrong.
        assert_eq!(commands_in(source)[0].body_lines, 5);
    }

    /// A lifetime in a signature must not be mistaken for a character literal, which would
    /// hide the brace that opens the body.
    #[test]
    fn a_lifetime_in_the_signature_does_not_hide_the_body() {
        let source = "\
#[tauri::command]
pub fn with_lifetime<'a>(value: &'a str) -> Result<(), AppError> {
    let _ = value;
}
";
        let commands = commands_in(source);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].name, "with_lifetime");
        assert_eq!(commands[0].body_lines, 1);
    }

    /// A command never closed is skipped rather than reported as a violation: an
    /// unparseable file is a compiler error, not a style finding.
    #[test]
    fn an_unterminated_command_is_not_a_violation() {
        let (report, checked) =
            check_source("#[tauri::command]\npub fn broken() -> Result<(), E> {\n");
        assert!(report.is_clean(), "{:?}", report.sorted());
        assert_eq!(checked, 0);
    }

    /// Two commands in one file are both measured, and the second one's line is right.
    #[test]
    fn every_command_in_a_file_is_measured() {
        let mut source = command_with_body("first", 1);
        source.push('\n');
        source.push_str(&command_with_body("second", MAX_BODY_LINES + 2));

        let (report, checked) = check_source(&source);
        assert_eq!(checked, 2);
        assert_eq!(report.len(), 1);
        assert!(report.sorted()[0].message.contains("second"));
    }
}
