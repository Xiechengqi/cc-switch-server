#!/usr/bin/env node
import fs from "node:fs";

const matrixPath = process.argv[2] || "docs/code-agent-regression-matrix.json";
const matrix = JSON.parse(fs.readFileSync(matrixPath, "utf8"));
const fixtureEvidencePath =
  process.env.MATRIX_LIVE_EVIDENCE_FILE || process.argv[3] || "";

if (matrix.schemaVersion !== 2) {
  throw new Error(
    `unsupported code-agent matrix schemaVersion: ${matrix.schemaVersion}`,
  );
}

function env(name) {
  if (name === "SERVER_URL") {
    return process.env.SERVER_URL || "http://127.0.0.1:15721";
  }
  return process.env[name] || "";
}

function present(name) {
  const value = env(name);
  return Boolean(value && !value.startsWith("<"));
}

function valueFor(primary, fallback) {
  if (primary && present(primary)) return env(primary);
  if (fallback && present(fallback)) return env(fallback);
  return "";
}

function unique(values) {
  return [...new Set(values.filter(Boolean))].sort();
}

function loadFixtureEvidence() {
  if (!fixtureEvidencePath || fixtureEvidencePath.startsWith("<")) return null;
  const evidence = JSON.parse(fs.readFileSync(fixtureEvidencePath, "utf8"));
  if (evidence.schemaVersion !== 1) {
    throw new Error(
      `unsupported matrix live evidence schemaVersion: ${evidence.schemaVersion}`,
    );
  }
  if (
    !evidence.cases ||
    typeof evidence.cases !== "object" ||
    Array.isArray(evidence.cases)
  ) {
    throw new Error(
      "matrix live evidence cases must be an object keyed by case id",
    );
  }
  return evidence;
}

const fixtureEvidence = loadFixtureEvidence();
const requiredFixtureFields = matrix.requiredFixtureFields || [];

function fixtureEvidenceFor(testCase) {
  const evidenceCase = fixtureEvidence?.cases?.[testCase.id];
  const checks = evidenceCase?.checks;
  const missing = requiredFixtureFields.filter(
    (field) => !checks || checks[field] !== "passed",
  );
  if (
    !evidenceCase?.evidencePath ||
    typeof evidenceCase.evidencePath !== "string"
  ) {
    missing.push("evidencePath");
  }
  return {
    complete: missing.length === 0,
    evidencePath: evidenceCase?.evidencePath || "",
    missing,
  };
}

function missingFor(testCase) {
  const missing = [];
  if (testCase.requiresServerToken && !present("CC_SWITCH_SERVER_TOKEN")) {
    missing.push("CC_SWITCH_SERVER_TOKEN");
  }
  if (testCase.requiresRouterToken && !present("ROUTER_API_TOKEN")) {
    missing.push("ROUTER_API_TOKEN");
  }
  if (
    testCase.requiresMarketOrRouterToken &&
    !present("ROUTER_API_TOKEN") &&
    !present("MARKET_API_TOKEN")
  ) {
    missing.push("ROUTER_API_TOKEN|MARKET_API_TOKEN");
  }
  if (
    testCase.shareEnv &&
    !valueFor(testCase.shareEnv, testCase.shareFallbackEnv)
  ) {
    missing.push(
      testCase.shareFallbackEnv
        ? `${testCase.shareEnv}|${testCase.shareFallbackEnv}`
        : testCase.shareEnv,
    );
  }
  if (testCase.urlEnv && !valueFor(testCase.urlEnv, testCase.urlFallbackEnv)) {
    missing.push(
      testCase.urlFallbackEnv
        ? `${testCase.urlEnv}|${testCase.urlFallbackEnv}`
        : testCase.urlEnv,
    );
  }
  return missing;
}

const cases = (matrix.cases || []).map((testCase) => {
  const missing = missingFor(testCase);
  const liveFixture = fixtureEvidenceFor(testCase);
  const staticCoverage = testCase.staticCoverage || {};
  const blockerGroup = missing.length > 0 ? "missing-matrix-input" : "";
  return {
    id: testCase.id,
    app: testCase.app,
    source: testCase.source,
    providerType: (testCase.providerFamilies || []).join("|"),
    entryPath: testCase.entryPath,
    supportsStream: Boolean(testCase.supportsStream),
    adapterStatus: testCase.adapterStatus || "unknown",
    staticNativeFamilies: staticCoverage.nativeFamilies || [],
    staticExperimentalFamilies: staticCoverage.experimentalFamilies || [],
    staticPlannedFamilies: staticCoverage.plannedFamilies || [],
    staticRemainingFallbackFamilies:
      staticCoverage.remainingFallbackFamilies || [],
    status: missing.length === 0 ? "runnable" : "blocked",
    verificationState:
      missing.length > 0
        ? "blocked_inputs"
        : testCase.staticCoverage
          ? "contract_verified"
          : "live_required",
    failureClass: "",
    blockerGroup,
    liveBlockerGroup:
      blockerGroup ||
      (liveFixture.complete ? "" : "missing-live-fixture-evidence"),
    evidencePath: liveFixture.evidencePath,
    fixtureEvidenceComplete: liveFixture.complete,
    missingFixtureFields: liveFixture.missing,
    runnable: missing.length === 0,
    missing,
  };
});

function caseFamilies(field) {
  return unique(cases.flatMap((item) => item[field] || []));
}

const summary = {
  matrixPath,
  total: cases.length,
  runnable: cases.filter((item) => item.runnable).length,
  skipped: cases.filter((item) => !item.runnable).length,
  skeleton: cases.filter(
    (item) =>
      item.adapterStatus === "skeleton" || item.adapterStatus === "mixed",
  ).length,
  realRequired: cases.filter((item) => item.adapterStatus === "real_required")
    .length,
  contractVerified: cases.filter(
    (item) => item.verificationState === "contract_verified",
  ).length,
  liveVerified: cases.filter(
    (item) => item.verificationState === "live_verified",
  ).length,
  liveRequired: cases.filter(
    (item) => item.verificationState === "live_required",
  ).length,
  blockedInputs: cases.filter(
    (item) => item.verificationState === "blocked_inputs",
  ).length,
  fixtureEvidencePath,
  matrixInputComplete: cases.every((item) => item.runnable),
  fixtureEvidenceComplete: cases.every((item) => item.fixtureEvidenceComplete),
  fixtureEvidenceMissing: cases.filter((item) => !item.fixtureEvidenceComplete)
    .length,
  blockerGroups: unique(cases.map((item) => item.blockerGroup)),
  liveBlockerGroups: unique(cases.map((item) => item.liveBlockerGroup)),
  staticNativeFamilies: caseFamilies("staticNativeFamilies"),
  staticExperimentalFamilies: caseFamilies("staticExperimentalFamilies"),
  staticPlannedFamilies: caseFamilies("staticPlannedFamilies"),
  staticRemainingFallbackFamilies: caseFamilies(
    "staticRemainingFallbackFamilies",
  ),
  cases,
  requiredFixtureFields,
};

console.log(JSON.stringify(summary, null, 2));
