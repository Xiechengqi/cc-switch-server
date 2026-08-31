#!/usr/bin/env node

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);

test("generated Web Session manifest stays current", () => {
  const output = execFileSync(
    process.execPath,
    ["scripts/audit/audit-web-session-registry.mjs", "--check"],
    { cwd: repoRoot, encoding: "utf8" },
  );
  assert.match(output, /manifest is current/);
});

test("reviewed Web Session Profiles stay hidden, implemented, and live-pending", () => {
  const manifest = JSON.parse(
    fs.readFileSync(path.join(repoRoot, "assets/contract/web-session-registry-manifest.json")),
  );
  assert.equal(manifest.summary.reviewedProfiles, 2);
  assert.equal(manifest.summary.visibleProfiles, 0);
  assert.equal(manifest.summary.inferenceImplementedProfiles, 2);
  assert.equal(manifest.summary.liveState, "live_pending");
  assert.equal(manifest.invariants.accountPool, false);
  assert.equal(manifest.invariants.crossCredentialRailFallback, false);
  assert.equal(manifest.invariants.authenticationRetry, false);
  assert.equal(manifest.invariants.explicitReimportOn401Or403, true);
});
