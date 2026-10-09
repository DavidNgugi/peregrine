//! Violation reporting shared by the gates.

use std::fmt;

/// One thing a gate found wrong, with enough context to fix it without a debugger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Repository-relative path the violation is attached to.
    pub path: String,
    /// What is wrong, stated as the rule that was broken.
    pub message: String,
}

impl Violation {
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// A gate's outcome: the violations it found, in a deterministic order.
#[derive(Debug, Default, Clone)]
pub struct Report {
    violations: Vec<Violation>,
}

impl Report {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, violation: Violation) {
        self.violations.push(violation);
    }

    pub fn extend(&mut self, violations: impl IntoIterator<Item = Violation>) {
        self.violations.extend(violations);
    }

    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.violations.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.violations.is_empty()
    }

    /// Violations sorted by path then message, so two runs print identically.
    #[must_use]
    pub fn sorted(&self) -> Vec<&Violation> {
        let mut out: Vec<&Violation> = self.violations.iter().collect();
        out.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.message.cmp(&b.message)));
        out
    }
}

/// Prints a gate's outcome and maps it to a process exit code.
pub fn finish(name: &str, report: &Report, checked: usize) -> bool {
    if report.is_clean() {
        println!("{name}: ok ({checked} checked)");
        return true;
    }
    eprintln!("{name}: {} violation(s)", report.len());
    for violation in report.sorted() {
        eprintln!("  {violation}");
    }
    false
}
