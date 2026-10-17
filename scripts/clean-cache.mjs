#!/usr/bin/env node
// Removes build output and nothing that was downloaded.
//
// `target/` is the one directory cargo owns: deleting it costs a rebuild and no network. Package
// caches live elsewhere -- ~/.cargo/registry, the pnpm store, and this repository's vendor/cache --
// and this script never touches them. That distinction is the whole point: a full disk should cost
// time to recover, not bandwidth.

import { existsSync, readdirSync, rmSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const args = new Set(process.argv.slice(2));
const dry = args.has("--dry-run");
const incrementalOnly = args.has("--incremental");
const target = join(root, "target");

function size(path) {
  if (!existsSync(path)) return null;
  const result = spawnSync("du", ["-sh", path], { encoding: "utf8" });
  return result.status === 0 ? (result.stdout.split("\t")[0] ?? "").trim() : "?";
}

function remove(path, label) {
  const measured = size(path);
  if (measured === null) return false;
  console.log((dry ? "would remove " : "removing ") + label + " (" + measured + ")");
  if (!dry) rmSync(path, { recursive: true, force: true });
  return true;
}

const targets = [];
if (incrementalOnly) {
  for (const profile of ["debug", "release"]) {
    targets.push([join(target, profile, "incremental"), profile + " incremental"]);
    targets.push([join(target, profile, "bundle"), profile + " bundles"]);
  }
  targets.push([join(target, "doc"), "rustdoc output"]);
  if (existsSync(target)) {
    for (const entry of readdirSync(target)) {
      if (entry.startsWith("chrome-") || entry === "swift-cache" || entry === "corepack") {
        targets.push([join(target, entry), entry]);
      }
    }
  }
} else {
  targets.push([target, "target/ (all build output)"]);
}

let removed = 0;
for (const [path, label] of targets) {
  if (remove(path, label)) removed += 1;
}

if (removed === 0) {
  console.log("nothing to clean: no build output at " + target);
}

console.log("");
console.log("kept, because they are downloads and not build output:");
console.log("  ~/.cargo/registry   crates");
console.log("  the pnpm store      npm packages");
console.log("  vendor/cache        pinned sidecar archives");
