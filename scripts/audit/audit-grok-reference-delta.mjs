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
    Array.isArray(source.historicalCommits) && source.historicalCommits.length === 8,
    `${source.id} historical delta set changed`,
  );
  assert(Array.isArray(source.files) && source.files.length === 7, `${source.id} evidence files changed`);
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

const fixtureChecks = contract.fixtureAcceptance?.checks ?? [];
assert(fixtureChecks.length === 13 && new Set(fixtureChecks).size === 13, "Grok fixture matrix changed");
assert(
  JSON.stringify(contract.fixtureAcceptance?.transportRails) ===
    JSON.stringify(["http", "sse", "websocket"]),
  "Grok transport fixture matrix changed",
);
const acceptance = contract.realAcceptance;
assert(acceptance?.receiptSchemaVersion === 1, "Grok receipt schema version changed");
assert(
  acceptance.requiredChecks?.length === 15 && new Set(acceptance.requiredChecks).size === 15,
  "Grok real acceptance matrix changed",
);
const receipts = new Map((acceptance.receipts ?? []).map((receipt) => [receipt.capability, receipt]));
assert(receipts.size === 3, "Grok inference/media/compaction receipts must remain separate");
for (const capability of ["inference", "media", "remote_compaction"]) {
  const receipt = receipts.get(capability);
  assert(
    receipt?.rail === "oauth" && receipt.status === "live_pending" && receipt.receipt === null,
    `${capability} improperly claims live evidence`,
  );
}
assert(
  receipts.get("remote_compaction")?.runtimeEnabled === false &&
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
  `grok reference delta audit ok (${capabilities.size} capabilities, ${fixtureChecks.length} fixture checks, ${acceptance.requiredChecks.length} real checks${
    checkSources ? ", external objects verified" : ", external check optional"
  })`,
);
