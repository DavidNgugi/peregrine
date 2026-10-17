#!/usr/bin/env node
// Downloads the pinned sidecars for this host into vendor/sidecars, verifying every digest.
//
// The lock file is the source of truth; this script is the only thing that downloads. An entry
// with a build key has no redistributable artefact and is produced by CI instead, so a local run
// skips it with a note rather than failing.

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import {
  chmodSync,
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const args = new Set(process.argv.slice(2));
const force = args.has("--force");
const components = args.has("--all") ? ["mpv", "ffmpeg", "ffprobe"] : ["mpv", "ffprobe"];

const lock = JSON.parse(readFileSync(join(root, "packaging/sidecars.lock.json"), "utf8"));

function targetKey() {
  if (process.env.PEREGRINE_TARGET) return process.env.PEREGRINE_TARGET;
  const arch = process.arch === "arm64" ? "arm64" : "x86_64";
  switch (process.platform) {
    case "darwin":
      return `macos-${arch}`;
    case "linux":
      return `linux-${arch}`;
    case "win32":
      return `windows-${arch}`;
    default:
      throw new Error(`unsupported host: ${process.platform}/${process.arch}`);
  }
}

function sha256(file) {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

function archiveKind(url) {
  if (url.endsWith(".tar.xz")) return "tar.xz";
  if (url.endsWith(".zip")) return "zip";
  throw new Error(`unknown archive: ${url}`);
}

async function download(url, destination) {
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok) throw new Error(`${url} answered ${response.status}`);
  writeFileSync(destination, Buffer.from(await response.arrayBuffer()));
}

function extract(archive, destination, kind) {
  if (kind === "zip") {
    if (process.platform === "win32") {
      execFileSync("tar", ["-xf", archive, "-C", destination]);
    } else {
      execFileSync("unzip", ["-q", archive, "-d", destination]);
    }
  } else {
    execFileSync("tar", ["-xJf", archive, "-C", destination]);
  }
}

const NESTED = /\.(zip|tar\.gz|tgz|tar\.xz)$/;

function hoist(directory) {
  for (let depth = 0; depth < 3; depth += 1) {
    const entries = readdirSync(directory);
    if (entries.length === 1 && NESTED.test(entries[0])) {
      const nested = join(directory, entries[0]);
      const kind = entries[0].endsWith(".zip") ? "zip" : "tar.xz";
      if (kind === "tar.xz" || entries[0].endsWith(".tar.gz")) {
        execFileSync("tar", [entries[0].endsWith(".tar.gz") ? "-xzf" : "-xJf", nested, "-C", directory]);
      } else {
        extract(nested, directory, "zip");
      }
      rmSync(nested);
      continue;
    }
    return directory;
  }
  return directory;
}

function ensureDirectory(path) {
  mkdirSync(path, { recursive: true });
}

for (const component of components) {
  const key = targetKey();
  const entry = lock[component]?.[key];
  if (!entry) throw new Error(`the lock has no ${component} entry for ${key}`);
  if (entry.build) {
    console.log(`${component}: no redistributable for ${key}; it is built in CI (skipped)`);
    continue;
  }

  const stage = join(root, "apps/desktop/src-tauri/sidecars");
  const isMacMpv = component === "mpv" && key.startsWith("macos");
  const destination = isMacMpv ? join(stage, "mpv.app") : join(stage, component);
  const binary = isMacMpv ? join(destination, "Contents/MacOS/mpv") : join(destination, entry.binary);
  if (existsSync(binary) && !force) {
    console.log(`${component}: present at ${destination}`);
    continue;
  }

  const cache = join(root, "vendor/cache", `${entry.sha256}-${key}-${component}`);
  ensureDirectory(dirname(cache));
  if (!existsSync(cache) || force) {
    console.log(`${component}: downloading ${entry.url}`);
    await download(entry.url, cache);
  }
  const digest = sha256(cache);
  if (digest !== entry.sha256) {
    rmSync(cache);
    throw new Error(`${component}: digest mismatch\n expected ${entry.sha256}\n actual   ${digest}`);
  }

  const staging = mkdtempSync(join(tmpdir(), "peregrine-sidecar-"));
  try {
    extract(cache, staging, archiveKind(entry.url));
    const content = hoist(staging);
    let source = content;
    if (component !== "mpv") {
      const entries = readdirSync(content);
      if (entries.length === 1 && statSync(join(content, entries[0])).isDirectory()) {
        source = join(content, entries[0]);
      }
    }
    rmSync(destination, { recursive: true, force: true });
    ensureDirectory(destination);
    cpSync(source, destination, { recursive: true });
    if (!existsSync(binary)) {
      throw new Error(`${component}: ${entry.binary} is missing after extraction`);
    }
    chmodSync(binary, 0o755);
    console.log(`${component}: ${key} ready at ${destination}`);
  } finally {
    rmSync(staging, { recursive: true, force: true });
  }
}
