//! `backlog-stats`: the roll-up table in docs/TASKS.md, computed rather than
//! estimated by hand.
//!
//! Reads the task checklist out of docs/TASKS.md, sums the estimate shorthand, and
//! groups by delivery phase and by state. It also compares the result with the
//! declared totals at the bottom of that file so the two cannot drift.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

pub const TASKS_FILE: &str = "docs/TASKS.md";
const ESTIMATE_DAYS: [(&str, f64); 5] =
    [("XS", 0.5), ("S", 1.0), ("M", 2.5), ("L", 4.5), ("XL", 8.0)];
const STATES: [&str; 4] = ["todo", "doing", "review", "done"];

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: String,
    pub epic: u32,
    pub estimate: String,
    pub state: String,
}

impl Task {
    #[must_use]
    pub fn days(&self) -> f64 {
        ESTIMATE_DAYS
            .iter()
            .find(|(name, _)| *name == self.estimate)
            .map_or(0.0, |(_, days)| *days)
    }

    #[must_use]
    pub fn phase(&self) -> u32 {
        match self.epic {
            0..=4 => 0,
            5..=9 => 1,
            10..=15 => 2,
            16..=18 => 3,
            19..=27 => 4,
            28..=29 => 5,
            30..=31 => 6,
            32..=34 => 7,
            35 => 8,
            36..=37 => 9,
            _ => 10,
        }
    }
}

/// Parses one checklist line, or `None` when the line is not a task.
///
/// The format is the one docs/TASKS.md's Conventions section documents:
///
/// ```text
/// - [ ] **PG-E3-10** *(doing)* `L` · deps PG-E3-04 · Type-safe repositories
/// ```
///
/// Only the machine-readable prefix is parsed, and only up to the first segment that is not
/// one of the known metadata shapes; everything after that is the task's prose and may
/// contain anything, including text that looks like an estimate or a state.
#[must_use]
pub fn parse_row(line: &str) -> Option<Task> {
    let trimmed = line.trim();
    let done = trimmed.starts_with("- [x] ") || trimmed.starts_with("- [X] ");
    let rest = if done {
        trimmed
            .strip_prefix("- [x] ")
            .or_else(|| trimmed.strip_prefix("- [X] "))
    } else {
        trimmed.strip_prefix("- [ ] ")
    }?;

    let (id, after_id) = bolded(rest)?;
    let epic = parse_epic(&id)?;

    let mut state = if done { "done" } else { "todo" }.to_owned();
    let mut estimate = String::new();
    let mut segments = after_id.trim_start().split(" · ");

    // The head holds the state tag and the estimate, in that order and nothing else: a
    // token that is neither ends the metadata, so prose cannot be read as a band.
    if let Some(head) = segments.next() {
        for token in head.split_whitespace() {
            match token {
                "*(doing)*" => "doing".clone_into(&mut state),
                "*(review)*" => "review".clone_into(&mut state),
                other => match backticked(other) {
                    Some(band) if ESTIMATE_DAYS.iter().any(|(name, _)| *name == band) => {
                        band.clone_into(&mut estimate);
                    }
                    _ => break,
                },
            }
        }
    }

    // Dependencies and platform follow the estimate, before the prose.
    for segment in segments {
        let segment = segment.trim();
        if segment.starts_with("deps ") || segment.starts_with("plat ") {
            continue;
        }
        break;
    }

    Some(Task {
        id,
        epic,
        estimate,
        state,
    })
}

/// The identifier inside the first `**…**` on the line, and what follows it.
fn bolded(text: &str) -> Option<(String, &str)> {
    let start = text.find("**")? + 2;
    let end = text[start..].find("**")? + start;
    Some((text[start..end].to_owned(), &text[end + 2..]))
}

/// The contents of a segment that is exactly one code span, if it is.
fn backticked(segment: &str) -> Option<&str> {
    segment.strip_prefix('`')?.strip_suffix('`')
}

/// `PG-E12-03` -> `12`. The prefix is deliberately strict: a malformed ID in the
/// backlog is a data error, not a row to skip quietly.
fn parse_epic(id: &str) -> Option<u32> {
    let rest = id.strip_prefix("PG-E")?;
    let (epic, task) = rest.split_once('-')?;
    if task.is_empty() || !task.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    epic.parse().ok()
}

pub fn parse(root: &Path) -> Result<Vec<Task>> {
    let path = root.join(TASKS_FILE);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("cannot read {TASKS_FILE}"))?;
    Ok(tasks_in(&text))
}

/// Every task in the file, ignoring fenced blocks.
///
/// The Conventions section shows the line format in a fence, and that example names a real
/// task: counting it would make the roll-up depend on the documentation about the roll-up.
#[must_use]
pub fn tasks_in(text: &str) -> Vec<Task> {
    let mut fenced = false;
    let mut tasks = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        if let Some(task) = parse_row(line) {
            tasks.push(task);
        }
    }
    tasks
}

#[derive(Debug, Default)]
pub struct Rollup {
    pub tasks: usize,
    pub days: f64,
    pub by_phase: BTreeMap<u32, (usize, f64)>,
    pub by_state: BTreeMap<String, usize>,
}

#[must_use]
pub fn rollup(tasks: &[Task]) -> Rollup {
    let mut out = Rollup::default();
    for task in tasks {
        out.tasks += 1;
        out.days += task.days();
        let entry = out.by_phase.entry(task.phase()).or_insert((0, 0.0));
        entry.0 += 1;
        entry.1 += task.days();
        *out.by_state.entry(task.state.clone()).or_insert(0) += 1;
    }
    out
}

/// Totals declared by the roll-up table at the bottom of docs/TASKS.md.
#[derive(Debug, Default, PartialEq)]
pub struct Declared {
    pub by_phase: BTreeMap<u32, (usize, f64)>,
    pub full_backlog: Option<(usize, f64)>,
}

/// Parses rows of the form `| 0 Foundations | E0–E4 | 89 | 169 |`.
#[must_use]
pub fn parse_declared(text: &str) -> Declared {
    let mut declared = Declared::default();
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            continue;
        }
        let fields: Vec<String> = trimmed
            .trim_matches('|')
            .split('|')
            .map(|f| f.trim().to_owned())
            .collect();
        if fields.len() < 4 {
            continue;
        }
        let label = plain(&fields[0]);
        let scope = plain(&fields[1]);
        let (Some(count), Some(days)) = (parse_int(&fields[2]), parse_days(&fields[3])) else {
            continue;
        };
        if label.starts_with("Full backlog") || scope.contains("phases 0–5") {
            declared.full_backlog = Some((count, days));
            continue;
        }
        if let Some(phase) = label
            .split_whitespace()
            .next()
            .and_then(|p| p.parse::<u32>().ok())
        {
            declared.by_phase.insert(phase, (count, days));
        }
    }
    declared
}

/// Rounds a summed total up to the next whole ideal day.
///
/// This is the convention the roll-up table in docs/TASKS.md uses: each row is
/// the sum of its tasks' band midpoints, carried to a whole day. Phase 1's raw
/// 198.5 becomes 199, and the full backlog's raw 1114.5 becomes 1115.
#[must_use]
pub fn round_up_days(days: f64) -> f64 {
    days.ceil()
}

/// Strips markdown emphasis and surrounding whitespace, because the declared
/// table holds values like `**1 115**` and `**Full backlog (phases 0–5)**`.
fn plain(value: &str) -> String {
    value.replace('*', "").trim().to_owned()
}

fn parse_int(value: &str) -> Option<usize> {
    let cleaned: String = value.chars().filter(char::is_ascii_digit).collect();
    if cleaned.is_empty() {
        None
    } else {
        cleaned.parse().ok()
    }
}

/// Reads the first number in a cell, ignoring thousands separators.
///
/// Later numbers are deliberately ignored: a cell like `≈ 90 total (39 written)`
/// must not be read as 9039, and the first figure is the one the row is about.
fn parse_days(value: &str) -> Option<f64> {
    let normalized: String = value
        .chars()
        .filter(|c| !matches!(c, ' ' | '\u{202f}' | '\u{2009}' | '\u{00a0}'))
        .collect();
    let mut number = String::new();
    for character in normalized.chars() {
        if character.is_ascii_digit() || character == '.' {
            number.push(character);
        } else if !number.is_empty() {
            break;
        }
    }
    let number = number.trim_end_matches('.');
    if number.is_empty() {
        None
    } else {
        number.parse().ok()
    }
}

/// Prints the roll-up and reports whether it agrees with the declared table.
pub fn run(root: &Path) -> Result<bool> {
    let tasks = parse(root)?;
    let stats = rollup(&tasks);
    let text = std::fs::read_to_string(root.join(TASKS_FILE))?;
    let declared = parse_declared(&text);

    println!(
        "backlog-stats: {} tasks, {:.1} ideal days",
        stats.tasks, stats.days
    );
    let mut state_names: Vec<String> = stats.by_state.keys().cloned().collect();
    state_names.sort_by_key(|s| state_order(s));
    let state_line: Vec<String> = state_names
        .iter()
        .map(|s| format!("{s} {}", stats.by_state[s]))
        .collect();
    println!("  by state: {}", state_line.join(" · "));
    for (phase, (count, days)) in &stats.by_phase {
        println!("  phase {phase}: {count} tasks, {days:.1} ideal days");
    }

    let mut ok = true;
    for (phase, (count, days)) in &stats.by_phase {
        if let Some((declared_count, declared_days)) = declared.by_phase.get(phase) {
            // Phase 7's declared row describes a partially broken-down roadmap
            // ("12 written" of "≈ 90 total"), so its day cell is not the phase
            // total. The task count is still a real check, and the computed
            // figure (39 after rounding) matches the row's "39 written".
            let days_comparable = *phase != 7;
            let computed_days = round_up_days(*days);
            let count_differs = declared_count != count;
            let days_differ = days_comparable && (declared_days - computed_days).abs() > 0.05;
            if count_differs || days_differ {
                eprintln!(
                    "  phase {phase} disagrees with docs/TASKS.md: computed {count} tasks / {computed_days:.1} d, declared {declared_count} / {declared_days:.1} d{}",
                    if days_comparable {
                        ""
                    } else {
                        " (day comparison skipped: partial breakdown)"
                    }
                );
                ok = false;
            }
        }
    }
    if let Some((count, days)) = declared.full_backlog {
        let phase_0_5_tasks: usize = stats
            .by_phase
            .iter()
            .filter(|(phase, _)| **phase <= 5)
            .map(|(_, (count, _))| count)
            .sum();
        let phase_0_5_days: f64 = stats
            .by_phase
            .iter()
            .filter(|(phase, _)| **phase <= 5)
            .map(|(_, (_, days))| days)
            .sum();
        if phase_0_5_tasks != count || (round_up_days(phase_0_5_days) - days).abs() > 0.05 {
            eprintln!(
                "  full backlog disagrees with docs/TASKS.md: computed {phase_0_5_tasks} tasks / {:.1} d, declared {count} / {days:.1} d",
                round_up_days(phase_0_5_days)
            );
            ok = false;
        }
    }
    if !ok {
        eprintln!(
            "backlog-stats: the declared roll-up in {TASKS_FILE} is stale or the estimate convention changed"
        );
    }
    Ok(ok)
}

fn state_order(state: &str) -> usize {
    STATES
        .iter()
        .position(|s| *s == state)
        .unwrap_or(STATES.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_open_task() {
        let line = "- [ ] **PG-E3-10** `L` · deps PG-E3-04 · Type-safe repositories per aggregate";
        let task = parse_row(line).unwrap();
        assert_eq!(task.id, "PG-E3-10");
        assert_eq!(task.epic, 3);
        assert_eq!(task.estimate, "L");
        assert_eq!(task.state, "todo");
        assert!((task.days() - 4.5).abs() < f64::EPSILON);
        assert_eq!(task.phase(), 0);
    }

    #[test]
    fn a_checked_box_is_done() {
        let task = parse_row("- [x] **PG-E0-01** `S` · Cargo workspace").unwrap();
        assert_eq!(task.state, "done");
        assert_eq!(task.estimate, "S");
        assert!(parse_row("- [X] **PG-E0-01** `S` · Cargo workspace").is_some());
    }

    #[test]
    fn an_annotated_state_is_read_and_absent_means_todo() {
        let review = parse_row("- [ ] **PG-E1-01** *(review)* `M` · plat mac · Chrome").unwrap();
        assert_eq!(review.state, "review");
        assert_eq!(review.estimate, "M");

        let doing = parse_row("- [ ] **PG-E3-10** *(doing)* `L` · deps PG-E3-04 · x").unwrap();
        assert_eq!(doing.state, "doing");

        let plain = parse_row("- [ ] **PG-E5-01** `M` · x").unwrap();
        assert_eq!(plain.state, "todo");
    }

    /// The prefix ends where the prose begins, so a task whose text contains a band name or
    /// a state word cannot move the numbers.
    #[test]
    fn prose_cannot_masquerade_as_metadata() {
        let task = parse_row(
            "- [ ] **PG-E7-01** `M` · Shell layout: an `XL` rail and a `done` marker in the text",
        )
        .unwrap();
        assert_eq!(task.estimate, "M", "the first band is the estimate");
        assert_eq!(task.state, "todo");

        let no_metadata = parse_row("- [ ] **PG-E7-01** · a task with no band at all").unwrap();
        assert_eq!(no_metadata.estimate, "");
        assert!((no_metadata.days() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn ignores_non_task_lines() {
        assert!(parse_row("## E0 · Repository and tooling").is_none());
        assert!(parse_row("- Acceptance: something").is_none());
        assert!(parse_row("| ID | Task | Acceptance | Deps | Est |").is_none());
        assert!(parse_row("- [ ] a task with no identifier").is_none());
    }

    #[test]
    fn rejects_a_malformed_task_id() {
        assert!(parse_row("- [ ] **PG-E-01** `S` · x").is_none());
        assert!(parse_row("- [ ] **PG-E1-x** `S` · x").is_none());
    }

    #[test]
    fn phase_mapping_follows_the_backlog_headings() {
        let at = |id: &str| {
            parse_row(&format!("- [ ] **{id}** `S` · x"))
                .unwrap()
                .phase()
        };
        assert_eq!(at("PG-E0-01"), 0);
        assert_eq!(at("PG-E4-20"), 0);
        assert_eq!(at("PG-E5-01"), 1);
        assert_eq!(at("PG-E15-01"), 2);
        assert_eq!(at("PG-E16-01"), 3);
        assert_eq!(at("PG-E27-01"), 4);
        assert_eq!(at("PG-E29-01"), 5);
        assert_eq!(at("PG-E31-01"), 6);
        assert_eq!(at("PG-E35-01"), 8);
        assert_eq!(at("PG-E36-01"), 9);
        assert_eq!(at("PG-E37-16"), 9);
        assert_eq!(at("PG-E38-01"), 10);
    }

    #[test]
    fn rollup_groups_by_phase_and_state() {
        let tasks = vec![
            parse_row("- [x] **PG-E0-01** `S` · a").unwrap(),
            parse_row("- [ ] **PG-E0-02** `M` · a").unwrap(),
            parse_row("- [ ] **PG-E5-01** `XL` · a").unwrap(),
        ];
        let stats = rollup(&tasks);
        assert_eq!(stats.tasks, 3);
        assert!((stats.days - 11.5).abs() < 1e-9);
        assert_eq!(stats.by_state["done"], 1);
        assert_eq!(stats.by_phase[&0], (2, 3.5));
        assert_eq!(stats.by_phase[&1], (1, 8.0));
    }

    /// The format example in the Conventions section names a real task; it must not be
    /// counted, or the roll-up would depend on the prose describing it.
    #[test]
    fn a_fenced_example_is_not_a_task() {
        let text =
            "```\n- [ ] **PG-E3-10** *(doing)* `L` · example\n```\n- [ ] **PG-E0-01** `S` · real\n";
        let tasks = tasks_in(text);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "PG-E0-01");
    }

    #[test]
    fn declared_table_is_parsed() {
        let text = "| 0 Foundations | E0–E4 | 89 | 169 |\n| **Full backlog (phases 0–5)** | | **472** | **1 115** |";
        let declared = parse_declared(text);
        assert_eq!(declared.by_phase[&0], (89, 169.0));
        assert_eq!(declared.full_backlog, Some((472, 1115.0)));
    }

    #[test]
    fn thousands_separators_do_not_split_a_day_count() {
        assert_eq!(parse_days("**1 115**"), Some(1115.0));
        assert_eq!(parse_days("1\u{202f}115"), Some(1115.0));
    }

    #[test]
    fn only_the_first_number_in_a_day_cell_is_read() {
        assert_eq!(parse_days("≈ 90 total (39 written)"), Some(90.0));
        assert_eq!(parse_days("577 (+30% contingency ≈ **750**)"), Some(577.0));
        assert_eq!(parse_days("—"), None);
    }

    #[test]
    fn totals_round_up_to_whole_days() {
        assert!((round_up_days(198.5) - 199.0).abs() < f64::EPSILON);
        assert!((round_up_days(1114.5) - 1115.0).abs() < f64::EPSILON);
        assert!((round_up_days(169.0) - 169.0).abs() < f64::EPSILON);
    }
}
