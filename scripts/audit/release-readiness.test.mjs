import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);
const readinessScript = path.join(repoRoot, "scripts/release-readiness.sh");

test("skipped local contracts block release readiness and evidence", () => {
  const directory = fs.mkdtempSync(
    path.join(os.tmpdir(), "cc-switch-release-readiness-"),
  );
  const evidenceFile = path.join(directory, "evidence.json");
  const environment = {
    ...process.env,
    RUN_TESTS: "0",
    RUN_REAL: "0",
    RUN_DEPLOYMENT_TESTS: "0",
    EVIDENCE_FILE: evidenceFile,
    CC_SWITCH_SERVER_TOKEN: "",
    SHARE_ID: "",
    CC_SWITCH_SHARE_URL: "",
    ROUTER_API_TOKEN: "",
    MARKET_API_URL: "",
    CLAUDE_PROVIDER_TOKEN: "",
    CODEX_PROVIDER_TOKEN: "",
    GEMINI_PROVIDER_TOKEN: "",
    CC_SWITCH_CODEX_IMAGES_SMOKE: "0",
  };

  const result = spawnSync("bash", [readinessScript], {
    cwd: repoRoot,
    env: environment,
    encoding: "utf8",
  });

  assert.equal(result.status, 1, result.stderr);
  assert.match(result.stdout, /\[BLOCKED-INTERNAL\] local-contracts-unverified/);
  assert.match(result.stdout, /decision=blocked/);
  assert.match(result.stdout, /verificationState=blocked_inputs/);
  assert.doesNotMatch(
    result.stdout,
    /decision=ready-with-known-external-blockers/,
  );

  const evidence = JSON.parse(fs.readFileSync(evidenceFile, "utf8"));
  assert.equal(evidence.status, "blocked");
  assert.equal(evidence.verificationState, "blocked_inputs");
  assert.equal(evidence.blockerGroup, "local-contracts-unverified");
  assert.equal(evidence.checks.releaseDecision, "blocked");
  assert.equal(evidence.checks.blockedGroups, "local-contracts-unverified");
  assert.equal(evidence.checks.codexImagesGateStatus, "disabled");
  assert.match(evidence.notes, /internalBlockers=local-contracts-unverified/);
});
