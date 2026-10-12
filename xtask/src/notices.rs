//! `check-notices`: every bundled sidecar carries a recorded licence, a version
//! pin, a source URL, and a written offer of corresponding source, and
//! `THIRD_PARTY.md` is exactly what those declarations plus `Cargo.lock` produce.
//!
//! Why the attribution artefact is derived rather than hand-maintained: the
//! crates linked into the application are an output of the lockfile, so a written
//! list drifts the first time a dependency changes, and it drifts silently. The
//! three things no file in the tree can answer — which executables we
//! redistribute, what build each one uses, and where its corresponding source is
//! offered from — are declared by hand in `sidecars.toml` and validated here.
//!
//! The crate inventory is the non-development dependency closure of the shipped
//! binary, read textually from `Cargo.lock` so the gate stays offline and needs no
//! registry. `Cargo.lock` does not record edge kinds, so a crate that only a
//! third-party build script uses can remain in the list; over-attribution is
//! harmless, and every crate reached at runtime is present.

use crate::report::{Report, Violation};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

/// The hand-maintained sidecar declarations.
pub const DECLARATIONS: &str = "sidecars.toml";
/// The generated attribution artefact.
pub const ARTEFACT: &str = "THIRD_PARTY.md";
const LOCKFILE: &str = "Cargo.lock";
const MANIFEST: &str = "Cargo.toml";

/// The one workspace member that is redistributed. `xtask` and
/// `peregrine-testkit` are development tools, so the closure starts at the
/// shipped binary instead of at every member, whose dependencies never ship.
const SHIPPED_ROOT: &str = "peregrine-desktop";

/// Licences a sidecar may carry. Copyleft only ever leaves the process boundary
/// (AGENTS R25, `docs/adr/0009`), and these are the two `docs/DEPENDENCIES.md` §5
/// has reviewed; a third is a deliberate change to that document and to this list.
const SIDECAR_LICENCES: [&str; 2] = ["GPL-2.0-or-later", "LGPL-2.1-or-later"];

/// The version value meaning `docs/DEPENDENCIES.md` §1's rule: the release
/// records the exact version and SHA-256 for each platform. It is a pin, not a
/// range, and the gate accepts no range.
const PIN_PER_OS: &str = "pinned-per-os";

/// Whether the command checks the artefact or rewrites it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Fail when the artefact differs from the declarations and the lockfile.
    Check,
    /// Rewrite the artefact from the declarations and the lockfile.
    Update,
}

/// One bundled executable, as declared in [`DECLARATIONS`].
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sidecar {
    #[serde(default)]
    name: String,
    #[serde(default)]
    licence: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    source_offer: String,
    #[serde(default)]
    why_separate: String,
    #[serde(default)]
    build: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Declarations {
    #[serde(default)]
    sidecar: Vec<Sidecar>,
}

#[derive(Debug, Default, Deserialize)]
struct Lockfile {
    #[serde(default)]
    package: Vec<LockPackage>,
}

#[derive(Debug, Deserialize)]
struct LockPackage {
    name: String,
    version: String,
    /// Absent for workspace members, which are not third-party code.
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
}

/// One third-party crate linked into the application.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Crate {
    name: String,
    version: String,
}

/// Everything a check or an update works from.
struct Inventory {
    sidecars: Vec<Sidecar>,
    crates: Vec<Crate>,
    report: Report,
}

impl Inventory {
    fn checked(&self) -> usize {
        self.sidecars.len() + self.crates.len()
    }
}

/// Runs the gate or the generator. Returns whether the command succeeded.
pub fn run(root: &Path, mode: Mode) -> Result<bool> {
    if mode == Mode::Update {
        return update(root);
    }
    let (report, checked) = check(root)?;
    Ok(crate::report::finish("check-notices", &report, checked))
}

/// Checks the declarations and the artefact, returning the violations found and
/// the number of components checked (sidecars plus crates).
pub fn check(root: &Path) -> Result<(Report, usize)> {
    let inventory = inventory(root)?;
    let checked = inventory.checked();
    let mut report = inventory.report;
    // A stale artefact is only meaningful once its inputs are valid; rendering
    // one from an incomplete declaration would report drift the user cannot
    // act on until the declaration itself is fixed.
    if report.is_clean() {
        let rendered = render(&inventory.sidecars, &inventory.crates);
        compare_artefact(root, &rendered, &mut report);
    }
    Ok((report, checked))
}

fn update(root: &Path) -> Result<bool> {
    let inventory = inventory(root)?;
    if !inventory.report.is_clean() {
        return Ok(crate::report::finish(
            "check-notices",
            &inventory.report,
            inventory.checked(),
        ));
    }
    let text = render(&inventory.sidecars, &inventory.crates);
    let path = root.join(ARTEFACT);
    std::fs::write(&path, &text).with_context(|| format!("cannot write {}", path.display()))?;
    println!(
        "check-notices: wrote {ARTEFACT} ({} sidecar(s), {} crate(s))",
        inventory.sidecars.len(),
        inventory.crates.len()
    );
    Ok(true)
}

fn inventory(root: &Path) -> Result<Inventory> {
    let mut report = Report::new();
    let sidecars = declarations(root, &mut report)?;
    let crates = runtime_crates(root, &mut report)?;
    Ok(Inventory {
        sidecars,
        crates,
        report,
    })
}

/// Reads and validates [`DECLARATIONS`], reporting every missing field rather
/// than stopping at the first so one fix round is enough.
fn declarations(root: &Path, report: &mut Report) -> Result<Vec<Sidecar>> {
    let path = root.join(DECLARATIONS);
    if !path.is_file() {
        report.push(Violation::new(
            DECLARATIONS,
            "is missing; every bundled sidecar is declared here with its licence, \
             version pin, source, source offer, and build configuration",
        ));
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    Ok(parse_declarations(&text, report))
}

/// Validates declaration text. Split from the file read so the fixture tests can
/// exercise every required field without a fixture directory.
fn parse_declarations(text: &str, report: &mut Report) -> Vec<Sidecar> {
    let parsed: Declarations = match toml::from_str(text) {
        Ok(parsed) => parsed,
        Err(error) => {
            report.push(Violation::new(
                DECLARATIONS,
                format!("is not a readable sidecar declaration file: {error}"),
            ));
            return Vec::new();
        }
    };
    if parsed.sidecar.is_empty() {
        report.push(Violation::new(
            DECLARATIONS,
            "declares no sidecars; the application bundles mpv and ffmpeg/ffprobe, and an \
             empty declaration reads exactly like a complete one (docs/DEPENDENCIES.md §5)",
        ));
        return Vec::new();
    }

    let mut seen = BTreeSet::new();
    for sidecar in &parsed.sidecar {
        validate(sidecar, &mut seen, report);
    }
    parsed.sidecar
}

fn validate(sidecar: &Sidecar, seen: &mut BTreeSet<String>, report: &mut Report) {
    let label = if sidecar.name.trim().is_empty() {
        "an unnamed sidecar".to_owned()
    } else {
        format!("sidecar `{}`", sidecar.name)
    };
    let mut fail = |message: String| report.push(Violation::new(DECLARATIONS, message));

    if sidecar.name.trim().is_empty() {
        fail("a sidecar has no `name`".to_owned());
    } else if !seen.insert(sidecar.name.clone()) {
        fail(format!("{label} is declared more than once"));
    }

    if sidecar.licence.trim().is_empty() {
        fail(format!(
            "{label} has no recorded `licence`; a redistributed executable needs one \
             (docs/DEPENDENCIES.md §5)"
        ));
    } else if !SIDECAR_LICENCES.contains(&sidecar.licence.as_str()) {
        fail(format!(
            "{label} records licence `{}`, which is not one of the reviewed sidecar licences ({})",
            sidecar.licence,
            SIDECAR_LICENCES.join(", ")
        ));
    }

    if sidecar.version.trim().is_empty() {
        fail(format!(
            "{label} has no `version`; record the exact pin shared by every platform, or \
             `{PIN_PER_OS}` with the exact version and SHA-256 recorded per OS"
        ));
    } else if !is_pin(&sidecar.version) {
        fail(format!(
            "{label} records `version = \"{}\"`, which is not a pin; record an exact version \
             or `{PIN_PER_OS}`",
            sidecar.version
        ));
    }

    if !sidecar.source.starts_with("https://") || sidecar.source.len() <= "https://".len() {
        fail(format!(
            "{label} has no https `source` URL naming the exact upstream source"
        ));
    }

    if !sidecar.source_offer.contains("https://") {
        fail(format!(
            "{label} records no written offer of corresponding source; the offer must name the \
             distribution channel with an https URL (docs/adr/0013)"
        ));
    }

    if sidecar.why_separate.trim().is_empty() {
        fail(format!(
            "{label} records no `why_separate`; copyleft has to stay outside the process \
             boundary (AGENTS R25)"
        ));
    }

    if sidecar.build.trim().is_empty() {
        fail(format!(
            "{label} records no `build`; the build configuration is part of the source offer \
             (docs/DEPENDENCIES.md §5)"
        ));
    }
}

/// Whether `version` is an exact pin (`major.minor[.patch][-suffix]`) or the
/// per-OS rule. A caret, tilde, wildcard, or bare `latest` is not a pin.
fn is_pin(version: &str) -> bool {
    if version == PIN_PER_OS {
        return true;
    }
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let mut parts = core.split('.');
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let (Some(major), Some(minor)) = (parts.next(), parts.next()) else {
        return false;
    };
    digits(major) && digits(minor) && parts.all(digits)
}

/// Every third-party crate the shipped binary resolves to, excluding
/// development-only dependencies.
fn runtime_crates(root: &Path, report: &mut Report) -> Result<Vec<Crate>> {
    let members = member_runtime_dependencies(root)?;
    let lock = read_lock(root)?;

    let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, package) in lock.package.iter().enumerate() {
        by_name
            .entry(package.name.as_str())
            .or_default()
            .push(index);
    }

    let Some(roots) = members.get(SHIPPED_ROOT) else {
        report.push(Violation::new(
            MANIFEST,
            format!(
                "declares no workspace member `{SHIPPED_ROOT}`; the attribution list is derived \
                 from the shipped binary and would otherwise be empty"
            ),
        ));
        return Ok(Vec::new());
    };
    for dependency in roots {
        if !by_name.contains_key(dependency.as_str()) {
            report.push(Violation::new(
                LOCKFILE,
                format!(
                    "`{SHIPPED_ROOT}` declares runtime dependency `{dependency}`, which the \
                     lockfile does not contain; regenerate it with `cargo update`"
                ),
            ));
        }
    }

    let mut visited = BTreeSet::new();
    let mut queue: Vec<(String, Option<String>)> =
        roots.iter().map(|name| (name.clone(), None)).collect();
    while let Some((name, version)) = queue.pop() {
        for index in matching(&lock.package, &by_name, &name, version.as_deref()) {
            if !visited.insert(index) {
                continue;
            }
            let package = &lock.package[index];
            let Some(dependencies) = members.get(&package.name) else {
                if package.source.is_none() {
                    report.push(Violation::new(
                        LOCKFILE,
                        format!(
                            "`{}` is in the lockfile but is not a workspace member, so its \
                             development dependencies cannot be excluded from the attribution list",
                            package.name
                        ),
                    ));
                    continue;
                }
                // A registry crate's lockfile edges are its normal and build
                // dependencies; Cargo does not resolve its development ones.
                for dependency in &package.dependencies {
                    let mut tokens = dependency.split_whitespace();
                    let name = tokens.next().unwrap_or_default().to_owned();
                    let version = tokens.next().map(str::to_owned);
                    queue.push((name, version));
                }
                continue;
            };
            for dependency in dependencies {
                if !by_name.contains_key(dependency.as_str()) {
                    report.push(Violation::new(
                        LOCKFILE,
                        format!(
                            "`{}` declares runtime dependency `{dependency}`, which the lockfile \
                             does not contain; regenerate it with `cargo update`",
                            package.name
                        ),
                    ));
                }
                queue.push((dependency.clone(), None));
            }
        }
    }

    let mut crates: Vec<Crate> = visited
        .into_iter()
        .filter(|index| lock.package[*index].source.is_some())
        .map(|index| Crate {
            name: lock.package[index].name.clone(),
            version: lock.package[index].version.clone(),
        })
        .collect();
    crates.sort();
    crates.dedup();
    Ok(crates)
}

/// Lockfile indices for a dependency edge, filtered by version when Cargo
/// recorded one because the name is ambiguous across the graph.
fn matching(
    packages: &[LockPackage],
    by_name: &HashMap<&str, Vec<usize>>,
    name: &str,
    version: Option<&str>,
) -> Vec<usize> {
    by_name
        .get(name)
        .map(|indices| {
            indices
                .iter()
                .copied()
                .filter(|index| version.is_none_or(|wanted| packages[*index].version == wanted))
                .collect()
        })
        .unwrap_or_default()
}

/// Each workspace member's runtime dependency package names, read from its
/// manifest. The lockfile cannot substitute for this: a member's lockfile edges
/// include its development dependencies, which are not redistributed.
fn member_runtime_dependencies(root: &Path) -> Result<HashMap<String, Vec<String>>> {
    let manifest_path = root.join(MANIFEST);
    let text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("cannot read {}", manifest_path.display()))?;
    let manifest: toml::Value =
        toml::from_str(&text).with_context(|| format!("cannot parse {MANIFEST}"))?;
    let patterns: Vec<String> = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .with_context(|| format!("{MANIFEST} has no [workspace] members list"))?
        .iter()
        .filter_map(toml::Value::as_str)
        .map(str::to_owned)
        .collect();

    let mut out = HashMap::new();
    for member in crate::workspace::expand_members(root, &patterns)? {
        let mut names = Vec::new();
        for (kind, table) in crate::workspace::dependency_tables(&member.manifest) {
            if kind != "runtime" {
                continue;
            }
            let Some(entries) = table.as_table() else {
                continue;
            };
            for (name, spec) in entries {
                names.push(
                    spec.get("package")
                        .and_then(toml::Value::as_str)
                        .unwrap_or(name)
                        .to_owned(),
                );
            }
        }
        names.sort();
        names.dedup();
        out.insert(member.name, names);
    }
    Ok(out)
}

fn read_lock(root: &Path) -> Result<Lockfile> {
    let path = root.join(LOCKFILE);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("cannot parse {LOCKFILE}"))
}

/// Compares the committed artefact with a fresh render. Trailing whitespace is
/// ignored so a checkout that renormalises line endings is not reported as drift.
fn compare_artefact(root: &Path, rendered: &str, report: &mut Report) {
    let path = root.join(ARTEFACT);
    match std::fs::read_to_string(&path) {
        Ok(committed) if committed.trim_end() == rendered.trim_end() => {}
        Ok(_) => report.push(Violation::new(
            ARTEFACT,
            format!(
                "does not match {DECLARATIONS} and {LOCKFILE}; run `cargo xtask check-notices --update`"
            ),
        )),
        Err(error) => report.push(Violation::new(
            ARTEFACT,
            format!(
                "cannot be read ({error}); run `cargo xtask check-notices --update`"
            ),
        )),
    }
}

fn render(sidecars: &[Sidecar], crates: &[Crate]) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    out.push_str("# Third-party attributions\n\n");
    let _ = write!(
        out,
        "<!-- Generated from `{DECLARATIONS}` and `{LOCKFILE}` by `cargo xtask check-notices \
         --update`. Do not edit by hand: `cargo xtask check-notices` fails when this file and \
         those inputs disagree. -->\n\n"
    );
    out.push_str(
        "Peregrine itself is dual-licensed MIT OR Apache-2.0 (see `LICENSE-MIT` and \
         `LICENSE-APACHE`). This is the attribution inventory a redistribution of Peregrine must \
         carry. It exists for users and distributors, and it is derived rather than hand-written \
         so it cannot drift from the lockfile.\n\n",
    );
    render_sidecars(&mut out, sidecars);
    render_crates(&mut out, crates);
    out.push_str(
        "\n## Frontend packages\n\n\
         The compiled frontend is bundled inside the application. Its runtime packages are listed \
         in `docs/DEPENDENCIES.md` §3; `pnpm-lock.yaml` does not record which packages are \
         development-only, so this gate does not derive that list, and the release's \
         bundled-licence manifest must cover it.\n",
    );
    out
}

fn render_sidecars(out: &mut String, sidecars: &[Sidecar]) {
    use std::fmt::Write as _;

    out.push_str(
        "## Bundled sidecar executables\n\n\
         These are separate programs invoked as subprocesses and never linked into the \
         application binary, so they are aggregated with Peregrine rather than derived from it \
         (`docs/adr/0009-licensing-and-redistribution.md`). Each ships with its licence text, the \
         build configuration below, and a written offer of corresponding source on the same \
         distribution channel as the binary.\n",
    );
    for sidecar in sidecars {
        let _ = write!(out, "\n### {}\n\n", sidecar.name);
        let _ = writeln!(out, "- **Licence:** {}", sidecar.licence);
        let pin = if sidecar.version == PIN_PER_OS {
            "pinned per OS; the exact version and SHA-256 for each platform are recorded by the \
             release bundling work (`docs/DEPENDENCIES.md` §1)"
                .to_owned()
        } else {
            sidecar.version.clone()
        };
        let _ = writeln!(out, "- **Version pin:** {pin}");
        let _ = writeln!(out, "- **Upstream source:** {}", sidecar.source);
        let _ = writeln!(
            out,
            "- **Written offer of corresponding source:** {}",
            sidecar.source_offer
        );
        let _ = writeln!(
            out,
            "- **Why it is a separate executable:** {}",
            sidecar.why_separate
        );
        let _ = writeln!(out, "- **Build configuration:** {}", sidecar.build);
    }
}

fn render_crates(out: &mut String, crates: &[Crate]) {
    use std::fmt::Write as _;

    let _ = write!(
        out,
        "\n## Rust dependencies\n\n\
         The application binary (`{SHIPPED_ROOT}`) resolves to the {} crates below: the \
         non-development dependency closure read from `{LOCKFILE}`, so test runners, mocks, and \
         benchmark harnesses are excluded. Cargo does not record dependency kinds in \
         `{LOCKFILE}`, so a crate that only a third-party build script uses can remain in the \
         list; over-attributing is safe, and every crate linked at runtime is present.\n\n\
         Licence expressions and texts are not repeated here: `cargo deny check` enforces them \
         against the allowlist in `deny.toml`, and the release's bundled-licence manifest and \
         SBOM must carry the texts. The special obligations in `docs/DEPENDENCIES.md` §6 apply \
         to this list.\n\n\
         | Crate | Version |\n| --- | --- |\n",
        crates.len()
    );
    for krate in crates {
        let _ = writeln!(out, "| `{}` | {} |", krate.name, krate.version);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn workspace_root() -> PathBuf {
        crate::workspace::find_root(&PathBuf::from(env!("CARGO_MANIFEST_DIR")))
            .expect("xtask lives inside the workspace")
    }

    /// One complete declaration, used as the fixture every field check removes a
    /// line from.
    const MPV: &str = r#"[[sidecar]]
name = "mpv"
licence = "GPL-2.0-or-later"
version = "pinned-per-os"
source = "https://github.com/mpv-player/mpv"
source_offer = "offered at https://example.invalid/releases"
why_separate = "GPL-2.0-or-later has to stay outside the process boundary"
build = "upstream release build"
"#;

    fn without(text: &str, prefix: &str) -> String {
        text.lines()
            .filter(|line| !line.trim_start().starts_with(prefix))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A minimal tree the gate can run against: one workspace member with no
    /// dependencies, an empty lockfile, and the given declarations.
    fn fixture(declarations: &str) -> tempfile::TempDir {
        let root = tempfile::TempDir::new().expect("a temporary directory");
        std::fs::create_dir_all(root.path().join("app")).unwrap();
        std::fs::write(
            root.path().join(MANIFEST),
            "[workspace]\nmembers = [\"app\"]\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("app").join(MANIFEST),
            "[package]\nname = \"peregrine-desktop\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(root.path().join(LOCKFILE), "version = 4\n").unwrap();
        std::fs::write(root.path().join(DECLARATIONS), declarations).unwrap();
        root
    }

    #[test]
    fn the_shipped_tree_is_attributed() {
        let (report, checked) = check(&workspace_root()).expect("the tree must be readable");
        assert!(
            report.is_clean(),
            "attribution violations: {:#?}",
            report.sorted()
        );
        assert!(
            checked >= 4,
            "the three sidecars and the crate inventory must be counted, got {checked}"
        );
    }

    /// The point of deriving the list from `Cargo.lock` rather than writing it by
    /// hand is that development-only crates never appear and linked ones always do.
    #[test]
    fn development_only_crates_are_excluded() {
        let mut report = Report::new();
        let crates =
            runtime_crates(&workspace_root(), &mut report).expect("the workspace must parse");
        assert!(report.is_clean(), "{:#?}", report.sorted());

        let names: BTreeSet<&str> = crates.iter().map(|krate| krate.name.as_str()).collect();
        for development in [
            "insta",
            "proptest",
            "wiremock",
            "criterion",
            "mockall",
            "rstest",
            "xtask",
            "peregrine-testkit",
        ] {
            assert!(
                !names.contains(development),
                "`{development}` is not redistributed and must not be attributed"
            );
        }
        for runtime in ["serde", "sqlx", "tauri"] {
            assert!(
                names.contains(runtime),
                "`{runtime}` is linked into the application and must be attributed"
            );
        }
    }

    #[test]
    fn the_artefact_lists_every_runtime_crate_and_sidecar() {
        let root = workspace_root();
        let mut report = Report::new();
        let crates = runtime_crates(&root, &mut report).expect("the workspace must parse");
        let artefact = std::fs::read_to_string(root.join(ARTEFACT)).expect("the artefact exists");
        for krate in &crates {
            assert!(
                artefact.contains(&format!("| `{}` | {} |", krate.name, krate.version)),
                "`{} {}` is missing from {ARTEFACT}",
                krate.name,
                krate.version
            );
        }
        for sidecar in ["mpv", "ffmpeg", "ffprobe"] {
            assert!(
                artefact.contains(&format!("### {sidecar}\n")),
                "sidecar `{sidecar}` is missing from {ARTEFACT}"
            );
        }
    }

    /// The task's negative fixture, one field at a time: every field the gate
    /// promises to check has to produce its own actionable message.
    #[test]
    fn every_required_sidecar_field_is_reported_when_missing() {
        let cases = [
            ("name = ", "has no `name`"),
            ("licence = ", "has no recorded `licence`"),
            ("version = ", "has no `version`"),
            ("source = ", "no https `source`"),
            ("source_offer = ", "no written offer"),
            ("why_separate = ", "no `why_separate`"),
            ("build = ", "no `build`"),
        ];
        for (prefix, expected) in cases {
            let mut report = Report::new();
            parse_declarations(&without(MPV, prefix), &mut report);
            assert!(
                report
                    .sorted()
                    .iter()
                    .any(|violation| violation.message.contains(expected)),
                "removing `{prefix}` should report `{expected}`, got {:#?}",
                report.sorted()
            );
        }
    }

    #[test]
    fn a_malformed_declaration_file_is_reported() {
        let mut report = Report::new();
        parse_declarations("[[sidecar]\nname = \"mpv\"\n", &mut report);
        assert!(
            report
                .sorted()
                .iter()
                .any(|violation| violation.message.contains("not a readable sidecar")),
            "{:#?}",
            report.sorted()
        );
    }

    #[test]
    fn a_tree_without_a_lockfile_is_an_error() {
        let root = tempfile::TempDir::new().unwrap();
        std::fs::write(root.path().join(MANIFEST), "[workspace]\nmembers = []\n").unwrap();
        std::fs::write(root.path().join(DECLARATIONS), MPV).unwrap();
        assert!(
            check(root.path()).is_err(),
            "a missing Cargo.lock must be an error, not a clean run"
        );
    }

    #[test]
    fn a_ranged_version_is_not_a_pin() {
        assert!(is_pin(PIN_PER_OS));
        assert!(is_pin("7.1"));
        assert!(is_pin("0.40.0"));
        assert!(is_pin("7.1.1-rc2"));
        for not_a_pin in [
            "*", "^0.40", "~0.40", ">=7.1", "latest", "0", "0.", "0.40.x",
        ] {
            assert!(!is_pin(not_a_pin), "`{not_a_pin}` is not a pin");
        }

        let mut report = Report::new();
        parse_declarations(
            &MPV.replace("version = \"pinned-per-os\"", "version = \"^0.40\""),
            &mut report,
        );
        assert!(
            report
                .sorted()
                .iter()
                .any(|violation| violation.message.contains("is not a pin")),
            "{:#?}",
            report.sorted()
        );
    }

    #[test]
    fn an_empty_or_missing_declaration_is_reported() {
        let mut report = Report::new();
        parse_declarations("", &mut report);
        assert!(
            report
                .sorted()
                .iter()
                .any(|violation| violation.message.contains("declares no sidecars")),
            "{:#?}",
            report.sorted()
        );

        let root = tempfile::TempDir::new().unwrap();
        std::fs::write(root.path().join(MANIFEST), "[workspace]\nmembers = []\n").unwrap();
        std::fs::write(root.path().join(LOCKFILE), "version = 4\n").unwrap();
        let (report, _) = check(root.path()).expect("the gate must run");
        assert!(
            report
                .sorted()
                .iter()
                .any(|violation| violation.path == DECLARATIONS
                    && violation.message.contains("is missing")),
            "{:#?}",
            report.sorted()
        );
    }

    #[test]
    fn a_duplicate_sidecar_name_is_reported() {
        let mut report = Report::new();
        parse_declarations(&format!("{MPV}{MPV}"), &mut report);
        assert!(
            report
                .sorted()
                .iter()
                .any(|violation| violation.message.contains("declared more than once")),
            "{:#?}",
            report.sorted()
        );
    }

    /// The command must exit non-zero on the fixture, refuse to generate an
    /// artefact from an incomplete declaration, and pass once repaired.
    #[test]
    fn the_command_fails_when_a_sidecar_has_no_licence_record() {
        let root = fixture(&without(MPV, "licence = "));

        let (report, _) = check(root.path()).expect("the gate must run against the fixture");
        assert!(
            report
                .sorted()
                .iter()
                .any(|violation| violation.path == DECLARATIONS
                    && violation.message.contains("has no recorded `licence`")),
            "{:#?}",
            report.sorted()
        );
        assert!(
            !run(root.path(), Mode::Check).expect("the gate must run"),
            "a missing licence record must fail the command"
        );
        assert!(
            !run(root.path(), Mode::Update).expect("update must run"),
            "update must not paper over a missing licence record"
        );

        std::fs::write(root.path().join(DECLARATIONS), MPV).unwrap();
        assert!(
            run(root.path(), Mode::Update).expect("update must run"),
            "a complete declaration must be writable"
        );
        assert!(
            run(root.path(), Mode::Check).expect("the gate must run"),
            "the repaired tree must pass"
        );
    }

    #[test]
    fn a_stale_artefact_is_rejected() {
        let root = fixture(MPV);
        assert!(run(root.path(), Mode::Update).expect("update must run"));
        assert!(
            run(root.path(), Mode::Check).expect("the gate must run"),
            "a freshly written artefact must pass"
        );

        let path = root.path().join(ARTEFACT);
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("\n| `ghost` | 9.9.9 |\n");
        std::fs::write(&path, text).unwrap();

        assert!(
            !run(root.path(), Mode::Check).expect("the gate must run"),
            "an artefact that drifted from the lockfile must fail the gate"
        );
        let (report, _) = check(root.path()).unwrap();
        assert!(
            report
                .sorted()
                .iter()
                .any(|violation| violation.path == ARTEFACT
                    && violation.message.contains("check-notices --update")),
            "{:#?}",
            report.sorted()
        );
    }
}
