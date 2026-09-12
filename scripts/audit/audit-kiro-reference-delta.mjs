#!/usr/bin/env node

import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const contractPath = path.join(repoRoot, "assets/contract/kiro-reference-delta.json");
const wirePath = path.join(repoRoot, "assets/contract/kiro-wire-protocol.json");
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
  contract.format === "cc-switch-kiro-reference-delta" && contract.schemaVersion === 1,
  "Kiro reference delta format changed",
);
assert(
  contract.policy?.externalSources === "read_only_optional_audit_input_never_runtime_dependency",
  "Kiro external-source boundary changed",
);
for (const invariant of [
  "no pool",
  "no rotation",
  "no credential-rail fallback",
  "no cross-account fallback",
  "no cross-Provider fallback",
]) {
  assert(contract.policy?.scope?.includes(invariant), `Kiro scope lost ${invariant}`);
}
assert(
  contract.policy?.cacheBoundary?.includes("local downstream metering fallback only") &&
    contract.policy.cacheBoundary.includes("never changes upstream request") &&
    contract.policy.cacheBoundary.includes("account binding"),
  "Kiro cache/account boundary changed",
);

for (const source of contract.sources ?? []) {
  assert(source.id && source.rootEnv, "Kiro source metadata is incomplete");
  assert(/^[a-f0-9]{40}$/.test(source.commit), `${source.id} has an invalid commit`);
  assert(
    JSON.stringify(source.historicalCommits) ===
      JSON.stringify(["f2cc574", "19b7f4b", "47633a4"]),
    `${source.id} historical delta set changed`,
  );
  assert(source.files?.length === 2, `${source.id} evidence file set changed`);
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
assert(capabilities.size === 5, "Kiro contract must contain KI-01 through KI-05");
for (let index = 1; index <= 5; index += 1) {
  const id = `KI-0${index}`;
  const capability = capabilities.get(id);
  assert(capability, `Kiro contract is missing ${id}`);
  safeRelative(capability.path, `${id} local path`);
  const localPath = path.join(repoRoot, capability.path);
  assert(fs.existsSync(localPath), `${id} local path is unavailable`);
  const local = fs.readFileSync(localPath, "utf8");
  for (const anchor of capability.anchors ?? []) {
    assert(local.includes(anchor), `${id} is missing local anchor ${anchor}`);
  }
}
for (const id of ["KI-01", "KI-02", "KI-03"]) {
  assert(capabilities.get(id)?.status === "fixture_verified", `${id} must be fixture_verified`);
}
for (const id of ["KI-04", "KI-05"]) {
  assert(capabilities.get(id)?.status === "live_pending", `${id} must remain live_pending`);
}
assert(capabilities.get("KI-05")?.runtimeEnabled === false, "KI-05 runtime gate opened");

const fixtureChecks = contract.fixtureAcceptance?.checks ?? [];
assert(
  fixtureChecks.length === 16 && new Set(fixtureChecks).size === 16,
  "Kiro fixture acceptance matrix changed",
);
const acceptance = contract.realAcceptance;
assert(acceptance?.receiptSchemaVersion === 1, "Kiro receipt schema version changed");
assert(
  acceptance.requiredChecks?.length === 10 && new Set(acceptance.requiredChecks).size === 10,
  "Kiro real acceptance checks changed",
);
const receipts = acceptance.receipts ?? [];
assert(receipts.length === 8, "Kiro acceptance must remain split by four auth kinds and two regions");
const expectedAuthKinds = ["api_key", "builder_id", "idc", "social"];
const expectedRegions = ["eu-central-1", "us-east-1"];
assert(
  JSON.stringify([...new Set(receipts.map((receipt) => receipt.authKind))].sort()) ===
    JSON.stringify(expectedAuthKinds),
  "Kiro auth-kind receipt split changed",
);
assert(
  JSON.stringify([...new Set(receipts.map((receipt) => receipt.region))].sort()) ===
    JSON.stringify(expectedRegions),
  "Kiro region receipt split changed",
);
for (const receipt of receipts) {
  assert(
    receipt.status === "live_pending" && receipt.receipt === null,
    `${receipt.authKind}/${receipt.region} improperly claims live evidence`,
  );
}

const shared = contract.sharedCacheGate;
assert(
  shared?.runtimeEnabled === false &&
    shared.requiresExplicitMultiReplicaNeed === true &&
    shared.requiresLiveEvidence === true,
  "Kiro shared-cache gate opened without deployment need and evidence",
);
assert(
  JSON.stringify(shared.forbiddenInputs) ===
    JSON.stringify([
      "account_affinity",
      "account_routing",
      "cache_hit_account_selection",
      "cross_account_cache",
      "reference_redis_runtime_dependency",
    ]),
  "Kiro shared-cache forbidden inputs changed",
);

const wire = JSON.parse(fs.readFileSync(wirePath, "utf8"));
assert(wire.schemaVersion === 3, "Kiro wire protocol schema version changed");
const promptCache = wire.promptCache;
assert(
  promptCache?.mode === "local_metering_fallback_only" &&
    promptCache.changesUpstreamInferenceCost === false,
  "Kiro local metering boundary changed",
);
assert(
  JSON.stringify(promptCache.namespaceComponents) ===
    JSON.stringify([
      "app",
      "provider_id",
      "provider_revision",
      "runtime_fingerprint",
      "account_id",
      "auth_identity_generation",
      "token_refresh_generation",
      "share_id",
      "signed_user",
      "route",
      "runtime_region",
      "session",
      "model_and_inference_options",
    ]),
  "Kiro prompt-cache namespace changed",
);
assert(
  promptCache.topLevelAutoCaching === true &&
    promptCache.explicitBlockBreakpoints === true &&
    promptCache.unifiedBreakpointLimit === 4 &&
    promptCache.lookbackPositions === 20 &&
    promptCache.mixedTtlOrder === "1h_before_5m",
  "Kiro prompt-cache breakpoint semantics changed",
);
assert(
  promptCache.usageMapping?.authoritativeUpstreamTokenUsageWinsIncludingExplicitZero === true &&
    promptCache.usageMapping.conservation.includes("equals_authoritative_total"),
  "Kiro authoritative usage/conservation contract changed",
);
assert(
  promptCache.persistence?.format === "sqlite" &&
    promptCache.persistence.requestPathBlockingWrites === false &&
    promptCache.persistence.generationCas === true &&
    promptCache.persistence.shutdownFlush === true &&
    promptCache.persistence.remoteStore === "disabled",
  "Kiro persistence or remote-store boundary changed",
);

console.log(
  `kiro reference delta audit ok (${capabilities.size} capabilities, ${fixtureChecks.length} fixture checks, ${receipts.length} pending receipts${
    checkSources ? ", external objects verified" : ", external check optional"
  })`,
);
