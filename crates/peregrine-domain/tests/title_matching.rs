//! Measures the title matching rule against the committed corpus.
//!
//! The corpus in `data/title_matching.json` is ours: every case is a provider candidate and the local
//! titles a catalogue might already hold, and every expected decision was written from the rule
//! `matching.rs` documents rather than read back out of the implementation. That is the whole value of
//! a corpus — a case whose expectation was copied from the code measures nothing.
//!
//! Unlike the parser's corpus next door, this one has no accuracy floor: the rule is deterministic and
//! the cases are constructed, so **every** case is expected to agree. A disagreement is either a bug in
//! the rule or a mistake in the corpus, and both are worth failing for.
//!
//! Every failing case is reported, not the first: one run tells a reader the whole state of the corpus.

use std::collections::BTreeSet;

use peregrine_domain::catalogue::TitleKind;
use peregrine_domain::id::{ExternalId, Provider};
use peregrine_domain::matching::{
    Candidate, LocalTitle, MatchBasis, MatchDecision, match_candidate,
};
use peregrine_domain::text::NonEmptyText;
use serde::Deserialize;

const CORPUS: &str = include_str!("data/title_matching.json");

/// The acceptance's floor: "100-case matching corpus with expected outcomes".
const MINIMUM_CASES: usize = 100;

#[derive(Debug, Deserialize)]
struct Case {
    /// What the case establishes, read by a person when it fails.
    why: String,
    candidate: CandidateJson,
    #[serde(default)]
    locals: Vec<LocalJson>,
    expected: ExpectedJson,
}

#[derive(Debug, Deserialize)]
struct CandidateJson {
    provider: String,
    value: String,
    name: String,
    kind: String,
    #[serde(default)]
    released: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LocalJson {
    id: i64,
    name: String,
    kind: String,
    #[serde(default)]
    released: Option<String>,
    #[serde(default)]
    external_ids: Vec<ExternalRefJson>,
    #[serde(default)]
    aliases: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ExternalRefJson {
    provider: String,
    value: String,
}

/// The expected outcome, tagged by the decision it names.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "decision", rename_all = "snake_case")]
enum ExpectedJson {
    Matched { local_id: i64, basis: String },
    New,
    Ambiguous { local_ids: Vec<i64> },
}

impl ExpectedJson {
    /// The `MatchDecision` this expectation describes, or an error naming the bad basis.
    fn to_decision(&self) -> Result<MatchDecision, String> {
        match self {
            Self::New => Ok(MatchDecision::New),
            Self::Matched { local_id, basis } => {
                let basis = match basis.as_str() {
                    "external_id" => MatchBasis::ExternalId,
                    "name_and_year" => MatchBasis::NameAndYear,
                    "name" => MatchBasis::Name,
                    other => return Err(format!("unknown basis `{other}`")),
                };
                Ok(MatchDecision::Matched {
                    local_id: *local_id,
                    basis,
                })
            }
            Self::Ambiguous { local_ids } => Ok(MatchDecision::Ambiguous {
                local_ids: local_ids.clone(),
            }),
        }
    }

    /// A label for the failure line, so a reader sees what was expected without decoding a struct.
    fn label(&self) -> String {
        match self {
            Self::New => "new".to_owned(),
            Self::Matched { local_id, basis } => format!("matched {local_id} by {basis}"),
            Self::Ambiguous { local_ids } => format!("ambiguous {local_ids:?}"),
        }
    }
}

/// A required name, or an error naming the case that carried it.
fn name(value: &str, case: &str) -> Result<NonEmptyText, String> {
    NonEmptyText::new(value).map_err(|_| format!("{case}: `{value}` is empty"))
}

/// A provider's spelling, or an error naming the case.
fn provider(value: &str, case: &str) -> Result<Provider, String> {
    value
        .parse::<Provider>()
        .map_err(|_| format!("{case}: `{value}` is not a provider this build knows"))
}

/// An external id, or an error naming the case.
fn external(reference: &ExternalRefJson, case: &str) -> Result<ExternalId, String> {
    let provider = provider(&reference.provider, case)?;
    ExternalId::new(provider, &reference.value).map_err(|error| {
        format!(
            "{case}: `{}:{}` is not an external id: {error}",
            reference.provider, reference.value
        )
    })
}

/// A kind, or an error naming the case.
fn kind(value: &str, case: &str) -> Result<TitleKind, String> {
    TitleKind::parse(value).ok_or_else(|| format!("{case}: `{value}` is not a title kind"))
}

/// A date, or an error naming the case.
fn date(value: &str, case: &str) -> Result<jiff::civil::Date, String> {
    value
        .parse()
        .map_err(|_| format!("{case}: `{value}` is not an ISO date"))
}

impl Case {
    /// The case as the rule's own inputs, or an error naming what is malformed about it.
    fn inputs(&self) -> Result<(Candidate, Vec<LocalTitle>), String> {
        let label = &self.why;
        let candidate = Candidate {
            external_id: external(
                &ExternalRefJson {
                    provider: self.candidate.provider.clone(),
                    value: self.candidate.value.clone(),
                },
                label,
            )?,
            name: name(&self.candidate.name, label)?,
            kind: kind(&self.candidate.kind, label)?,
            released: self
                .candidate
                .released
                .as_deref()
                .map(|value| date(value, label))
                .transpose()?,
        };

        let mut locals = Vec::with_capacity(self.locals.len());
        for local in &self.locals {
            locals.push(LocalTitle {
                id: local.id,
                kind: kind(&local.kind, label)?,
                name: name(&local.name, label)?,
                released: local
                    .released
                    .as_deref()
                    .map(|value| date(value, label))
                    .transpose()?,
                external_ids: local
                    .external_ids
                    .iter()
                    .map(|reference| external(reference, label))
                    .collect::<Result<Vec<_>, _>>()?,
                aliases: local
                    .aliases
                    .iter()
                    .map(|alias| name(alias, label))
                    .collect::<Result<Vec<_>, _>>()?,
            });
        }
        Ok((candidate, locals))
    }
}

#[test]
fn the_matching_corpus_agrees_with_the_rule() {
    let cases: Vec<Case> = serde_json::from_str(CORPUS).expect("the corpus is valid JSON");

    assert!(
        cases.len() >= MINIMUM_CASES,
        "the corpus holds {} cases; the acceptance asks for at least {MINIMUM_CASES}",
        cases.len()
    );

    // A corpus that only ever says "new" would pass while measuring almost nothing, so the decisions
    // it covers are asserted rather than assumed.
    let mut covered: BTreeSet<&str> = BTreeSet::new();
    let mut failures: Vec<String> = Vec::new();

    for case in &cases {
        assert!(
            !case.why.trim().is_empty(),
            "a case has no `why`, so a failure could not be read"
        );
        covered.insert(match case.expected {
            ExpectedJson::New => "new",
            ExpectedJson::Matched { .. } => "matched",
            ExpectedJson::Ambiguous { .. } => "ambiguous",
        });

        let expected = match case.expected.to_decision() {
            Ok(decision) => decision,
            Err(problem) => {
                failures.push(format!("{}: {problem}", case.why));
                continue;
            }
        };
        let (candidate, locals) = match case.inputs() {
            Ok(inputs) => inputs,
            Err(problem) => {
                failures.push(problem);
                continue;
            }
        };

        let actual = match_candidate(&candidate, &locals);
        if actual != expected {
            failures.push(format!(
                "{}\n      expected {} and got {actual:?}",
                case.why,
                case.expected.label()
            ));
        }
    }

    assert!(
        covered.len() == 3,
        "the corpus covers {covered:?}; every decision needs at least one case"
    );
    assert!(
        failures.is_empty(),
        "{} of {} cases disagree with the rule:\n  - {}",
        failures.len(),
        cases.len(),
        failures.join("\n  - ")
    );
}
