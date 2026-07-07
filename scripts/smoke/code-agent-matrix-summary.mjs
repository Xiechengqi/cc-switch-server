#!/usr/bin/env node
import fs from "node:fs";

const matrixPath = process.argv[2] || "docs/code-agent-regression-matrix.json";
const matrix = JSON.parse(fs.readFileSync(matrixPath, "utf8"));

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

function missingFor(testCase) {
  const missing = [];
  if (testCase.requiresServerToken && !present("CC_SWITCH_SERVER_TOKEN")) {
    missing.push("CC_SWITCH_SERVER_TOKEN");
  }
  if (testCase.requiresRouterToken && !present("ROUTER_API_TOKEN")) {
    missing.push("ROUTER_API_TOKEN");
  }
  if (testCase.requiresMarketOrRouterToken && !present("ROUTER_API_TOKEN") && !present("MARKET_API_TOKEN")) {
    missing.push("ROUTER_API_TOKEN|MARKET_API_TOKEN");
  }
  if (testCase.shareEnv && !valueFor(testCase.shareEnv, testCase.shareFallbackEnv)) {
    missing.push(testCase.shareFallbackEnv ? `${testCase.shareEnv}|${testCase.shareFallbackEnv}` : testCase.shareEnv);
  }
  if (testCase.urlEnv && !valueFor(testCase.urlEnv, testCase.urlFallbackEnv)) {
    missing.push(testCase.urlFallbackEnv ? `${testCase.urlEnv}|${testCase.urlFallbackEnv}` : testCase.urlEnv);
  }
  return missing;
}

const cases = (matrix.cases || []).map((testCase) => {
  const missing = missingFor(testCase);
  const staticCoverage = testCase.staticCoverage || {};
  let blockerGroup = "";
  if (missing.some((item) => item.includes("ROUTER_API_TOKEN"))) blockerGroup = "missing-router-token";
  else if (missing.some((item) => item.includes("MARKET_API_TOKEN"))) blockerGroup = "missing-market-auth";
  else if (missing.length > 0) blockerGroup = "missing-env";
  else if (testCase.adapterStatus === "real_required" || testCase.adapterStatus === "mixed") {
    blockerGroup = "missing-provider-token";
  }
  return {
    id: testCase.id,
    app: testCase.app,
    source: testCase.source,
    providerType: (testCase.providerFamilies || []).join("|"),
    entryPath: testCase.entryPath,
    supportsStream: Boolean(testCase.supportsStream),
    adapterStatus: testCase.adapterStatus || "unknown",
    staticNativeFamilies: staticCoverage.nativeFamilies || [],
    staticPlannedFamilies: staticCoverage.plannedFamilies || [],
    staticRemainingFallbackFamilies: staticCoverage.remainingFallbackFamilies || [],
    status: missing.length === 0 ? "runnable" : "blocked",
    failureClass: "",
    blockerGroup,
    evidencePath: "",
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
  skeleton: cases.filter((item) => item.adapterStatus === "skeleton" || item.adapterStatus === "mixed").length,
  realRequired: cases.filter((item) => item.adapterStatus === "real_required").length,
  staticNativeFamilies: caseFamilies("staticNativeFamilies"),
  staticPlannedFamilies: caseFamilies("staticPlannedFamilies"),
  staticRemainingFallbackFamilies: caseFamilies("staticRemainingFallbackFamilies"),
  cases,
  requiredFixtureFields: matrix.requiredFixtureFields || [],
};

console.log(JSON.stringify(summary, null, 2));
