#!/usr/bin/env node
/**
 * Sync selected desktop UI files from a pinned, reviewed upstream commit.
 *
 * Usage:
 *   node scripts/sync/sync-desktop-ui.mjs [--check] [path...]
 *   node scripts/sync/sync-desktop-ui.mjs --update-pinned-hashes
 *   node scripts/sync/sync-desktop-ui.mjs --refresh-upstream
 */
import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const manifestPath = process.env.CC_SWITCH_DESKTOP_SYNC_MANIFEST
  ? path.resolve(process.env.CC_SWITCH_DESKTOP_SYNC_MANIFEST)
  : path.join(repoRoot, "assets/contract/desktop-ui-sync.json");
const serverWebSrc = path.join(repoRoot, "web-src/src");
const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
const desktopRoot =
  process.env.CC_SWITCH_DESKTOP_ROOT || manifest.upstream.repository;

const testSuffixes = [".test.ts", ".test.tsx", ".spec.ts", ".spec.tsx"];

function git(args, options = {}) {
  return execFileSync("git", ["-C", desktopRoot, ...args], {
    encoding: options.buffer ? null : "utf8",
    maxBuffer: 32 * 1024 * 1024,
    stdio: ["ignore", "pipe", options.quiet ? "ignore" : "pipe"],
  });
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

export function normalizeRelative(relativePath) {
  return relativePath.replaceAll("\\", "/").replace(/^\.\//, "").replace(/\/$/, "");
}

function sourcePath(relativePath) {
  return `src/${normalizeRelative(relativePath)}`;
}

function readPinnedSource(relativePath, commit = manifest.upstream.commit) {
  try {
    const content = git(
      ["show", `${commit}:${sourcePath(relativePath)}`],
      { buffer: true, quiet: true },
    );
    rejectConflictMarkers(relativePath, content);
    return content;
  } catch (error) {
    if (error instanceof Error && error.message.includes("conflict marker")) {
      throw error;
    }
    return null;
  }
}

export function rejectConflictMarkers(relativePath, content) {
  const text = content.toString("utf8");
  if (
    text.includes("<<<<<<< ") ||
    text.includes("\n=======\n") ||
    text.includes("\n>>>>>>> ")
  ) {
    throw new Error(`pinned upstream source contains conflict marker: ${relativePath}`);
  }
}

function listPinnedFiles(relativePath) {
  const root = sourcePath(relativePath);
  const output = git([
    "ls-tree",
    "-r",
    "--name-only",
    manifest.upstream.commit,
    "--",
    root,
  ]).trim();
  if (!output) {
    throw new Error(`missing pinned upstream source: ${root}`);
  }
  return output
    .split("\n")
    .filter(Boolean)
    .map((file) => normalizeRelative(file.replace(/^src\//, "")));
}

function walkFiles(root) {
  if (!fs.existsSync(root)) return [];
  const stat = fs.statSync(root);
  if (stat.isFile()) return [root];
  return fs
    .readdirSync(root, { withFileTypes: true })
    .flatMap((entry) => walkFiles(path.join(root, entry.name)));
}

function toWebRelative(file) {
  return normalizeRelative(path.relative(serverWebSrc, file));
}

function isTestFile(relativePath) {
  return testSuffixes.some((suffix) => relativePath.endsWith(suffix));
}

const serverOwnedByPath = new Map(
  manifest.serverOwned.map((entry) => [normalizeRelative(entry.path), entry]),
);

function excludedRule(relativePath) {
  return manifest.excluded.find((entry) => {
    const rulePath = normalizeRelative(entry.path);
    return entry.kind === "prefix"
      ? relativePath === rulePath || relativePath.startsWith(`${rulePath}/`)
      : relativePath === rulePath;
  });
}

export function validateManifestShape(candidate) {
  if (candidate?.schemaVersion !== 1) {
    throw new Error(`unsupported desktop UI sync manifest schema: ${candidate?.schemaVersion}`);
  }
  if (
    !candidate.upstream ||
    typeof candidate.upstream.repository !== "string" ||
    !candidate.upstream.repository ||
    !/^[0-9a-f]{40}$/.test(candidate.upstream.commit ?? "")
  ) {
    throw new Error("desktop UI sync manifest must pin a full upstream commit");
  }
  if (!Array.isArray(candidate.syncRoots) || candidate.syncRoots.length === 0) {
    throw new Error("desktop UI sync manifest requires syncRoots");
  }
  const roots = candidate.syncRoots.map(normalizeRelative);
  if (roots.some((root) => !root) || new Set(roots).size !== roots.length) {
    throw new Error("desktop UI sync roots must be non-empty and unique");
  }
  if (!Array.isArray(candidate.serverOwned) || !Array.isArray(candidate.excluded)) {
    throw new Error("desktop UI sync manifest requires serverOwned and excluded arrays");
  }
  const seen = new Set();
  for (const entry of candidate.serverOwned) {
    const relativePath = normalizeRelative(entry.path);
    if (!relativePath || seen.has(relativePath)) {
      throw new Error(`duplicate server-owned sync entry: ${relativePath}`);
    }
    seen.add(relativePath);
    if (
      entry.owner !== "server" ||
      !entry.reason ||
      !/^[0-9a-f]{40}$/.test(entry.lastReviewedUpstreamCommit ?? "") ||
      !/^[0-9a-f]{40}$/.test(entry.lastReviewedServerCommit ?? "") ||
      !entry.exitPhase ||
      (entry.upstreamSourceSha256 !== null &&
        !/^[0-9a-f]{64}$/.test(entry.upstreamSourceSha256 ?? ""))
    ) {
      throw new Error(`incomplete server-owned sync entry: ${relativePath}`);
    }
    if (entry.lastReviewedUpstreamCommit !== candidate.upstream.commit) {
      throw new Error(`stale reviewed commit for ${relativePath}`);
    }
  }
  const excluded = new Set();
  for (const entry of candidate.excluded) {
    const relativePath = normalizeRelative(entry.path);
    if (
      !relativePath ||
      excluded.has(relativePath) ||
      !["exact", "prefix"].includes(entry.kind) ||
      !entry.reason
    ) {
      throw new Error(`invalid excluded sync entry: ${relativePath}`);
    }
    if (seen.has(relativePath)) {
      throw new Error(`sync path cannot be both server-owned and excluded: ${relativePath}`);
    }
    excluded.add(relativePath);
  }
}

export function validatePinnedManifest(
  candidate,
  { resolveCommit, readSource },
) {
  validateManifestShape(candidate);
  const actualCommit = resolveCommit(candidate.upstream.commit);
  if (actualCommit !== candidate.upstream.commit) {
    throw new Error("pinned upstream commit does not resolve exactly");
  }
  for (const entry of candidate.serverOwned) {
    const relativePath = normalizeRelative(entry.path);
    const source = readSource(relativePath, candidate.upstream.commit);
    if (source) rejectConflictMarkers(relativePath, source);
    const actualHash = source ? sha256(source) : null;
    if (entry.upstreamSourceSha256 !== actualHash) {
      throw new Error(
        `stale upstream hash for ${relativePath}; run --update-pinned-hashes after review`,
      );
    }
  }
}

function validateManifest() {
  validatePinnedManifest(manifest, {
    resolveCommit: (commit) => git(["rev-parse", commit]).trim(),
    readSource: (relativePath, commit) => readPinnedSource(relativePath, commit),
  });
}

function updatePinnedHashes({ refreshCommit }) {
  if (refreshCommit) {
    const status = git(["status", "--porcelain=v1"]).trim();
    if (status) {
      throw new Error(
        "refusing to refresh desktop UI baseline from a dirty or conflicted upstream worktree",
      );
    }
    manifest.upstream.commit = git(["rev-parse", "HEAD"]).trim();
  }

  for (const entry of manifest.serverOwned) {
    const source = readPinnedSource(entry.path);
    entry.upstreamSourceSha256 = source ? sha256(source) : null;
    entry.lastReviewedUpstreamCommit = manifest.upstream.commit;
  }
  fs.writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(
    refreshCommit
      ? `desktop UI baseline refreshed at ${manifest.upstream.commit}`
      : `desktop UI hashes refreshed from pinned ${manifest.upstream.commit}`,
  );
}

function copyOrCheck(relativePath, source, checkOnly) {
  if (isTestFile(relativePath) || excludedRule(relativePath)) return;
  if (serverOwnedByPath.has(relativePath)) return;

  const destination = path.join(serverWebSrc, relativePath);
  if (checkOnly) {
    if (!fs.existsSync(destination)) {
      throw new Error(`missing server target: ${destination}`);
    }
    if (!source.equals(fs.readFileSync(destination))) {
      throw new Error(`server copy drifted from pinned desktop: ${destination}`);
    }
    return;
  }

  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(destination, source);
  console.log(`synced ${sourcePath(relativePath)} -> ${path.relative(repoRoot, destination)}`);
}

function processRoot(relativeRoot, checkOnly) {
  const sourceFiles = listPinnedFiles(relativeRoot);
  const sourceSet = new Set(sourceFiles);
  const failures = [];
  for (const relativePath of sourceFiles) {
    try {
      const source = readPinnedSource(relativePath);
      if (!source) throw new Error(`missing pinned upstream blob: ${relativePath}`);
      copyOrCheck(relativePath, source, checkOnly);
    } catch (error) {
      failures.push(error instanceof Error ? error.message : String(error));
    }
  }

  const destinationRoot = path.join(serverWebSrc, normalizeRelative(relativeRoot));
  if (!fs.existsSync(destinationRoot) || fs.statSync(destinationRoot).isFile()) return;
  for (const file of walkFiles(destinationRoot)) {
    const relativePath = toWebRelative(file);
    if (isTestFile(relativePath)) continue;
    if (sourceSet.has(relativePath)) continue;
    if (serverOwnedByPath.has(relativePath) || excludedRule(relativePath)) continue;
    failures.push(`unexpected server-local file in synced tree: ${file}`);
  }
  if (failures.length > 0) throw new Error(failures.join("\n"));
}

function main() {
  const args = process.argv.slice(2);
  if (args.includes("--update-pinned-hashes")) {
    updatePinnedHashes({ refreshCommit: false });
    return;
  }
  if (args.includes("--refresh-upstream")) {
    updatePinnedHashes({ refreshCommit: true });
    return;
  }

  validateManifest();
  const checkOnly = args.includes("--check");
  const selected = args.filter((arg) => !arg.startsWith("--"));
  const roots = selected.length > 0 ? selected : manifest.syncRoots;
  const failures = [];
  for (const relativeRoot of roots) {
    try {
      processRoot(relativeRoot, checkOnly);
    } catch (error) {
      failures.push(error instanceof Error ? error.message : String(error));
    }
  }

  if (failures.length > 0) {
    for (const failure of [...new Set(failures)]) console.error(failure);
    console.error(`sync-desktop-ui failed: ${failures.length} root(s)`);
    process.exit(1);
  }
  console.log(
    checkOnly
      ? `sync-desktop-ui check ok at ${manifest.upstream.commit}`
      : `sync-desktop-ui complete from ${manifest.upstream.commit}`,
  );
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main();
}
