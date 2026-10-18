//! `release-check`: the local half of the release decision.
//!
//! A tag is only safe to cut when three version sources agree, the pinned
//! sidecars and the generated attribution are current, and the workflow that
//! will build the four bundles is present and still uploads them. Signing is
//! reported rather than required: the v1.0 release candidate is deliberately
//! unsigned, and `--allow-unsigned` is how a maintainer states that policy
//! explicitly instead of letting an unset environment variable decide it.
//!
//! Secret *values* are never read. The check looks only at whether the names a
//! signed release needs are present in the environment, which is enough to
//! answer "would the signing job run?" without turning a terminal log into a
//! place a key can leak.

use crate::notices;
use crate::report::{Report, Violation};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

/// The workflow that builds and publishes a release.
pub const WORKFLOW: &str = ".github/workflows/release.yml";
/// The committed sidecar pins, read by `scripts/fetch-sidecars.mjs`.
pub const SIDECAR_LOCK: &str = "packaging/sidecars.lock.json";
/// The Tauri bundle configuration, which carries the version a release is cut from.
pub const TAURI_CONFIG: &str = "apps/desktop/src-tauri/tauri.conf.json";
/// The workspace manifest, whose `[workspace.package] version` is the root version.
pub const MANIFEST: &str = "Cargo.toml";
/// The frontend workspace manifest.
pub const PACKAGE_JSON: &str = "package.json";

/// The repository secrets and variable a signed manifest needs, named exactly as
/// the workflow reads them. Presence is checked; values are not.
const SIGNING_SECRETS: [&str; 3] = [
    "RELEASE_MINISIGN_SECRET_KEY",
    "RELEASE_MINISIGN_PASSWORD",
    "RELEASE_MINISIGN_PUBLIC_KEY",
];
const SIGNING_VARIABLE: &str = "RELEASE_SIGNING_CONFIGURED";

/// The four bundle labels the release matrix must still build and upload.
const BUNDLE_LABELS: [&str; 4] = [
    "macos-aarch64",
    "macos-x86_64",
    "windows-x86_64",
    "linux-x86_64",
];

/// Whether the environment can produce a signed manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signing {
    /// Every named secret and the opt-in variable are present.
    Configured,
    /// Signing is absent; the list names what is missing, never what it contains.
    Unconfigured { missing: Vec<&'static str> },
}

impl Signing {
    #[must_use]
    pub fn is_configured(&self) -> bool {
        matches!(self, Self::Configured)
    }
}

/// Reads only environment *presence*, so a missing and an empty variable are the
/// same answer: neither can sign anything.
#[must_use]
pub fn signing_status(env: impl Fn(&str) -> Option<String>) -> Signing {
    let mut missing: Vec<&'static str> = SIGNING_SECRETS
        .iter()
        .filter(|name| env(name).is_none_or(|value| value.trim().is_empty()))
        .copied()
        .collect();
    if env(SIGNING_VARIABLE).as_deref() != Some("true") {
        missing.push(SIGNING_VARIABLE);
    }
    if missing.is_empty() {
        Signing::Configured
    } else {
        Signing::Unconfigured { missing }
    }
}

/// Runs the gate, printing the signing decision before the pass/fail summary.
pub fn run(root: &Path, allow_unsigned: bool) -> Result<bool> {
    let signing = signing_status(|name| std::env::var(name).ok());
    match &signing {
        Signing::Configured => {
            println!("release-check: signing is configured; the release manifest will be signed");
        }
        Signing::Unconfigured { missing } => println!(
            "release-check: signing is not configured; missing {} (presence only, no values read)",
            missing.join(", ")
        ),
    }
    if !signing.is_configured() && allow_unsigned {
        println!(
            "release-check: unsigned release candidate explicitly allowed by --allow-unsigned"
        );
    }

    let (report, checked) = check(root, &signing, allow_unsigned)?;
    Ok(crate::report::finish("release-check", &report, checked))
}

/// Checks the release inputs, returning the violations and how many things were
/// inspected.
pub fn check(root: &Path, signing: &Signing, allow_unsigned: bool) -> Result<(Report, usize)> {
    let mut report = Report::new();
    let mut checked = 0;

    checked += check_versions(root, &mut report)?;
    checked += check_artefacts(root, &mut report)?;
    checked += check_workflow(root, &mut report)?;

    // The attribution artefact and the sidecar declarations are derived inputs,
    // so the existing gate owns whether they are current. Re-implementing any
    // part of that here would be a second answer to the same question.
    let (notices_report, notices_checked) = notices::check(root)?;
    report.extend(notices_report.sorted().into_iter().cloned());
    checked += notices_checked;

    if !signing.is_configured()
        && !allow_unsigned
        && let Signing::Unconfigured { missing } = signing
    {
        report.push(Violation::new(
            WORKFLOW,
            format!(
                "signing is not configured (missing {}); pass --allow-unsigned to prepare an unsigned release candidate, or configure the signing secrets for a signed one",
                missing.join(", ")
            ),
        ));
    }

    Ok((report, checked))
}

/// The three versions that must agree before a tag means one thing.
fn check_versions(root: &Path, report: &mut Report) -> Result<usize> {
    let workspace = workspace_version(root, report)?;
    let tauri = json_version(root, TAURI_CONFIG, report)?;
    let package = json_version(root, PACKAGE_JSON, report)?;

    if let Some(expected) = &workspace {
        for (path, actual) in [(TAURI_CONFIG, &tauri), (PACKAGE_JSON, &package)] {
            if let Some(actual) = actual
                && actual != expected
            {
                report.push(Violation::new(
                    path,
                    format!(
                        "declares version {actual} but Cargo.toml's [workspace.package] declares {expected}; all three release version sources must agree"
                    ),
                ));
            }
        }
    }
    Ok(3)
}

fn workspace_version(root: &Path, report: &mut Report) -> Result<Option<String>> {
    let path = root.join(MANIFEST);
    if !path.is_file() {
        report.push(Violation::new(
            MANIFEST,
            "is missing; the workspace version cannot be read",
        ));
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let manifest: toml::Value = match toml::from_str(&text) {
        Ok(manifest) => manifest,
        Err(error) => {
            report.push(Violation::new(
                MANIFEST,
                format!("is not readable TOML: {error}"),
            ));
            return Ok(None);
        }
    };
    let version = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str);
    if version.is_none() {
        report.push(Violation::new(
            MANIFEST,
            "has no [workspace.package] version; every release version is derived from it",
        ));
    }
    Ok(version.map(str::to_owned))
}

fn json_version(root: &Path, path: &str, report: &mut Report) -> Result<Option<String>> {
    let full = root.join(path);
    if !full.is_file() {
        report.push(Violation::new(
            path,
            "is missing; the release version cannot be read",
        ));
        return Ok(None);
    }
    let text = std::fs::read_to_string(&full)
        .with_context(|| format!("cannot read {}", full.display()))?;
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            report.push(Violation::new(
                path,
                format!("is not readable JSON: {error}"),
            ));
            return Ok(None);
        }
    };
    let version = value.get("version").and_then(Value::as_str);
    if version.is_none() {
        report.push(Violation::new(
            path,
            "has no top-level version string; the release tag is compared against it",
        ));
    }
    Ok(version.map(str::to_owned))
}

/// The pins the sidecars are fetched from, plus the note that notices owns the
/// generated attribution file.
fn check_artefacts(root: &Path, report: &mut Report) -> Result<usize> {
    let path = root.join(SIDECAR_LOCK);
    if !path.is_file() {
        report.push(Violation::new(
            SIDECAR_LOCK,
            "is missing; the sidecar pins are the only source of the bytes a bundle ships",
        ));
        return Ok(1);
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(lock)) => {
            let components = lock.keys().filter(|key| !key.starts_with('$')).count();
            if components == 0 {
                report.push(Violation::new(
                    SIDECAR_LOCK,
                    "declares no sidecar components; an empty lock reads like a complete one",
                ));
            }
        }
        Ok(_) => report.push(Violation::new(
            SIDECAR_LOCK,
            "is not a JSON object of sidecar pins",
        )),
        Err(error) => report.push(Violation::new(
            SIDECAR_LOCK,
            format!("is not readable JSON: {error}"),
        )),
    }
    // THIRD_PARTY.md is covered by the notices check, which compares it byte
    // for byte against its declarations and both lockfiles.
    Ok(1)
}

/// The workflow has to keep both triggers and the four-archive upload, because a
/// release candidate that builds three platforms is not a smaller release.
fn check_workflow(root: &Path, report: &mut Report) -> Result<usize> {
    let path = root.join(WORKFLOW);
    if !path.is_file() {
        report.push(Violation::new(
            WORKFLOW,
            "is missing; the release is built here",
        ));
        return Ok(1);
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;

    if !has_tag_trigger(&text) {
        report.push(Violation::new(
            WORKFLOW,
            "has no v* tag trigger; a release has to be cut from a tag",
        ));
    }
    if !text.contains("workflow_dispatch:") {
        report.push(Violation::new(
            WORKFLOW,
            "has no workflow_dispatch trigger; the dry run would be impossible",
        ));
    }
    for label in BUNDLE_LABELS {
        let marker = format!("label: {label}");
        let present = text.lines().any(|line| {
            let line = line.trim();
            line == marker || line.strip_prefix("- ") == Some(marker.as_str())
        });
        if !present {
            report.push(Violation::new(
                WORKFLOW,
                format!(
                    "does not build the {label} bundle; the four-platform matrix is the release"
                ),
            ));
        }
    }
    if !text.contains("actions/upload-artifact") || !text.contains("bundle-${{ matrix.label }}") {
        report.push(Violation::new(
            WORKFLOW,
            "does not upload bundle-${{ matrix.label }} artifacts; a bundle no job uploads is not a release",
        ));
    }
    if !text.contains("SHA256SUMS") {
        report.push(Violation::new(
            WORKFLOW,
            "does not write SHA256SUMS; an unsigned candidate is only verifiable by its digests",
        ));
    }
    Ok(1)
}

fn has_tag_trigger(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim();
        line.starts_with("tags:") && line.contains("v*")
    }) || text
        .lines()
        .any(|line| matches!(line.trim(), "- \"v*\"" | "- 'v*'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notices::ARTEFACT;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn workspace_root() -> PathBuf {
        crate::workspace::find_root(&PathBuf::from(env!("CARGO_MANIFEST_DIR")))
            .expect("xtask lives inside the workspace")
    }

    const PNPM_LOCK: &str = r"lockfileVersion: '9.0'
importers:
  apps/desktop:
    dependencies:
      react:
        specifier: 19.3.0
        version: 19.3.0
snapshots:
  react@19.3.0: {}
";

    const DECLARATIONS: &str = r#"[[sidecar]]
name = "mpv"
licence = "GPL-2.0-or-later"
version = "pinned-per-os"
source = "https://github.com/mpv-player/mpv"
source_offer = "offered at https://example.invalid/releases"
why_separate = "GPL has to stay outside the process boundary"
build = "upstream release build"
"#;

    const WORKFLOW_TEXT: &str = r#"on:
  push:
    tags: ["v*"]
  workflow_dispatch:
jobs:
  bundle:
    strategy:
      matrix:
        include:
          - label: macos-aarch64
          - label: macos-x86_64
          - label: windows-x86_64
          - label: linux-x86_64
    steps:
      - uses: actions/upload-artifact@v7
        with:
          name: bundle-${{ matrix.label }}
          path: dist/*.tar.gz
  release:
    steps:
      - name: Write SHA256SUMS
        run: sha256sum dist/*.tar.gz > dist/SHA256SUMS
"#;

    /// A complete release tree: the three versions, the sidecar lock, the
    /// declarations and lockfiles notices needs, and a workflow shaped like the
    /// real one. The attribution artefact is generated by the real updater, so
    /// the currency check exercises the same path a maintainer does.
    fn fixture() -> TempDir {
        let root = TempDir::new().expect("a temporary directory");
        std::fs::create_dir_all(root.path().join("app")).unwrap();
        std::fs::create_dir_all(root.path().join("apps/desktop")).unwrap();
        std::fs::create_dir_all(root.path().join("apps/desktop/src-tauri")).unwrap();
        std::fs::create_dir_all(root.path().join("packaging")).unwrap();
        std::fs::create_dir_all(root.path().join(".github/workflows")).unwrap();

        std::fs::write(
            root.path().join(MANIFEST),
            "[workspace]\nmembers = [\"app\"]\n\n[workspace.package]\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("app").join(MANIFEST),
            "[package]\nname = \"peregrine-desktop\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(root.path().join("Cargo.lock"), "version = 4\n").unwrap();
        std::fs::write(root.path().join("pnpm-lock.yaml"), PNPM_LOCK).unwrap();
        std::fs::write(root.path().join("sidecars.toml"), DECLARATIONS).unwrap();
        std::fs::write(root.path().join(TAURI_CONFIG), "{\"version\": \"0.1.0\"}\n").unwrap();
        std::fs::write(root.path().join(PACKAGE_JSON), "{\"version\": \"0.1.0\"}\n").unwrap();
        std::fs::write(
            root.path().join(SIDECAR_LOCK),
            "{\"mpv\": {\"macos-arm64\": {\"version\": \"v0.41.0\"}}}\n",
        )
        .unwrap();
        std::fs::write(root.path().join(WORKFLOW), WORKFLOW_TEXT).unwrap();

        assert!(notices::run(root.path(), notices::Mode::Update).expect("notices update runs"));
        root
    }

    fn configured() -> Signing {
        Signing::Configured
    }

    fn unconfigured() -> Signing {
        Signing::Unconfigured {
            missing: vec!["RELEASE_MINISIGN_SECRET_KEY", "RELEASE_SIGNING_CONFIGURED"],
        }
    }

    fn messages(report: &Report) -> Vec<String> {
        report
            .sorted()
            .into_iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn the_shipped_tree_is_release_ready_with_an_explicit_unsigned_flag() {
        let root = workspace_root();
        let (report, checked) = check(&root, &unconfigured(), true).expect("the tree is readable");
        assert!(report.is_clean(), "{:#?}", report.sorted());
        assert!(checked > 0);
    }

    #[test]
    fn the_fixture_passes_every_check() {
        let root = fixture();
        let (report, _) =
            check(root.path(), &configured(), false).expect("the fixture is readable");
        assert!(report.is_clean(), "{:#?}", report.sorted());
    }

    #[test]
    fn a_version_that_disagrees_with_the_workspace_is_reported() {
        let root = fixture();
        std::fs::write(root.path().join(PACKAGE_JSON), "{\"version\": \"1.0.0\"}\n").unwrap();
        let (report, _) = check(root.path(), &configured(), false).expect("the gate runs");
        let messages = messages(&report);
        assert!(
            messages
                .iter()
                .any(|message| message.contains("package.json")
                    && message.contains("1.0.0")
                    && message.contains("0.1.0")),
            "{messages:#?}"
        );
    }

    #[test]
    fn a_missing_workspace_version_is_reported() {
        let root = fixture();
        std::fs::write(
            root.path().join(MANIFEST),
            "[workspace]\nmembers = [\"app\"]\n",
        )
        .unwrap();
        let (report, _) = check(root.path(), &configured(), false).expect("the gate runs");
        assert!(
            messages(&report)
                .iter()
                .any(|message| message.contains("[workspace.package] version")),
            "{:#?}",
            report.sorted()
        );
    }

    #[test]
    fn a_missing_sidecar_lock_is_reported() {
        let root = fixture();
        std::fs::remove_file(root.path().join(SIDECAR_LOCK)).unwrap();
        let (report, _) = check(root.path(), &configured(), false).expect("the gate runs");
        assert!(
            messages(&report)
                .iter()
                .any(|message| message.contains(SIDECAR_LOCK) && message.contains("missing")),
            "{:#?}",
            report.sorted()
        );
    }

    #[test]
    fn a_stale_attribution_artefact_is_reported_by_the_existing_gate() {
        let root = fixture();
        let path = root.path().join(ARTEFACT);
        let committed = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("{committed}\nstale\n")).unwrap();
        let (report, _) = check(root.path(), &configured(), false).expect("the gate runs");
        assert!(
            messages(&report)
                .iter()
                .any(|message| message.contains(ARTEFACT)
                    && message.contains("check-notices --update")),
            "{:#?}",
            report.sorted()
        );
    }

    #[test]
    fn a_missing_workflow_is_reported() {
        let root = fixture();
        std::fs::remove_file(root.path().join(WORKFLOW)).unwrap();
        let (report, _) = check(root.path(), &configured(), false).expect("the gate runs");
        assert!(
            messages(&report)
                .iter()
                .any(|message| message.contains(WORKFLOW) && message.contains("missing")),
            "{:#?}",
            report.sorted()
        );
    }

    #[test]
    fn a_workflow_that_drops_a_bundle_is_reported() {
        let root = fixture();
        let workflow = WORKFLOW_TEXT.replace("          - label: windows-x86_64\n", "");
        assert_ne!(workflow, WORKFLOW_TEXT);
        std::fs::write(root.path().join(WORKFLOW), workflow).unwrap();
        let (report, _) = check(root.path(), &configured(), false).expect("the gate runs");
        assert!(
            messages(&report)
                .iter()
                .any(|message| message.contains("windows-x86_64")),
            "{:#?}",
            report.sorted()
        );
    }

    #[test]
    fn a_workflow_without_checksums_is_reported() {
        let root = fixture();
        std::fs::write(
            root.path().join(WORKFLOW),
            WORKFLOW_TEXT.replace("SHA256SUMS", "checksums"),
        )
        .unwrap();
        let (report, _) = check(root.path(), &configured(), false).expect("the gate runs");
        assert!(
            messages(&report)
                .iter()
                .any(|message| message.contains("SHA256SUMS")),
            "{:#?}",
            report.sorted()
        );
    }

    #[test]
    fn unsigned_is_refused_unless_the_caller_allows_it() {
        let root = fixture();
        let (report, _) = check(root.path(), &unconfigured(), false).expect("the gate runs");
        assert!(
            messages(&report)
                .iter()
                .any(|message| message.contains("--allow-unsigned")),
            "{:#?}",
            report.sorted()
        );
        let (allowed, _) = check(root.path(), &unconfigured(), true).expect("the gate runs");
        assert!(allowed.is_clean(), "{:#?}", allowed.sorted());
    }

    #[test]
    fn signing_presence_names_what_is_missing_and_never_a_value() {
        let mut env = BTreeMap::new();
        env.insert("RELEASE_MINISIGN_SECRET_KEY", "secret-value");
        let status = signing_status(|name| env.get(name).map(ToString::to_string));
        assert_eq!(
            status,
            Signing::Unconfigured {
                missing: vec![
                    "RELEASE_MINISIGN_PASSWORD",
                    "RELEASE_MINISIGN_PUBLIC_KEY",
                    "RELEASE_SIGNING_CONFIGURED",
                ],
            }
        );
        let rendered = format!("{status:?}");
        assert!(!rendered.contains("secret-value"), "{rendered}");
    }

    #[test]
    fn signing_is_configured_only_when_every_name_and_the_variable_are_present() {
        let mut env = BTreeMap::new();
        for name in SIGNING_SECRETS {
            env.insert(name, "present");
        }
        env.insert(SIGNING_VARIABLE, "true");
        assert_eq!(
            signing_status(|name| env.get(name).map(ToString::to_string)),
            Signing::Configured
        );

        env.insert(SIGNING_VARIABLE, "false");
        assert!(!signing_status(|name| env.get(name).map(ToString::to_string)).is_configured());
    }

    #[test]
    fn a_block_style_tag_trigger_is_recognised() {
        assert!(has_tag_trigger("on:\n  push:\n    tags:\n      - \"v*\"\n"));
        assert!(!has_tag_trigger("on:\n  push:\n    branches: [main]\n"));
    }
}
