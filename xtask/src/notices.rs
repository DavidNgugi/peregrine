//! `check-notices`: every bundled sidecar carries a recorded licence, a version
//! pin, a source URL, and a written offer of corresponding source, and
//! `THIRD_PARTY.md` is exactly what those declarations plus `Cargo.lock` and
//! `pnpm-lock.yaml` produce.
//!
//! Why the attribution artefact is derived rather than hand-maintained: the
//! crates linked into the application and the packages bundled into the compiled
//! frontend are outputs of the two lockfiles, so a written list drifts the first
//! time a dependency changes, and it drifts silently. The three things no file in
//! the tree can answer — which executables we redistribute, what build each one
//! uses, and where its corresponding source is offered from — are declared by
//! hand in `sidecars.toml` and validated here.
//!
//! The crate inventory is the non-development dependency closure of the shipped
//! binary, read textually from `Cargo.lock` so the gate stays offline and needs no
//! registry. `Cargo.lock` does not record edge kinds, so a crate that only a
//! third-party build script uses can remain in the list; over-attribution is
//! harmless, and every crate reached at runtime is present.
//!
//! The frontend inventory is the runtime closure of `apps/desktop` and the
//! workspace packages it links, read out of `pnpm-lock.yaml`. That lockfile does
//! record which edges are development-only, so the boundary is derived rather
//! than guessed — see [`walk_frontend`] for why a `pnpm`-based generator was
//! rejected instead. Both walks run offline: `check-notices` needs no registry,
//! no `node_modules`, and no network.

use crate::report::{Report, Violation};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use yaml_rust2::YamlLoader;
use yaml_rust2::yaml::{Hash, Yaml};

/// The hand-maintained sidecar declarations.
pub const DECLARATIONS: &str = "sidecars.toml";
/// The generated attribution artefact.
pub const ARTEFACT: &str = "THIRD_PARTY.md";
const LOCKFILE: &str = "Cargo.lock";
const MANIFEST: &str = "Cargo.toml";
/// The pnpm lockfile the frontend runtime closure is read from.
const FRONTEND_LOCKFILE: &str = "pnpm-lock.yaml";
/// The workspace project the compiled frontend is built from. Vite bundles this
/// app and the workspace packages it links into `apps/desktop/dist`.
const FRONTEND_ROOT: &str = "apps/desktop";
/// The only `lockfileVersion` this walk understands. pnpm has written `9.0` across
/// pnpm 9, 10, and 11; a different major means the sections below may have moved,
/// and a walk that found no edges would report an empty inventory that reads
/// exactly like a complete one.
const PNPM_LOCKFILE_VERSION: &str = "9.0";

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

/// One third-party package bundled into the compiled frontend.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct FrontendPackage {
    name: String,
    version: String,
}

/// Everything a check or an update works from.
struct Inventory {
    sidecars: Vec<Sidecar>,
    crates: Vec<Crate>,
    packages: Vec<FrontendPackage>,
    report: Report,
}

impl Inventory {
    fn checked(&self) -> usize {
        self.sidecars.len() + self.crates.len() + self.packages.len()
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
        let rendered = render(&inventory.sidecars, &inventory.crates, &inventory.packages);
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
    let text = render(&inventory.sidecars, &inventory.crates, &inventory.packages);
    let path = root.join(ARTEFACT);
    std::fs::write(&path, &text).with_context(|| format!("cannot write {}", path.display()))?;
    println!(
        "check-notices: wrote {ARTEFACT} ({} sidecar(s), {} crate(s), {} frontend package(s))",
        inventory.sidecars.len(),
        inventory.crates.len(),
        inventory.packages.len()
    );
    Ok(true)
}

fn inventory(root: &Path) -> Result<Inventory> {
    let mut report = Report::new();
    let sidecars = declarations(root, &mut report)?;
    let packages = frontend_packages(root, &mut report)?;
    let crates = runtime_crates(root, &mut report)?;
    Ok(Inventory {
        sidecars,
        crates,
        packages,
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

/// The runtime packages bundled into the compiled frontend.
fn frontend_packages(root: &Path, report: &mut Report) -> Result<Vec<FrontendPackage>> {
    let path = root.join(FRONTEND_LOCKFILE);
    if !path.is_file() {
        report.push(Violation::new(
            FRONTEND_LOCKFILE,
            "is missing; the packages bundled into the compiled frontend are read from it",
        ));
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    Ok(walk_frontend(&text, report))
}

/// The frontend runtime closure, read from lockfile text.
///
/// Derived here rather than by a `pnpm licenses list --json --prod` generator,
/// which is what the release SBOM uses for licence texts, because that command
/// does not draw the boundary this artefact needs: it reports an *optional peer*
/// that `autoInstallPeers` installed — `typescript` for `i18next`, `@types/react`
/// for `zustand` — as a production package, and those are type-only installs the
/// bundle never imports. It would also make the gate need `node_modules`, and the
/// lockfile already records the edge kinds the generator would be guessing at.
///
/// Split from the file read so the fixture tests can exercise the walk without a
/// fixture directory.
fn walk_frontend(text: &str, report: &mut Report) -> Vec<FrontendPackage> {
    let Ok(documents) = YamlLoader::load_from_str(text) else {
        report.push(Violation::new(
            FRONTEND_LOCKFILE,
            "is not readable YAML; the frontend runtime inventory is read from it",
        ));
        return Vec::new();
    };
    let Some(document) = documents.first() else {
        report.push(Violation::new(
            FRONTEND_LOCKFILE,
            "is empty; the frontend runtime inventory is read from it",
        ));
        return Vec::new();
    };
    let version = document["lockfileVersion"].as_str();
    if version != Some(PNPM_LOCKFILE_VERSION) {
        report.push(Violation::new(
            FRONTEND_LOCKFILE,
            format!(
                "records lockfileVersion `{}` and this gate reads `{PNPM_LOCKFILE_VERSION}` (pnpm 9 \
                 through 11); an unreadable format has to fail here rather than produce an empty \
                 inventory that reads like a complete one",
                version.unwrap_or("nothing")
            ),
        ));
        return Vec::new();
    }
    let Some(importers) = document["importers"].as_hash() else {
        report.push(Violation::new(
            FRONTEND_LOCKFILE,
            "has no `importers` section, so no workspace project's dependencies can be read",
        ));
        return Vec::new();
    };
    let mut walk = FrontendWalk {
        snapshots: document["snapshots"].as_hash(),
        packages: document["packages"].as_hash(),
        attributed: BTreeSet::new(),
        visited_snapshots: BTreeSet::new(),
        visited_projects: BTreeSet::new(),
    };
    walk.project(importers, FRONTEND_ROOT, report);
    if walk.attributed.is_empty() {
        report.push(Violation::new(
            FRONTEND_LOCKFILE,
            format!(
                "resolves no runtime package for `{FRONTEND_ROOT}`; an empty inventory reads \
                 exactly like a complete one"
            ),
        ));
    }
    walk.attributed
        .into_iter()
        .map(|(name, version)| FrontendPackage { name, version })
        .collect()
}

/// One walk over `pnpm-lock.yaml`'s runtime graph.
///
/// The lockfile has two edge tables. `importers` holds each workspace project's
/// own `dependencies` and `devDependencies`, which is where the development-only
/// boundary is written down; `snapshots` holds each resolved package's own
/// dependency edges. Only a project's runtime tables and a package's runtime
/// edges are followed, so a package reachable only through the toolchain has no
/// edge into the walk and cannot appear.
struct FrontendWalk<'lock> {
    snapshots: Option<&'lock Hash>,
    /// The `packages` section, keyed `name@version`. Needed only to recognise an
    /// auto-installed optional peer, which pnpm records in a snapshot's
    /// `optionalDependencies` beside genuine optional dependencies.
    packages: Option<&'lock Hash>,
    /// Attributed `(name, version)` pairs, with pnpm's peer suffix dropped.
    attributed: BTreeSet<(String, String)>,
    visited_snapshots: BTreeSet<String>,
    visited_projects: BTreeSet<String>,
}

impl FrontendWalk<'_> {
    /// Follows one workspace project's runtime dependencies.
    fn project(&mut self, importers: &Hash, project: &str, report: &mut Report) {
        if !self.visited_projects.insert(project.to_owned()) {
            return;
        }
        let Some(entry) = importers.get(&Yaml::String(project.to_owned())) else {
            report.push(Violation::new(
                FRONTEND_LOCKFILE,
                format!(
                    "has no `{project}` importer, so the frontend runtime inventory cannot start; \
                     update `FRONTEND_ROOT` in xtask/src/notices.rs if the layout changed"
                ),
            ));
            return;
        };
        for section in ["dependencies", "optionalDependencies"] {
            let Some(edges) = entry[section].as_hash() else {
                continue;
            };
            for (name, edge) in edges {
                let Some(name) = name.as_str() else {
                    continue;
                };
                let Some(version) = scalar(&edge["version"]) else {
                    report.push(Violation::new(
                        FRONTEND_LOCKFILE,
                        format!(
                            "records no resolved version for `{name}` in `{project}`; a dependency \
                             with no resolution cannot be attributed"
                        ),
                    ));
                    continue;
                };
                if let Some(link) = version.strip_prefix("link:") {
                    // A workspace package is our own code rather than third-party
                    // attribution, but its runtime dependencies bundle with it.
                    self.project(importers, &resolve_project(project, link), report);
                } else {
                    self.edge(name, &version, report);
                }
            }
        }
    }

    /// Follows one resolved dependency edge.
    fn edge(&mut self, name: &str, version: &str, report: &mut Report) {
        // An aliased dependency (`"typescript": "npm:@typescript/typescript6@6.0.2"`)
        // records the whole resolved key in the version field rather than a
        // version on its own.
        let primary = format!("{name}@{version}");
        let key = if snapshot(self.snapshots, &primary).is_some() {
            primary
        } else {
            version.to_owned()
        };
        let Some(entry) = snapshot(self.snapshots, &key) else {
            report.push(Violation::new(
                FRONTEND_LOCKFILE,
                format!(
                    "resolves `{name}` to `{version}`, which no snapshot matches; a runtime \
                     dependency without a resolved package would otherwise be dropped from the \
                     inventory without a word"
                ),
            ));
            return;
        };
        if !self.visited_snapshots.insert(key.clone()) {
            return;
        }
        if let Some(pair) = split_snapshot_key(&key) {
            self.attributed.insert(pair);
        }
        for section in ["dependencies", "optionalDependencies"] {
            let Some(edges) = entry[section].as_hash() else {
                continue;
            };
            for (dependency, spec) in edges {
                let (Some(dependency), Some(version)) = (dependency.as_str(), scalar(spec)) else {
                    continue;
                };
                // pnpm writes the optional peers it installed for a package into
                // the same table as that package's genuine optional dependencies.
                // A peer is a type or tooling install the bundle never imports —
                // attributing `typescript` because `i18next` names it as a peer
                // would be a false entry — while a genuine optional dependency is
                // kept, because over-attribution is safe and omission is not.
                if section == "optionalDependencies" && is_peer(self.packages, &key, dependency) {
                    continue;
                }
                self.edge(dependency, &version, report);
            }
        }
    }
}

/// The snapshot entry a resolved edge points at.
fn snapshot<'lock>(snapshots: Option<&'lock Hash>, key: &str) -> Option<&'lock Yaml> {
    snapshots?.get(&Yaml::String(key.to_owned()))
}

/// The package name and resolved version in a snapshot key, with pnpm's peer
/// suffix dropped (`react-dom@19.3.0(react@19.3.0)`).
fn split_snapshot_key(key: &str) -> Option<(String, String)> {
    let base = key.split('(').next()?;
    let (name, version) = base.rsplit_once('@')?;
    if name.is_empty() || version.is_empty() {
        return None;
    }
    Some((name.to_owned(), version.to_owned()))
}

/// Whether `dependency` is a declared peer of the package a snapshot describes.
fn is_peer(packages: Option<&Hash>, snapshot_key: &str, dependency: &str) -> bool {
    let base = snapshot_key.split('(').next().unwrap_or(snapshot_key);
    packages
        .and_then(|packages| packages.get(&Yaml::String(base.to_owned())))
        .and_then(|package| package["peerDependencies"].as_hash())
        .is_some_and(|peers| peers.contains_key(&Yaml::String(dependency.to_owned())))
}

/// The literal text of a YAML scalar.
///
/// pnpm does not quote every version, and YAML 1.2 reads a bare `1.0` as a real
/// and a bare `1` as an integer, so the text is recovered from all three variants;
/// a version read as `1` where the lockfile means `1.0` would not match its
/// snapshot key and would be reported as unresolved rather than silently dropped.
fn scalar(value: &Yaml) -> Option<String> {
    match value {
        Yaml::String(text) | Yaml::Real(text) => Some(text.clone()),
        Yaml::Integer(number) => Some(number.to_string()),
        _ => None,
    }
}

/// The importer key a `link:` target names, resolved against the project that
/// declared it. Lockfile paths always use forward slashes.
fn resolve_project(project: &str, link: &str) -> String {
    let mut parts: Vec<&str> = if project == "." {
        Vec::new()
    } else {
        project.split('/').collect()
    };
    for component in link.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    }
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
                "does not match {DECLARATIONS}, {LOCKFILE}, and {FRONTEND_LOCKFILE}; run `cargo \
                 xtask check-notices --update`"
            ),
        )),
        Err(error) => report.push(Violation::new(
            ARTEFACT,
            format!("cannot be read ({error}); run `cargo xtask check-notices --update`"),
        )),
    }
}

fn render(sidecars: &[Sidecar], crates: &[Crate], packages: &[FrontendPackage]) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    out.push_str("# Third-party attributions\n\n");
    let _ = write!(
        out,
        "<!-- Generated from `{DECLARATIONS}`, `{LOCKFILE}`, and `{FRONTEND_LOCKFILE}` by \
         `cargo xtask check-notices --update`. Do not edit by hand: `cargo xtask check-notices` \
         fails when this file and those inputs disagree. -->\n\n"
    );
    out.push_str(
        "Peregrine itself is dual-licensed MIT OR Apache-2.0 (see `LICENSE-MIT` and \
         `LICENSE-APACHE`). This is the attribution inventory a redistribution of Peregrine must \
         carry. It exists for users and distributors, and it is derived rather than hand-written \
         so it cannot drift from the lockfiles.\n\n",
    );
    render_sidecars(&mut out, sidecars);
    render_crates(&mut out, crates);
    render_frontend(&mut out, packages);
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

fn render_frontend(out: &mut String, packages: &[FrontendPackage]) {
    use std::fmt::Write as _;

    let _ = write!(
        out,
        "\n## Frontend packages\n\n\
         The compiled frontend (`apps/desktop/dist`) bundles the {} packages below. They are the \
         runtime closure of `{FRONTEND_ROOT}` and of the workspace packages it links, read from \
         `{FRONTEND_LOCKFILE}` by following `dependencies` and `optionalDependencies`.\n\n\
         Development-only packages are not redistributed and cannot appear here: pnpm records a \
         workspace project's development dependencies in the lockfile but never resolves their \
         transitive dependencies, so the Vite, ESLint, and Vitest toolchains have no edge into this \
         walk. An optional *peer* that pnpm auto-installed for another package's types \
         (`typescript`, `@types/react`) is excluded for the same reason: the bundle imports no code \
         from it.\n\n\
         Licence expressions and texts are not repeated here: `docs/DEPENDENCIES.md` §3 records the \
         reviewed licence and purpose of each package, and the release's `pnpm licenses list \
         --json` manifest carries the texts. `pnpm check-dead-deps` fails on a declared dependency \
         nothing imports, so no package is listed merely because a manifest mentions it.\n\n\
         | Package | Version |\n| --- | --- |\n",
        packages.len()
    );
    for package in packages {
        let _ = writeln!(out, "| `{}` | {} |", package.name, package.version);
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

    /// A minimal, complete pnpm lockfile: two runtime dependencies, one of which
    /// depends on the other. Every fixture tree gets it, so `check` can run.
    const PNPM_LOCK: &str = r"lockfileVersion: '9.0'
importers:
  apps/desktop:
    dependencies:
      react:
        specifier: 19.3.0
        version: 19.3.0
      react-dom:
        specifier: 19.3.0
        version: 19.3.0(react@19.3.0)
packages:
  react@19.3.0: {}
  react-dom@19.3.0:
    peerDependencies:
      react: ^19.3.0
snapshots:
  react@19.3.0: {}
  react-dom@19.3.0(react@19.3.0):
    dependencies:
      react: 19.3.0
";

    fn without(text: &str, prefix: &str) -> String {
        text.lines()
            .filter(|line| !line.trim_start().starts_with(prefix))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A minimal tree the gate can run against: one workspace member with no
    /// dependencies, a lockfile for each half, and the given declarations.
    fn fixture(declarations: &str) -> tempfile::TempDir {
        fixture_lock(declarations, PNPM_LOCK)
    }

    /// The same tree with the frontend lockfile replaced.
    fn fixture_lock(declarations: &str, lock: &str) -> tempfile::TempDir {
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
        std::fs::write(root.path().join(FRONTEND_LOCKFILE), lock).unwrap();
        std::fs::write(root.path().join(DECLARATIONS), declarations).unwrap();
        root
    }

    fn frontend_names(lock: &str) -> Vec<String> {
        let mut report = Report::new();
        let packages = walk_frontend(lock, &mut report);
        assert!(report.is_clean(), "{:#?}", report.sorted());
        packages.into_iter().map(|package| package.name).collect()
    }

    #[test]
    fn the_shipped_tree_is_attributed() {
        let root = workspace_root();
        let (report, checked) = check(&root).expect("the tree must be readable");
        assert!(
            report.is_clean(),
            "attribution violations: {:#?}",
            report.sorted()
        );

        let mut parts = Report::new();
        let sidecars = declarations(&root, &mut parts).expect("the declarations read");
        let packages = frontend_packages(&root, &mut parts).expect("the frontend lockfile reads");
        let crates = runtime_crates(&root, &mut parts).expect("the workspace parses");
        assert!(parts.is_clean(), "{:#?}", parts.sorted());
        assert_eq!(
            checked,
            sidecars.len() + crates.len() + packages.len(),
            "the count must cover the sidecars, the crate closure, and the frontend packages"
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

    /// The point of deriving the frontend list from `pnpm-lock.yaml` rather than
    /// writing it by hand is that the toolchain stays out and the bundled packages
    /// always appear.
    #[test]
    fn development_only_frontend_packages_are_excluded() {
        let mut report = Report::new();
        let packages =
            frontend_packages(&workspace_root(), &mut report).expect("the lockfile reads");
        assert!(report.is_clean(), "{:#?}", report.sorted());

        let names: BTreeSet<&str> = packages
            .iter()
            .map(|package| package.name.as_str())
            .collect();
        for development in [
            "typescript",
            "@types/react",
            "csstype",
            "@typescript/typescript-darwin-arm64",
            "vite",
            "vitest",
            "eslint",
            "prettier",
        ] {
            assert!(
                !names.contains(development),
                "`{development}` is development-only and must not be attributed"
            );
        }
        for runtime in [
            "react",
            "react-dom",
            "scheduler",
            "zustand",
            "i18next",
            "react-i18next",
            "use-sync-external-store",
        ] {
            assert!(
                names.contains(runtime),
                "`{runtime}` is bundled into the frontend and must be attributed"
            );
        }
    }

    #[test]
    fn the_artefact_lists_every_bundled_frontend_package() {
        let root = workspace_root();
        let mut report = Report::new();
        let packages = frontend_packages(&root, &mut report).expect("the lockfile reads");
        assert!(report.is_clean(), "{:#?}", report.sorted());
        let artefact = std::fs::read_to_string(root.join(ARTEFACT)).expect("the artefact exists");
        for package in &packages {
            assert!(
                artefact.contains(&format!("| `{}` | {} |", package.name, package.version)),
                "`{} {}` is missing from {ARTEFACT}",
                package.name,
                package.version
            );
        }
    }

    /// The negative fixture the task asks for: a runtime dependency the lockfile
    /// declares but does not resolve must fail the walk, rather than disappear
    /// from the inventory. `react-dom` is genuinely bundled, so its absence is the
    /// failure the gate exists to catch.
    #[test]
    fn a_runtime_dependency_with_no_resolved_package_is_reported() {
        let broken = PNPM_LOCK.replace(
            "  react-dom@19.3.0(react@19.3.0):\n    dependencies:\n      react: 19.3.0\n",
            "",
        );
        assert_ne!(
            broken, PNPM_LOCK,
            "the fixture must lose its `react-dom` snapshot"
        );

        let mut report = Report::new();
        let packages = walk_frontend(&broken, &mut report);
        assert!(
            report
                .sorted()
                .iter()
                .any(|violation| violation.message.contains("`react-dom`")
                    && violation.message.contains("no snapshot matches")),
            "{:#?}",
            report.sorted()
        );
        assert!(
            !packages.iter().any(|package| package.name == "react-dom"),
            "an unresolved dependency must not be attributed from nowhere"
        );
    }

    /// The other direction of the same fixture, end to end: an artefact that does
    /// not list a bundled package is stale, and the command exits non-zero.
    #[test]
    fn an_artefact_missing_a_bundled_package_is_rejected() {
        let root = fixture(MPV);
        assert!(run(root.path(), Mode::Update).expect("update must run"));
        assert!(run(root.path(), Mode::Check).expect("a fresh artefact must pass"));

        let path = root.path().join(ARTEFACT);
        let committed = std::fs::read_to_string(&path).unwrap();
        let missing = committed.replace("| `react-dom` | 19.3.0 |\n", "");
        assert_ne!(
            missing, committed,
            "the fresh artefact must list `react-dom`"
        );
        std::fs::write(&path, missing).unwrap();

        assert!(
            !run(root.path(), Mode::Check).expect("the gate must run"),
            "an artefact missing a bundled runtime dependency must fail the gate"
        );
    }

    /// A dependency added to the lockfile but not to the artefact is drift, which
    /// is the direction a byte comparison exists to catch.
    #[test]
    fn a_runtime_dependency_the_artefact_does_not_list_is_drift() {
        let root = fixture(MPV);
        assert!(run(root.path(), Mode::Update).expect("update must run"));

        let grown = PNPM_LOCK
            .replace(
                "snapshots:\n  react@19.3.0: {}\n",
                "snapshots:\n  react@19.3.0: {}\n  scheduler@0.28.0: {}\n",
            )
            .replace(
                "      react-dom:\n",
                "      scheduler:\n        specifier: 0.28.0\n        version: 0.28.0\n      react-dom:\n",
            );
        assert_ne!(grown, PNPM_LOCK);
        std::fs::write(root.path().join(FRONTEND_LOCKFILE), grown).unwrap();

        assert!(
            !run(root.path(), Mode::Check).expect("the gate must run"),
            "a bundled package the artefact does not list must fail the gate"
        );
        let (report, _) = check(root.path()).expect("the gate must run");
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

    #[test]
    fn an_unsupported_lockfile_version_is_reported() {
        let mut report = Report::new();
        let packages = walk_frontend(&PNPM_LOCK.replace("'9.0'", "'6.0'"), &mut report);
        assert!(packages.is_empty(), "{packages:#?}");
        assert!(
            report
                .sorted()
                .iter()
                .any(|violation| violation.message.contains("lockfileVersion")),
            "{:#?}",
            report.sorted()
        );
    }

    #[test]
    fn a_malformed_lockfile_is_reported() {
        let mut report = Report::new();
        let packages = walk_frontend("importers: [\n", &mut report);
        assert!(packages.is_empty(), "{packages:#?}");
        assert!(
            report
                .sorted()
                .iter()
                .any(|violation| violation.message.contains("not readable YAML")),
            "{:#?}",
            report.sorted()
        );
    }

    /// A lockfile whose only edges are development edges resolves nothing, which
    /// has to be a violation rather than an empty inventory that reads as clean.
    #[test]
    fn a_development_only_lockfile_resolves_nothing() {
        let lock = "lockfileVersion: '9.0'
importers:
  apps/desktop:
    devDependencies:
      vite:
        specifier: 8.3.4
        version: 8.3.4
";
        let mut report = Report::new();
        let packages = walk_frontend(lock, &mut report);
        assert!(packages.is_empty(), "{packages:#?}");
        assert!(
            report
                .sorted()
                .iter()
                .any(|violation| violation.message.contains("resolves no runtime package")),
            "{:#?}",
            report.sorted()
        );
    }

    /// A workspace link is our own code, so it is not attributed, but its runtime
    /// dependencies bundle with it and are.
    #[test]
    fn a_workspace_package_is_followed_but_not_attributed() {
        let lock = "lockfileVersion: '9.0'
importers:
  apps/desktop:
    dependencies:
      '@peregrine/i18n':
        specifier: workspace:*
        version: link:../../packages/i18n
  packages/i18n:
    dependencies:
      i18next:
        specifier: 26.4.2
        version: 26.4.2
snapshots:
  i18next@26.4.2: {}
";
        assert_eq!(frontend_names(lock), ["i18next"]);
    }

    /// pnpm writes the optional peers it auto-installed into the same table as a
    /// package's genuine optional dependencies. Only the peer is excluded: it is a
    /// type-only install the bundle never imports, while a real optional
    /// dependency is kept because over-attribution is safe and omission is not.
    #[test]
    fn an_auto_installed_optional_peer_is_excluded() {
        let lock = "lockfileVersion: '9.0'
importers:
  apps/desktop:
    dependencies:
      react-error-boundary:
        specifier: 6.1.6
        version: 6.1.6
snapshots:
  react-error-boundary@6.1.6:
    dependencies:
      react: 19.3.0
    optionalDependencies:
      '@types/react': 19.3.0
      fsevents: 2.3.3
  react@19.3.0: {}
  fsevents@2.3.3: {}
packages:
  react-error-boundary@6.1.6:
    peerDependencies:
      '@types/react': ^19.0.0
";
        let names = frontend_names(lock);
        assert!(names.contains(&"fsevents".to_owned()), "{names:#?}");
        assert!(!names.contains(&"@types/react".to_owned()), "{names:#?}");
    }

    /// An aliased dependency records the whole resolved key in the version field,
    /// so it does not resolve as `declared@version`.
    #[test]
    fn an_aliased_dependency_resolves_to_its_real_package() {
        let lock = "lockfileVersion: '9.0'
importers:
  apps/desktop:
    dependencies:
      typescript:
        specifier: npm:@typescript/typescript6@6.0.2
        version: '@typescript/typescript6@6.0.2'
snapshots:
  '@typescript/typescript6@6.0.2': {}
";
        let mut report = Report::new();
        let packages = walk_frontend(lock, &mut report);
        assert!(report.is_clean(), "{:#?}", report.sorted());
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "@typescript/typescript6");
        assert_eq!(packages[0].version, "6.0.2");
    }

    /// pnpm does not quote every version, and YAML reads a bare `1.0` as a number.
    /// The version has to survive as text or its snapshot key cannot match.
    #[test]
    fn an_unquoted_numeric_version_resolves() {
        let lock = "lockfileVersion: '9.0'
importers:
  apps/desktop:
    dependencies:
      tiny:
        specifier: 1.0
        version: 1.0
snapshots:
  tiny@1.0: {}
";
        let mut report = Report::new();
        let packages = walk_frontend(lock, &mut report);
        assert!(report.is_clean(), "{:#?}", report.sorted());
        assert_eq!(packages[0].name, "tiny");
        assert_eq!(packages[0].version, "1.0");
    }

    /// A `link:` target is relative to the project that declares it.
    #[test]
    fn a_link_target_is_resolved_against_its_project() {
        assert_eq!(
            resolve_project("apps/desktop", "../../packages/ui"),
            "packages/ui"
        );
        assert_eq!(resolve_project("apps/desktop", "../ui"), "apps/ui");
        assert_eq!(resolve_project(".", "packages/i18n"), "packages/i18n");
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

    /// A missing frontend lockfile is a violation rather than a clean run, since
    /// the frontend half of the inventory would otherwise be silently empty.
    #[test]
    fn a_tree_without_a_frontend_lockfile_is_reported() {
        let root = tempfile::TempDir::new().unwrap();
        std::fs::write(root.path().join(MANIFEST), "[workspace]\nmembers = []\n").unwrap();
        std::fs::write(root.path().join(LOCKFILE), "version = 4\n").unwrap();
        std::fs::write(root.path().join(DECLARATIONS), MPV).unwrap();

        let mut report = Report::new();
        let packages = frontend_packages(root.path(), &mut report).expect("the gate must run");
        assert!(packages.is_empty(), "{packages:#?}");
        assert!(
            report
                .sorted()
                .iter()
                .any(|violation| violation.path == FRONTEND_LOCKFILE
                    && violation.message.contains("is missing")),
            "{:#?}",
            report.sorted()
        );
        assert!(
            !run(root.path(), Mode::Check).expect("the gate must run"),
            "a missing frontend lockfile must fail the command"
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
