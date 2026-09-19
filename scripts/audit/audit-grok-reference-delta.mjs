#!/usr/bin/env node

import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const contractPath = path.join(repoRoot, "assets/contract/grok-reference-delta.json");
const checkSources = process.argv.includes("--check-sources");

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function safeRelative(value, label) {
  assert(
    typeof value === "string" &&
      value.length > 0 &&
      !path.isAbsolute(value) &&
      !value.split("/").includes(".."),
    `${label} is not a safe relative path`,
  );
}

const contract = JSON.parse(fs.readFileSync(contractPath, "utf8"));
assert(
  contract.format === "cc-switch-grok-reference-delta" && contract.schemaVersion === 1,
  "Grok reference delta format changed",
);
assert(
  contract.policy?.externalSources === "read_only_optional_audit_input_never_runtime_dependency",
  "Grok external-source boundary changed",
);
for (const invariant of [
  "no pool",
  "no rotation",
  "no credential-rail fallback",
  "no cross-account fallback",
  "no cross-Provider fallback",
]) {
  assert(contract.policy?.scope?.includes(invariant), `Grok scope lost ${invariant}`);
}
assert(
  contract.policy?.recoveryBoundary?.includes("same-account") &&
    contract.policy.recoveryBoundary.includes("same-rail") &&
    contract.policy.recoveryBoundary.includes("pre-commit") &&
    contract.policy.recoveryBoundary.includes("shared total attempt"),
  "Grok recovery boundary changed",
);

for (const source of contract.sources ?? []) {
  assert(source.id && source.rootEnv, "Grok source metadata is incomplete");
  assert(/^[a-f0-9]{40}$/.test(source.commit), `${source.id} has an invalid commit`);
  assert(
    source.qualityPolicyCommit === "7f3f3d3ce030d5cf946b0bbe994f3026775fa308",
    `${source.id} quality-policy commit changed without review`,
  );
  assert(
    Array.isArray(source.historicalCommits) && source.historicalCommits.length === 8,
    `${source.id} historical delta set changed`,
  );
  assert(Array.isArray(source.files) && source.files.length === 10, `${source.id} evidence files changed`);
  const sourceRoot = path.resolve(
    repoRoot,
    process.env[source.rootEnv] || source.defaultRelativeRoot,
  );
  for (const file of source.files) {
    safeRelative(file.path, `${source.id} evidence path`);
    assert(/^[a-f0-9]{64}$/.test(file.sha256), `${source.id}:${file.path} has invalid SHA-256`);
    if (checkSources) {
      assert(fs.existsSync(sourceRoot), `${source.id} source root is unavailable`);
      const content = execFileSync(
        "git",
        ["-C", sourceRoot, "show", `${source.commit}:${file.path}`],
        { encoding: null, maxBuffer: 64 * 1024 * 1024 },
      );
      assert(
        sha256(content) === file.sha256,
        `${source.id}:${file.path} drifted from the reviewed Git object`,
      );
    }
  }
}

const capabilities = new Map(
  (contract.capabilities ?? []).map((capability) => [capability.id, capability]),
);
assert(capabilities.size === 5, "Grok contract must contain GR-01 through GR-05");
for (let index = 1; index <= 5; index += 1) {
  const id = `GR-0${index}`;
  const capability = capabilities.get(id);
  assert(capability, `Grok contract is missing ${id}`);
  safeRelative(capability.path, `${id} local path`);
  const localPath = path.join(repoRoot, capability.path);
  assert(fs.existsSync(localPath), `${id} local path is unavailable`);
  const source = fs.readFileSync(localPath, "utf8");
  for (const anchor of capability.anchors ?? []) {
    assert(source.includes(anchor), `${id} is missing local anchor ${anchor}`);
  }
}
for (const id of ["GR-01", "GR-02", "GR-03"]) {
  assert(capabilities.get(id)?.status === "fixture_verified", `${id} must be fixture_verified`);
}
for (const id of ["GR-04", "GR-05"]) {
  assert(capabilities.get(id)?.status === "live_pending", `${id} must remain live_pending`);
}
assert(capabilities.get("GR-05")?.runtimeEnabled === false, "GR-05 runtime gate opened");

const enhancements = new Map(
  (contract.enhancements ?? []).map((enhancement) => [enhancement.id, enhancement]),
);
assert(
  enhancements.size === 3 &&
    enhancements.has("GR-N1") &&
    enhancements.has("CORE-N1") &&
    enhancements.has("LIVE-N1"),
  "Grok enhancement set changed",
);
const qualityObservation = enhancements.get("GR-N1");
assert(qualityObservation.status === "fixture_verified", "GR-N1 fixture status changed");
for (const field of ["automaticRetry", "accountRotation", "responseMutation"]) {
  assert(qualityObservation[field] === false, `GR-N1 unexpectedly enabled ${field}`);
}
assert(
  JSON.stringify(qualityObservation.metricLabels) ===
    JSON.stringify([
      "transport",
      "outcome",
      "has_terminal",
      "has_tool",
      "has_visible_text",
      "visible_length_bucket",
    ]),
  "GR-N1 metric label set changed",
);
assert(
  Array.isArray(qualityObservation.localEvidence) && qualityObservation.localEvidence.length === 3,
  "GR-N1 local evidence set changed",
);
for (const evidence of qualityObservation.localEvidence) {
  safeRelative(evidence.path, "GR-N1 local path");
  const localPath = path.join(repoRoot, evidence.path);
  assert(fs.existsSync(localPath), `GR-N1 local path is unavailable: ${evidence.path}`);
  const source = fs.readFileSync(localPath, "utf8");
  for (const anchor of evidence.anchors ?? []) {
    assert(source.includes(anchor), `GR-N1 is missing local anchor ${anchor}`);
  }
}
for (const id of ["CORE-N1", "LIVE-N1"]) {
  const enhancement = enhancements.get(id);
  assert(
    enhancement.status === (id === "CORE-N1" ? "fixture_verified" : "live_pending"),
    `${id} status changed`,
  );
  assert(
    Array.isArray(enhancement.localEvidence) &&
      enhancement.localEvidence.length === (id === "CORE-N1" ? 2 : 3),
    `${id} local evidence set changed`,
  );
  for (const evidence of enhancement.localEvidence) {
    safeRelative(evidence.path, `${id} local path`);
    const localPath = path.join(repoRoot, evidence.path);
    assert(fs.existsSync(localPath), `${id} local path is unavailable: ${evidence.path}`);
    const source = fs.readFileSync(localPath, "utf8");
    for (const anchor of evidence.anchors ?? []) {
      assert(source.includes(anchor), `${id} is missing local anchor ${anchor}`);
    }
  }
}
assert(
  enhancements.get("CORE-N1").wireChanged === false &&
    enhancements.get("CORE-N1").bindingChanged === false &&
    enhancements.get("CORE-N1").recoveryBudgetChanged === false,
  "CORE-N1 changed Grok wire, binding, or recovery budget",
);
assert(
  enhancements.get("LIVE-N1").operationScoped === true &&
    enhancements.get("LIVE-N1").receiptsIndependent === true &&
    enhancements.get("LIVE-N1").runtimeAutoEnable === false,
  "LIVE-N1 receipt isolation changed",
);

const fixtureChecks = contract.fixtureAcceptance?.checks ?? [];
assert(fixtureChecks.length === 14 && new Set(fixtureChecks).size === 14, "Grok fixture matrix changed");
assert(
  JSON.stringify(contract.fixtureAcceptance?.transportRails) ===
    JSON.stringify(["http", "sse", "websocket"]),
  "Grok transport fixture matrix changed",
);
const acceptance = contract.realAcceptance;
assert(acceptance?.receiptSchemaVersion === 1, "Grok receipt schema version changed");
assert(
  acceptance.harnessRevision === 1,
  "Grok receipt harness revision changed",
);
const operations = new Map(
  (acceptance.operations ?? []).map((entry) => [entry.operation, entry]),
);
assert(operations.size === 3, "Grok inference/media/compaction receipts must remain separate");
const operationShapes = {
  inference: [21, 10, 5],
  media: [17, 6, 5],
  remote_compaction: [15, 5, 5],
};
for (const [operation, [checkCount, hashCount, measurementCount]] of Object.entries(
  operationShapes,
)) {
  const receipt = operations.get(operation);
  assert(
    receipt?.rail === "oauth" && receipt.status === "live_pending" && receipt.receipt === null,
    `${operation} improperly claims live evidence`,
  );
  assert(
    receipt.requiredChecks?.length === checkCount &&
      new Set(receipt.requiredChecks).size === checkCount &&
      receipt.requiredBodyHashes?.length === hashCount &&
      new Set(receipt.requiredBodyHashes).size === hashCount &&
      receipt.requiredMeasurements?.length === measurementCount &&
      new Set(receipt.requiredMeasurements).size === measurementCount,
    `${operation} acceptance matrix changed`,
  );
}
assert(
  operations.get("remote_compaction")?.runtimeEnabled === false &&
    contract.remoteCompactionGate?.runtimeEnabled === false &&
    contract.remoteCompactionGate?.requiresLiveOAuthReceipt === true,
  "Grok remote compaction gate opened without evidence",
);
assert(
  JSON.stringify(contract.remoteCompactionGate?.forbiddenInputs) ===
    JSON.stringify(["grok_web_cookie", "cross_account_cache", "sub2api_pool_or_routing"]),
  "Grok remote compaction forbidden inputs changed",
);

console.log(
  `grok reference delta audit ok (${capabilities.size} capabilities, ${enhancements.size} enhancements, ${fixtureChecks.length} fixture checks, ${[...operations.values()].reduce((total, entry) => total + entry.requiredChecks.length, 0)} operation checks${
    checkSources ? ", external objects verified" : ", external check optional"
  })`,
);
