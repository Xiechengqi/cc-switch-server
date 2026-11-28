import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { matchingOperationEvidence, validateOperationEvidence } from "./provider-operation-evidence.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const read = (relative) => JSON.parse(fs.readFileSync(path.join(root, relative), "utf8"));
const evidence = read("assets/contract/provider-conformance-evidence.json");
const registry = read("assets/contract/provider-registry.json");
const fresh = () => structuredClone(evidence.operationEvidence);
const validate = (view) => validateOperationEvidence(view, registry, root);
const sha = (relative) => crypto.createHash("sha256").update(fs.readFileSync(path.join(root, relative))).digest("hex");

// A shape fixture for the read-only index, never a real receipt or a persisted
// live result. Real private validation remains in the existing live harness.
function receiptShapeFixture() {
  const profile = registry.profiles.find((profile) => profile.app === "codex" && profile.driverBinding?.driverId === "oauth.openai_codex");
  const contract = read("assets/contract/codex-reference-delta.json");
  const operation = [
    ...contract.realAcceptance.operations,
    ...(contract.realAcceptanceExtensions ?? []).flatMap((extension) => extension.operations ?? []),
  ].find((operation) => operation.operation === "gpt_6_astra");
  const scope = {
    driverId: "oauth.openai_codex", driverContractRevision: registry.drivers.find((driver) => driver.driverId === "oauth.openai_codex").driverContractRevision, providerType: profile.compatibilityProviderType,
    profileId: profile.profileId, profileSchemaRevision: profile.profileSchemaRevision, rail: "chatgpt-codex-oauth", transport: "https-sse",
    model: operation.model, surface: "responses", stream: true,
    clientVersion: "codex-cli/0.169.0", feature: operation.operation,
    sourceCommit: "b".repeat(40), sourceDigest: "c".repeat(64),
    binarySha256: "a".repeat(64), bindingScopeDigest: "d".repeat(64),
  };
  const harness = "scripts/smoke/codex-real-receipt.mjs";
  return {
    id: "synthetic-index-shape-only", operation: "forward", scope,
    recordedOn: "2026-11-27", generationRequests: 1,
    verificationLevel: "live_verified", liveState: "live_verified", outcome: "success",
    source: { kind: "validated_receipt_summary", path: harness, sha256: sha(harness), anchor: operation.operation },
    receipt: {
      verificationState: "live_verified", liveState: "live_verified",
      receiptSchemaVersion: contract.realAcceptance.receiptSchemaVersion,
      harnessRevision: contract.realAcceptance.harnessRevision,
      receiptSha256: "e".repeat(64), scopeDigest: "f".repeat(64),
      recordedOn: "2026-11-27", operation: operation.operation,
      scope: structuredClone(scope), generationRequests: 1, outcome: "success",
    },
  };
}

test("operation view is optional and the existing coarse driver truth is unchanged", () => {
  assert.deepEqual(validate(undefined), { entries: 0, historical: 0, fixture: 0, live: 0 });
  const coarse = JSON.stringify(evidence.drivers);
  const profiles = JSON.stringify(registry);
  const statistics = validate(fresh());
  assert.equal(statistics.historical, 9);
  assert.equal(statistics.live, 0);
  assert(statistics.fixture >= 5, "the original offline corpus is retained");
  assert.equal(statistics.entries, statistics.historical + statistics.fixture);
  assert.equal(JSON.stringify(evidence.drivers), coarse);
  assert.equal(JSON.stringify(registry), profiles);
  assert(evidence.drivers.every((driver) => Object.values(driver.operations).every((operation) => operation.liveState !== "live_verified")));
});

test("historical successes keep missing inputs unknown and cannot become receipts", () => {
  const view = fresh();
  const historical = view.entries.find((entry) => entry.id === "zen-20261120-codex-cli-reported");
  assert.equal(historical.generationRequests, 10);
  for (const field of ["model", "stream", "sourceCommit", "sourceDigest", "binarySha256", "bindingScopeDigest"]) assert.equal(historical.scope[field], null);
  historical.verificationLevel = "live_verified";
  historical.liveState = "live_verified";
  assert.throws(() => validate(view), /receipt summary/);
});

test("fixture corpus distinguishes rejected, truncated, duplicate-terminal and cancelled outcomes", () => {
  const view = fresh();
  for (const outcome of ["expected_failure", "truncated", "duplicate_terminal", "cancelled"]) {
    assert(view.entries.some((entry) => entry.outcome === outcome && entry.verificationLevel === "fixture_verified" && entry.liveState === "live_pending"));
  }
  for (const entry of view.entries.filter((entry) => entry.verificationLevel === "fixture_verified")) {
    const candidate = structuredClone(view);
    const changed = candidate.entries.find((row) => row.id === entry.id);
    changed.verificationLevel = "live_verified";
    changed.liveState = "live_verified";
    assert.throws(() => validate(candidate));
  }
});

test("index matches the complete scope without model, rail, Profile or version fallback", () => {
  const view = fresh();
  const original = view.entries.find((entry) => entry.id === "qoder-r6-high-to-max-fixture");
  assert.equal(matchingOperationEvidence(view, structuredClone(original.scope)).length, 1);
  for (const [key, value] of Object.entries({ model: "another-model", rail: "cn_oauth", transport: "another-wire", profileId: "claude.qoder_cosy", stream: true, clientVersion: "future-cli/9", sourceCommit: "a".repeat(40), sourceDigest: "b".repeat(64), binarySha256: "c".repeat(64), bindingScopeDigest: "d".repeat(64) })) {
    assert.equal(matchingOperationEvidence(view, { ...original.scope, [key]: value }).length, 0, key);
  }
  assert.throws(() => matchingOperationEvidence(view, { model: original.scope.model }), /scope fields/);
  assert.throws(() => matchingOperationEvidence(view, { ...original.scope, model: "*" }), /bounded exact identifier/);
});

test("a complete accepted-receipt summary can be represented without promoting the driver", () => {
  const view = fresh();
  view.entries.push(receiptShapeFixture());
  assert.equal(validate(view).live, 1);
  const driver = evidence.drivers.find((driver) => driver.driverId === "oauth.openai_codex");
  assert.equal(driver.operations.forward.verificationLevel, "fixture_verified");
  assert.equal(driver.operations.forward.liveState, "live_pending");
});

test("a receipt summary cannot move across model, rail, version, scope or build", () => {
  const changes = { model: "another-model", rail: "another-rail", profileId: "another.profile", profileSchemaRevision: 99999, driverContractRevision: 99999, transport: "another-wire", surface: "chat", stream: false, clientVersion: "codex-cli/9", feature: "another-feature", sourceCommit: "a".repeat(40), sourceDigest: "b".repeat(64), binarySha256: "c".repeat(64), bindingScopeDigest: "e".repeat(64) };
  for (const [key, value] of Object.entries(changes)) {
    const view = fresh();
    const entry = receiptShapeFixture();
    entry.receipt.scope[key] = value;
    view.entries.push(entry);
    assert.throws(() => validate(view), /receipt scope/, key);
  }
  for (const key of ["generationRequests", "recordedOn", "operation", "outcome"]) {
    const view = fresh();
    const entry = receiptShapeFixture();
    entry.receipt[key] = key === "generationRequests" ? 2 : key === "recordedOn" ? "2026-11-26" : key === "outcome" ? "rejected" : "another-operation";
    view.entries.push(entry);
    assert.throws(() => validate(view), /receipt scope/);
  }
});

test("fixture-mode receipt labels, incomplete scope and ungoverned validators fail closed", () => {
  for (const mutation of [
    (entry) => { entry.receipt.verificationState = "contract_verified"; entry.receipt.liveState = "live_pending"; },
    (entry) => { entry.scope.clientVersion = null; entry.receipt.scope.clientVersion = null; },
    (entry) => { entry.receipt.harnessRevision += 1; },
    (entry) => { entry.receipt.receiptSchemaVersion += 1; },
    (entry) => { entry.source.anchor = "another-feature"; },
    (entry) => { entry.source.path = "scripts/smoke/claude-real-receipt.mjs"; entry.source.sha256 = sha(entry.source.path); },
  ]) {
    const view = fresh();
    const entry = receiptShapeFixture();
    mutation(entry);
    view.entries.push(entry);
    assert.throws(() => validate(view));
  }
});

test("source drift, cross-Provider fixture attribution, private fields and changed policy are rejected", () => {
  for (const mutation of [
    (view) => { view.entries[0].source.sha256 = "0".repeat(64); },
    (view) => { view.entries[0].source.path = "../../outside"; },
    (view) => { view.entries[0].prompt = "private"; },
    (view) => { view.entries[0].scope.authorization = "private"; },
    (view) => { view.policy.grantsCapabilities = true; },
    (view) => { view.entries.push(structuredClone(view.entries[0])); },
    (view) => {
      const entry = view.entries.find((entry) => entry.id === "qoder-r6-high-to-max-fixture");
      entry.scope.driverId = "special.opencode_zen";
      entry.scope.driverContractRevision = registry.drivers.find((driver) => driver.driverId === "special.opencode_zen").driverContractRevision;
      entry.scope.providerType = "opencode_zen";
      entry.scope.profileId = "codex.opencode_zen";
      entry.scope.profileSchemaRevision = registry.profiles.find((profile) => profile.profileId === "codex.opencode_zen").profileSchemaRevision;
    },
  ]) {
    const view = fresh();
    mutation(view);
    assert.throws(() => validate(view));
  }
});

test("an indexed source cannot turn into an external checkout through a symlink", () => {
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "operation-evidence-"));
  try {
    const repo = path.join(temporary, "repo");
    fs.mkdirSync(path.join(repo, "docs/provider"), { recursive: true });
    const outside = path.join(temporary, "outside.md");
    fs.writeFileSync(outside, "fixture-only");
    fs.symlinkSync(outside, path.join(repo, "docs/provider/opencode-zen.md"));
    const view = fresh();
    view.entries = [view.entries[0]];
    assert.throws(() => validateOperationEvidence(view, registry, repo), /escapes the repository/);
  } finally {
    fs.rmSync(temporary, { recursive: true, force: true });
  }
});
