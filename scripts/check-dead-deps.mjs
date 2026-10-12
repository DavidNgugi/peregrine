#!/usr/bin/env node
// `pnpm check-dead-deps` — the frontend half of PG-E0-15: no workspace manifest
// may declare a dependency its sources never use.
//
// The Rust half wraps `cargo-machete` (xtask/src/deaddeps.rs); this wraps `knip`.
// A clean scan is not by itself evidence that the gate works, because a knip
// configuration that analyses nothing also exits zero, so a second run happens
// against a fixture package that is known to violate the rule. If knip ever
// stops detecting unused dependencies — through an upgrade, or through an
// `ignoreDependencies` entry that swallows the fixture — this command fails
// loudly instead of reporting a clean tree.
//
// The fixture is deliberately outside `pnpm-workspace.yaml`, so the real scan
// never sees it. `xtask/fixtures/dead-deps/unused-dep` is kept out of the Cargo
// workspace for the same reason.
//
// Findings are read from knip's JSON reporter rather than from the human
// reporter because knip also exits 1 when it cannot start at all (a missing
// native parser, say). Parsing the report separates "the tool found something"
// from "the tool did not run", which a bare exit code cannot.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));

const CONFIG = join(root, "knip.jsonc");
const VIOLATION_FIXTURE = "xtask/fixtures/dead-deps/unused-pnpm-dep";

// The issue types `knip.jsonc` puts in scope, and how a finding of each type is
// described to the person who has to fix it.
const SECTIONS = [
  ["dependencies", "unused dependency"],
  ["devDependencies", "unused devDependency"],
  ["optionalPeerDependencies", "unused optional peerDependency"],
  ["unlisted", "unlisted dependency"],
  ["binaries", "unlisted binary"],
];

// `knip`'s exit codes: 0 is clean and 1 is "issues found". Anything else — 2, or
// a signal — is knip failing to run, which must not be mistaken for either.
const CLEAN = 0;
const ISSUES_FOUND = 1;

// The CLI entry point is named rather than looked up on PATH: a shell lookup
// would need `shell: true`, and on Windows that would also mean quoting a
// repository path that may contain spaces.
const knip = join(dirname(createRequire(import.meta.url).resolve("knip")), "..", "bin", "knip.js");

function findings(args) {
  const result = spawnSync(
    process.execPath,
    [knip, "--no-progress", "--reporter", "json", ...args],
    {
      cwd: root,
      encoding: "utf8",
    },
  );
  if (result.error) {
    throw new Error(`failed to run ${knip}: ${result.error.message}`);
  }
  if (result.status !== CLEAN && result.status !== ISSUES_FOUND) {
    throw new Error(`knip exited ${result.status}:\n${result.stdout}${result.stderr}`);
  }
  let report;
  try {
    report = JSON.parse(result.stdout);
  } catch {
    throw new Error(
      `knip exited ${result.status} without reporting JSON, so it did not run:\n` +
        `${result.stdout}${result.stderr}`,
    );
  }
  return report.issues ?? [];
}

// The declared names are read from the fixture rather than written out here, so
// the check cannot drift from the manifest it is checking.
function fixtureDependencies() {
  const manifest = JSON.parse(readFileSync(join(root, VIOLATION_FIXTURE, "package.json"), "utf8"));
  const declared = Object.keys(manifest.dependencies ?? {});
  if (declared.length === 0) {
    throw new Error(`${VIOLATION_FIXTURE} declares no dependencies, so it proves nothing`);
  }
  return declared;
}

function describe(issues) {
  const lines = [];
  for (const issue of issues) {
    for (const [section, label] of SECTIONS) {
      for (const item of issue[section] ?? []) {
        const where = item.line ? `${issue.file}:${item.line}:${item.col}` : issue.file;
        lines.push(`  [${label}] ${item.name}  ${where}`);
      }
    }
  }
  return lines;
}

// Returns whether the workspace is clean, and reports the findings when it is not.
function checkWorkspace() {
  const lines = describe(findings(["--config", CONFIG]));
  if (lines.length === 0) {
    return true;
  }
  process.stderr.write(
    `check-dead-deps: unused dependencies found:\n${lines.join("\n")}\n` +
      "  Remove the dependency, or move it to the manifest that actually uses it.\n",
  );
  return false;
}

function checkViolationFixture() {
  const declared = fixtureDependencies();
  // The same configuration file the real scan uses, so an ignore added there
  // that also swallows this fixture is a gate failure rather than a blind spot.
  const issues = findings(["--directory", VIOLATION_FIXTURE, "--config", CONFIG]);
  const reported = new Set(
    issues.flatMap((issue) => (issue.dependencies ?? []).map((d) => d.name)),
  );
  const missing = declared.filter((name) => !reported.has(name));
  if (missing.length > 0) {
    throw new Error(
      `the gate is broken: ${VIOLATION_FIXTURE} declares ${declared.join(", ")}, which its ` +
        `sources never import, but knip did not report ${missing.join(", ")}. ` +
        "Fix the gate before trusting a green run.",
    );
  }
}

try {
  if (checkWorkspace()) {
    checkViolationFixture();
    console.log("check-dead-deps: ok (frontend workspace clean, violation fixture still detected)");
  } else {
    process.exitCode = 1;
  }
} catch (error) {
  process.stderr.write(`check-dead-deps: ${error.message}\n`);
  process.exitCode = 1;
}
