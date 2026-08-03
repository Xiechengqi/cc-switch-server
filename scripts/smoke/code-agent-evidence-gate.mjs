#!/usr/bin/env node

function flag(name) {
  const value = process.env[name] || "0";
  if (value === "1" || value === "true") return true;
  if (value === "0" || value === "false") return false;
  throw new Error(`${name} must be 0, 1, true, or false`);
}

function count(name) {
  const value = process.env[name] || "0";
  if (!/^\d+$/.test(value)) {
    throw new Error(`${name} must be a non-negative integer`);
  }
  return Number.parseInt(value, 10);
}

function addUnique(items, item) {
  if (!items.includes(item)) items.push(item);
}

const failures = count("FAILURES");
const contractFailures = count("CONTRACT_FAILURES");
const skipped = count("SKIPPED");
const matrixTotal = count("MATRIX_TOTAL");
const matrixRunnable = count("MATRIX_RUNNABLE");
const matrixSkipped = count("MATRIX_SKIPPED");
const runReal = flag("RUN_REAL");
const runContractTests = flag("RUN_CONTRACT_TESTS");
const contractTestsPassed = flag("CONTRACT_TESTS_PASSED");
const streamProbe = flag("STREAM_PROBE");
const requireStreamUsage = flag("REQUIRE_STREAM_USAGE");
const fixtureEvidenceComplete = flag("MATRIX_FIXTURE_EVIDENCE_COMPLETE");
if (contractFailures > failures) {
  throw new Error("CONTRACT_FAILURES cannot exceed FAILURES");
}
const nonContractFailures = Math.max(0, failures - contractFailures);
const contractComplete =
  runContractTests && contractTestsPassed && contractFailures === 0;
const matrixShapeComplete =
  matrixTotal > 0 && matrixRunnable + matrixSkipped === matrixTotal;
const matrixInputComplete =
  matrixShapeComplete && matrixSkipped === 0 && matrixRunnable === matrixTotal;
const contractBaselineComplete = contractComplete && matrixShapeComplete;

const blockerGroups = [];
if (contractFailures > 0 || !contractComplete || !matrixShapeComplete) {
  addUnique(blockerGroups, "contract-incomplete");
}
if (nonContractFailures > 0) {
  addUnique(blockerGroups, "live-probe-failed");
}
if (matrixSkipped > 0) {
  addUnique(blockerGroups, "missing-matrix-input");
}
if (!streamProbe || !requireStreamUsage) {
  addUnique(blockerGroups, "missing-stream-evidence");
}
if (!fixtureEvidenceComplete) {
  addUnique(blockerGroups, "missing-live-fixture-evidence");
}
if (!runReal) {
  addUnique(blockerGroups, "live-run-disabled");
}

const expectedContractSkip = runContractTests ? 0 : 1;
if (matrixSkipped === 0 && skipped > expectedContractSkip) {
  addUnique(blockerGroups, "live-probe-skipped");
}

const liveVerificationComplete =
  failures === 0 &&
  runReal &&
  contractComplete &&
  streamProbe &&
  requireStreamUsage &&
  fixtureEvidenceComplete &&
  matrixInputComplete &&
  skipped === 0;

let status = contractBaselineComplete
  ? "ready-with-known-external-blockers"
  : "blocked";
let verificationState = contractBaselineComplete
  ? "contract_verified"
  : "blocked_inputs";
let failureClass = "";

if (failures > 0) {
  status = "fail";
  verificationState = "failed";
  if (contractFailures > 0 && nonContractFailures > 0) {
    failureClass = "contract-and-live-probe";
  } else if (contractFailures > 0) {
    failureClass = "contract";
  } else {
    failureClass = "provider-auth-or-transform";
  }
} else if (liveVerificationComplete) {
  status = "pass";
  verificationState = "live_verified";
}

console.log(
  JSON.stringify(
    {
      liveVerificationComplete,
      status,
      verificationState,
      blockerGroup: blockerGroups[0] || "",
      blockerGroups,
      failureClass,
    },
    null,
    2,
  ),
);
