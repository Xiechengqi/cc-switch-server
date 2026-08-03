import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const gateScript = path.join(
  repoRoot,
  "scripts/smoke/code-agent-evidence-gate.mjs",
);
const evidenceWriter = path.join(
  repoRoot,
  "scripts/smoke/write-acceptance-evidence.mjs",
);

function runGate(overrides = {}) {
  const environment = {
    ...process.env,
    FAILURES: "0",
    CONTRACT_FAILURES: "0",
    SKIPPED: "0",
    MATRIX_TOTAL: "1",
    MATRIX_RUNNABLE: "1",
    MATRIX_SKIPPED: "0",
    RUN_REAL: "1",
    RUN_CONTRACT_TESTS: "1",
    CONTRACT_TESTS_PASSED: "1",
    STREAM_PROBE: "1",
    REQUIRE_STREAM_USAGE: "1",
    MATRIX_FIXTURE_EVIDENCE_COMPLETE: "true",
    ...overrides,
  };
  return JSON.parse(
    execFileSync(process.execPath, [gateScript], {
      cwd: repoRoot,
      env: environment,
      encoding: "utf8",
    }),
  );
}

test("reports live_verified only when every live gate is complete", () => {
  const result = runGate();

  assert.equal(result.liveVerificationComplete, true);
  assert.equal(result.status, "pass");
  assert.equal(result.verificationState, "live_verified");
  assert.equal(result.blockerGroup, "");
  assert.deepEqual(result.blockerGroups, []);
});

test("reports every missing live evidence class without collapsing to a token blocker", () => {
  const result = runGate({
    RUN_REAL: "0",
    MATRIX_TOTAL: "4",
    MATRIX_RUNNABLE: "1",
    MATRIX_SKIPPED: "3",
    SKIPPED: "4",
    STREAM_PROBE: "0",
    REQUIRE_STREAM_USAGE: "0",
    MATRIX_FIXTURE_EVIDENCE_COMPLETE: "false",
  });

  assert.equal(result.liveVerificationComplete, false);
  assert.equal(result.verificationState, "contract_verified");
  assert.equal(result.blockerGroup, "missing-matrix-input");
  assert.deepEqual(result.blockerGroups, [
    "missing-matrix-input",
    "missing-stream-evidence",
    "missing-live-fixture-evidence",
    "live-run-disabled",
  ]);
});

test("distinguishes incomplete contracts from failed live probes", () => {
  const contract = runGate({
    RUN_CONTRACT_TESTS: "0",
    CONTRACT_TESTS_PASSED: "0",
    SKIPPED: "1",
  });
  assert.equal(contract.verificationState, "blocked_inputs");
  assert.equal(contract.status, "blocked");
  assert.equal(contract.blockerGroup, "contract-incomplete");

  const liveFailure = runGate({ FAILURES: "1" });
  assert.equal(liveFailure.verificationState, "failed");
  assert.equal(liveFailure.blockerGroup, "live-probe-failed");
  assert.equal(liveFailure.failureClass, "provider-auth-or-transform");
});

test("refuses an empty matrix even when every other gate is green", () => {
  const result = runGate({ MATRIX_TOTAL: "0", MATRIX_RUNNABLE: "0" });

  assert.equal(result.liveVerificationComplete, false);
  assert.equal(result.status, "blocked");
  assert.equal(result.verificationState, "blocked_inputs");
  assert.equal(result.blockerGroup, "contract-incomplete");
});

test("evidence writer rejects code-agent live_verified when a raw gate is missing", () => {
  const directory = fs.mkdtempSync(
    path.join(os.tmpdir(), "cc-switch-evidence-"),
  );
  const output = path.join(directory, "evidence.json");
  const result = spawnSync(
    process.execPath,
    [evidenceWriter, "--out", output],
    {
      cwd: repoRoot,
      env: {
        ...process.env,
        EVIDENCE_TARGET: "code-agent-matrix",
        EVIDENCE_VERIFICATION_STATE: "live_verified",
        FAILURES: "0",
        CONTRACT_FAILURES: "0",
        RUN_REAL: "1",
        RUN_CONTRACT_TESTS: "1",
        CONTRACT_TESTS_PASSED: "1",
        STREAM_PROBE: "0",
        REQUIRE_STREAM_USAGE: "1",
        MATRIX_FIXTURE_EVIDENCE_COMPLETE: "true",
        MATRIX_FIXTURE_EVIDENCE_MISSING: "0",
        MATRIX_TOTAL: "1",
        MATRIX_RUNNABLE: "1",
        MATRIX_SKIPPED: "0",
        SKIPPED: "0",
        LIVE_VERIFICATION_COMPLETE: "0",
      },
      encoding: "utf8",
    },
  );

  assert.equal(result.status, 2);
  assert.match(result.stderr, /refusing to write code-agent live_verified/);
  assert.equal(fs.existsSync(output), false);
});

test("evidence writer accepts code-agent live_verified with every raw gate", () => {
  const directory = fs.mkdtempSync(
    path.join(os.tmpdir(), "cc-switch-evidence-"),
  );
  const output = path.join(directory, "evidence.json");
  const result = spawnSync(
    process.execPath,
    [evidenceWriter, "--out", output],
    {
      cwd: repoRoot,
      env: {
        ...process.env,
        EVIDENCE_TARGET: "code-agent-matrix",
        EVIDENCE_VERIFICATION_STATE: "live_verified",
        FAILURES: "0",
        CONTRACT_FAILURES: "0",
        RUN_REAL: "1",
        RUN_CONTRACT_TESTS: "1",
        CONTRACT_TESTS_PASSED: "1",
        STREAM_PROBE: "1",
        REQUIRE_STREAM_USAGE: "1",
        MATRIX_FIXTURE_EVIDENCE_COMPLETE: "true",
        MATRIX_FIXTURE_EVIDENCE_MISSING: "0",
        MATRIX_TOTAL: "1",
        MATRIX_RUNNABLE: "1",
        MATRIX_SKIPPED: "0",
        SKIPPED: "0",
        LIVE_VERIFICATION_COMPLETE: "1",
      },
      encoding: "utf8",
    },
  );

  assert.equal(result.status, 0, result.stderr);
  const evidence = JSON.parse(fs.readFileSync(output, "utf8"));
  assert.equal(evidence.verificationState, "live_verified");
});
