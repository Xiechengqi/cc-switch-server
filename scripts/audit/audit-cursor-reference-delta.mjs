#!/usr/bin/env node

import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const contractPath = path.join(repoRoot, "assets/contract/cursor-reference-delta.json");
const registryPath = path.join(repoRoot, "assets/contract/provider-registry.json");
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
  contract.format === "cc-switch-cursor-reference-delta" && contract.schemaVersion === 1,
  "Cursor reference delta format changed",
);
assert(
  contract.policy?.externalSources === "read_only_optional_audit_input_never_runtime_dependency",
  "Cursor external-source boundary changed",
);
for (const invariant of [
  "no pool",
  "no rotation",
  "no rail fallback",
  "no cross-account fallback",
  "no cross-Provider fallback",
]) {
  assert(contract.policy?.scope?.includes(invariant), `Cursor scope lost ${invariant}`);
}
assert(
  contract.policy?.liveEvidence?.includes("separate real receipts") &&
    contract.policy.liveEvidence.includes("never upgrades either rail"),
  "Cursor fixture/live evidence boundary changed",
);

for (const source of contract.sources ?? []) {
  assert(source.id && source.rootEnv, "Cursor source metadata is incomplete");
  assert(/^[a-f0-9]{40}$/.test(source.commit), `${source.id} has an invalid commit`);
  assert(
    source.previousReviewedCommit === "a3ca33fa6442b59adc42976c795709eaf5351109",
    `${source.id} previous review point changed`,
  );
  assert(Array.isArray(source.files) && source.files.length >= 2, `${source.id} lacks evidence files`);
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

const incrementalReview = contract.incrementalReview;
assert(incrementalReview?.id === "CUR-N2", "Cursor incremental review id changed");
assert(
  incrementalReview.fromCommit === "a3ca33fa6442b59adc42976c795709eaf5351109" &&
    incrementalReview.toCommit === "02c663cdd0e8577bdcf2b01a44046bcd46dc6a7a",
  "Cursor incremental review range changed",
);
assert(
  incrementalReview.reviewedCommittedObjects === 6 &&
    incrementalReview.excludedWorktreeEntries === 22 &&
    incrementalReview.wireDelta === "none" &&
    incrementalReview.productionCodeChanged === false,
  "Cursor no-wire-delta conclusion changed",
);
if (checkSources) {
  const source = contract.sources.find((candidate) => candidate.id === "omniroute");
  assert(source, "Cursor OmniRoute source is missing");
  const sourceRoot = path.resolve(
    repoRoot,
    process.env[source.rootEnv] || source.defaultRelativeRoot,
  );
  const changedReviewedPaths = execFileSync(
    "git",
    [
      "-C",
      sourceRoot,
      "diff",
      "--name-only",
      `${incrementalReview.fromCommit}..${incrementalReview.toCommit}`,
      "--",
      ...source.files.map((file) => file.path),
    ],
    { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 },
  ).trim();
  assert(changedReviewedPaths === "", "Cursor reviewed wire objects changed in the frozen range");
}

const enhancements = new Map(
  (contract.enhancements ?? []).map((enhancement) => [enhancement.id, enhancement]),
);
assert(enhancements.size === 2, "Cursor enhancement set changed");
assert(
  enhancements.get("CUR-N1")?.status === "live_pending" &&
    JSON.stringify(enhancements.get("CUR-N1")?.rails) === JSON.stringify(["oauth", "api_key"]) &&
    enhancements.get("CUR-N1")?.receiptsIndependent === true,
  "CUR-N1 dual-rail live gate changed",
);
assert(
  enhancements.get("CUR-N2")?.status === "reviewed_no_wire_delta" &&
    enhancements.get("CUR-N2")?.productionCodeChanged === false,
  "CUR-N2 no-wire-delta conclusion changed",
);

const capabilities = new Map(
  (contract.capabilities ?? []).map((capability) => [capability.id, capability]),
);
assert(capabilities.size === 3, "Cursor contract must contain CUR-01 through CUR-03");
for (let index = 1; index <= 3; index += 1) {
  const id = `CUR-0${index}`;
  const capability = capabilities.get(id);
  assert(capability, `Cursor contract is missing ${id}`);
  safeRelative(capability.path, `${id} local path`);
  const localPath = path.join(repoRoot, capability.path);
  assert(fs.existsSync(localPath), `${id} local path is unavailable`);
  const source = fs.readFileSync(localPath, "utf8");
  for (const anchor of capability.anchors ?? []) {
    assert(source.includes(anchor), `${id} is missing local anchor ${anchor}`);
  }
}
for (const id of ["CUR-01", "CUR-03"]) {
  assert(capabilities.get(id)?.status === "fixture_verified", `${id} must remain fixture_verified`);
}
assert(capabilities.get("CUR-02")?.status === "live_pending", "CUR-02 must remain live_pending");

const registry = JSON.parse(fs.readFileSync(registryPath, "utf8"));
const drivers = Array.isArray(registry) ? registry : registry.drivers;
const driver = drivers?.find((candidate) => candidate.driverId === contract.registryTruth.driverId);
assert(driver, "special.cursor is absent from the provider registry");
assert(
  driver.driverContractRevision === contract.registryTruth.driverContractRevision,
  "Cursor driver contract revision drifted",
);
for (const [operation, expected] of Object.entries(contract.registryTruth.operations)) {
  assert(driver.operations?.[operation] === expected, `Cursor ${operation} operation drifted`);
}
const conformance = registry.conformance?.find(
  (candidate) => candidate.driverId === contract.registryTruth.driverId,
);
assert(conformance, "special.cursor conformance is absent from the provider registry");
for (const [operation, expected] of Object.entries(contract.registryTruth.conformance)) {
  assert(conformance[operation] === expected, `Cursor ${operation} conformance drifted`);
}

const acceptance = contract.realAcceptance;
assert(acceptance?.receiptSchemaVersion === 1, "Cursor receipt schema version changed");
assert(
  Array.isArray(acceptance.requiredChecks) &&
    acceptance.requiredChecks.length === 16 &&
    new Set(acceptance.requiredChecks).size === 16,
  "Cursor acceptance must retain exactly 16 unique checks",
);
const expectedChecks = [
  "fresh_catalog",
  "authoritative_empty_catalog",
  "transient_stale_catalog",
  "full_fast_model_id",
  "claude_stream",
  "claude_non_stream",
  "codex_stream",
  "codex_non_stream",
  "gemini_stream",
  "gemini_non_stream",
  "declared_tool",
  "image_input",
  "park_resume",
  "same_identity_401",
  "absolute_deadline",
  "identity_generation_drift",
];
assert(
  JSON.stringify(acceptance.requiredChecks) === JSON.stringify(expectedChecks),
  "Cursor acceptance check set or order changed",
);
assert(Array.isArray(acceptance.rails) && acceptance.rails.length === 2, "Cursor needs two rails");
const rails = new Map(acceptance.rails.map((rail) => [rail.rail, rail]));
assert(
  rails.get("oauth")?.credentialOwner === "managed_account" &&
    rails.get("api_key")?.credentialOwner === "provider_static_secret",
  "Cursor OAuth/API-key credential ownership changed",
);
for (const rail of rails.values()) {
  assert(rail.status === "live_pending" && rail.receipt === null, `${rail.rail} improperly claims live evidence`);
}

const fixtures = contract.protobufFixtures;
for (const name of [
  "serverConfigUnknownFields",
  "serverConfigDuplicate",
  "interactionUnknownBeforeKnown",
  "interactionUnknownOnly",
  "connectFrame",
]) {
  const value = fixtures?.[name]?.hex;
  assert(typeof value === "string" && value.length > 0 && value.length % 2 === 0, `${name} hex is invalid`);
  assert(/^[a-f0-9]+$/.test(value), `${name} hex is not lowercase hexadecimal`);
  Buffer.from(value, "hex");
}
assert(
  fixtures.connectFrame.hex.slice(10) === fixtures.connectFrame.expectedPayloadHex,
  "Cursor Connect fixture payload drifted",
);
assert(
  fixtures.model?.freshCatalogId === fixtures.model?.expectedWireId &&
    fixtures.model?.staleCatalogMayAuthorizeWireId === false,
  "Cursor exact fresh-model authorization boundary changed",
);
assert(
  fixtures.serverConfigDuplicate?.expected === "fail_closed" &&
    fixtures.connectFrame?.partialEof === "fail_closed" &&
    fixtures.terminal?.plainEofWithoutEnvelope === "fail_closed",
  "Cursor fail-closed protobuf/EOF boundary changed",
);

console.log(
  `cursor reference delta audit ok (${capabilities.size} capabilities, ${acceptance.requiredChecks.length} acceptance checks${
    checkSources ? ", external objects verified" : ", external check optional"
  })`,
);
