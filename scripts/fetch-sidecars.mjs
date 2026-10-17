#!/usr/bin/env node
// Downloads the pinned sidecars for this host and stages them beside the Tauri config, verifying
// every digest. The lock file is the source of truth; this is the only thing that downloads.
//
// A component whose lock entry has a build key has no redistributable artefact and is produced by
// CI instead, so a local run skips it with a note rather than failing.

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
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
import { basename, dirname, join, resolve } from "node:path";
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
      // bsdtar reads "D:\\..." as a remote host:path, so Windows must not see the path in tar.
      execFileSync("powershell", [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        `Expand-Archive -LiteralPath '${archive}' -DestinationPath '${destination}' -Force`,
      ]);
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
      if (entries[0].endsWith(".tar.gz")) {
        execFileSync("tar", ["-xzf", nested, "-C", directory]);
      } else {
        extract(nested, directory, kind);
      }
      rmSync(nested);
      continue;
    }
    return directory;
  }
  return directory;
}

function walk(directory) {
  const found = [];
  for (const entry of readdirSync(directory)) {
    const path = join(directory, entry);
    if (statSync(path).isDirectory()) found.push(...walk(path));
    else found.push(path);
  }
  return found;
}

/** The first file whose path ends with one of the suffixes, with either separator. */
function findBinary(directory, suffix) {
  const wanted = suffix.replaceAll("\\", "/");
  const match = walk(directory).find((path) => path.replaceAll("\\", "/").endsWith(wanted));
  if (match === undefined) throw new Error(`${suffix} is missing after extraction`);
  return match;
}

function ensureDirectory(path) {
  mkdirSync(path, { recursive: true });
}

const stage = join(root, "apps/desktop/src-tauri/sidecars");

for (const component of components) {
  const key = targetKey();
  const entry = lock[component]?.[key];
  if (!entry) throw new Error(`the lock has no ${component} entry for ${key}`);
  if (entry.build) {
    console.log(`${component}: no redistributable for ${key}; it is built in CI (skipped)`);
    continue;
  }

  const isMacMpv = component === "mpv" && key.startsWith("macos");
  const directory = isMacMpv ? join(stage, "mpv.app") : join(stage, component);
  const staged = component === "mpv" ? join(directory, isMacMpv ? "Contents/MacOS/mpv" : "mpv.exe") : directory;
  if (existsSync(staged) && !force) {
    console.log(`${component}: present at ${directory}`);
    continue;
  }

  // One cache entry per digest: ffmpeg and ffprobe share an archive on each platform.
  const cache = join(root, "vendor/cache", entry.sha256);
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
    if (component === "mpv") {
      rmSync(directory, { recursive: true, force: true });
      if (isMacMpv) {
        const app = walk(content).find((path) => basename(path) === "mpv.app");
        if (app === undefined) throw new Error("mpv.app is missing after extraction");
        cpSync(app, directory, { recursive: true });
      } else {
        const binary = findBinary(content, "mpv.exe");
        cpSync(dirname(binary), directory, { recursive: true });
      }
      if (!existsSync(staged)) throw new Error(`${component}: ${staged} is missing after staging`);
      chmodSync(staged, 0o755);
    } else {
      const binary = findBinary(content, entry.binary);
      rmSync(directory, { recursive: true, force: true });
      ensureDirectory(dirname(directory));
      copyFileSync(binary, directory);
      chmodSync(directory, 0o755);
    }
    console.log(`${component}: ${key} ready at ${directory}`);
  } finally {
    rmSync(staging, { recursive: true, force: true });
  }
}
