#!/usr/bin/env node

import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const root = path.resolve(new URL("../..", import.meta.url).pathname);
const manifestPath = path.join(root, "assets/contract/provider-enhancement-committed-target.json");
const raw = fs.readFileSync(manifestPath);
const digest = (value) => crypto.createHash("sha256").update(value).digest("hex");
const assert = (condition, message) => {
  if (!condition) throw new Error(message);
};
const git = (...args) => execFileSync("git", ["-C", root, ...args], {
  encoding: "utf8",
  maxBuffer: 64 * 1024 * 1024,
}).trim();
const gitFile = (commit, relativePath) => execFileSync(
  "git", ["-C", root, "show", `${commit}:${relativePath}`],
  { maxBuffer: 64 * 1024 * 1024 },
);
const safePath = (value) => typeof value === "string" &&
  value.length > 0 && !path.isAbsolute(value) &&
  !value.split("/").includes("..") && !value.split("/").includes("");
const commitPattern = /^[a-f0-9]{40}$/;
const digestPattern = /^[a-f0-9]{64}$/;

assert(
  digest(raw) === "f2075c8d21082d6b0edecef50f0e2ea627552bec59d6ea8b6a9f63365d0ee3be",
  "committed-target observation changed; append a new revision instead",
);
const observation = JSON.parse(raw);
assert(
  observation.format === "cc-switch-provider-enhancement-committed-target" &&
  observation.schemaVersion === 1 &&
  Number.isFinite(Date.parse(observation.observedAt)) &&
  observation.readMode === "committed_git_objects" &&
  observation.evidenceClass === "offline_fixture_only" &&
  observation.liveAcceptance === "live_pending_outside_this_goal",
  "committed-target evidence boundary changed",
);
assert(
  commitPattern.test(observation.baselineCommit) &&
  commitPattern.test(observation.implementationCommit) &&
  commitPattern.test(observation.implementationTree) &&
  observation.baselineCommit !== observation.implementationCommit,
  "invalid committed target identity",
);
assert(
  git("rev-parse", `${observation.implementationCommit}^{tree}`) === observation.implementationTree &&
  git("rev-parse", `${observation.implementationCommit}^`) === observation.baselineCommit,
  "committed target tree or parent changed",
);

const families = ["antigravity", "claude", "codex", "cursor", "grok", "kiro", "qoder", "codebuddy"];
assert(
  JSON.stringify(observation.providers?.map((provider) => provider.family)) === JSON.stringify(families),
  "committed-target provider inventory changed",
);
const changedPaths = new Set(git("diff", "--name-only", observation.baselineCommit, observation.implementationCommit).split("\n"));
for (const provider of observation.providers) {
  const { family, referenceDeltaPath, referenceDeltaSha256, implementationPath, fixturePath, fixtureAnchor } = provider;
  assert(referenceDeltaPath === `assets/contract/${family}-reference-delta.json`, `${family} reference path changed`);
  assert(
    [referenceDeltaPath, implementationPath, fixturePath].every(safePath) &&
    digestPattern.test(referenceDeltaSha256) &&
    typeof fixtureAnchor === "string" && fixtureAnchor.length > 8,
    `${family} has invalid path, digest, or fixture anchor`,
  );
  assert(
    digest(gitFile(observation.implementationCommit, referenceDeltaPath)) === referenceDeltaSha256,
    `${family} reference delta does not match the committed target`,
  );
  assert(changedPaths.has(implementationPath) && changedPaths.has(fixturePath), `${family} implementation or fixture was not committed`);
  gitFile(observation.implementationCommit, implementationPath);
  assert(
    gitFile(observation.implementationCommit, fixturePath).toString("utf8").includes(fixtureAnchor),
    `${family} committed fixture anchor is missing`,
  );
}
console.log(`Eight-provider committed-target observation verified: ${observation.implementationCommit}`);
