import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

// This is a read-only index inside the existing conformance framework. It
// never imports private receipts, runs their harnesses, or changes admission.
const scopeKeys = ["driverId", "driverContractRevision", "providerType", "profileId", "profileSchemaRevision", "rail", "transport", "model", "surface", "stream", "clientVersion", "feature", "sourceCommit", "sourceDigest", "binarySha256", "bindingScopeDigest"];
const entryKeys = ["id", "operation", "scope", "recordedOn", "generationRequests", "verificationLevel", "liveState", "outcome", "source", "receipt"];
const receiptHarnesses = {
  "oauth.claude_messages": ["scripts/smoke/claude-real-receipt.mjs", "claude-reference-delta.json"],
  "oauth.openai_codex": ["scripts/smoke/codex-real-receipt.mjs", "codex-reference-delta.json"],
  "oauth.grok_responses": ["scripts/smoke/grok-real-receipt.mjs", "grok-reference-delta.json"],
  "special.kiro": ["scripts/smoke/kiro-real-receipt.mjs", "kiro-reference-delta.json"],
  "special.codebuddy_oauth": ["scripts/smoke/codebuddy-real-receipt.mjs", "codebuddy-reference-delta.json"],
  "special.workbuddy_oauth": ["scripts/smoke/workbuddy-real-receipt.mjs", "codebuddy-reference-delta.json"],
  "special.qoder_cosy": ["scripts/smoke/qoder-real.mjs", "qoder-cli-oracle.json"],
  "special.antigravity": ["scripts/smoke/antigravity-real.mjs", "antigravity-reference-delta.json"],
  "special.agy": ["scripts/smoke/antigravity-real.mjs", "antigravity-reference-delta.json"],
  "special.cursor": ["scripts/smoke/cursor-real.mjs", "cursor-reference-delta.json"],
};
const assert = (condition, message) => {
  if (!condition) throw new Error(`operation evidence: ${message}`);
};
const plain = (value) => value && typeof value === "object" && !Array.isArray(value);
const exactKeys = (value, keys, label) => {
  assert(plain(value), `${label} must be an object`);
  assert(JSON.stringify(Object.keys(value).sort()) === JSON.stringify([...keys].sort()), `${label} fields changed`);
};
const short = (value, label) => {
  assert(typeof value === "string" && /^[A-Za-z0-9][A-Za-z0-9_.:/+-]{0,159}$/.test(value), `${label} must be a bounded exact identifier`);
};
const hash = (value, length, label) => assert(typeof value === "string" && new RegExp(`^[a-f0-9]{${length}}$`).test(value), `${label} digest is invalid`);
const canonical = (value) => {
  if (Array.isArray(value)) return value.map(canonical);
  if (plain(value)) return Object.fromEntries(Object.keys(value).sort().map((key) => [key, canonical(value[key])]));
  return value;
};
const same = (left, right) => JSON.stringify(canonical(left)) === JSON.stringify(canonical(right));
const date = (value) => {
  assert(typeof value === "string" && /^\d{4}-\d{2}-\d{2}$/.test(value) && new Date(`${value}T00:00:00Z`).toISOString().slice(0, 10) === value, "recorded date is invalid");
};

function validateScope(scope) {
  exactKeys(scope, scopeKeys, "scope");
  for (const key of ["driverId", "providerType", "profileId", "rail", "transport", "feature"]) short(scope[key], key);
  for (const key of ["model", "clientVersion"]) if (scope[key] !== null) short(scope[key], key);
  assert(["messages", "chat", "responses", "gemini", "management"].includes(scope.surface), "surface is invalid");
  assert(scope.stream === null || typeof scope.stream === "boolean", "stream is invalid");
  for (const key of ["driverContractRevision", "profileSchemaRevision"]) assert(scope[key] === null || (Number.isSafeInteger(scope[key]) && scope[key] > 0 && scope[key] <= 1000000), `${key} is invalid`);
  for (const [key, length] of [["sourceCommit", 40], ["sourceDigest", 64], ["binarySha256", 64], ["bindingScopeDigest", 64]]) {
    if (scope[key] !== null) hash(scope[key], length, key);
  }
}

function readSource(source, repoRoot, scope) {
  exactKeys(source, ["kind", "path", "sha256", "anchor"], "source");
  assert(["fixture", "historical_document", "validated_receipt_summary"].includes(source.kind), "source kind is invalid");
  assert(typeof source.path === "string" && source.path.length < 240 && !source.path.includes("\\") && !path.posix.isAbsolute(source.path) && path.posix.normalize(source.path) === source.path && !source.path.split("/").includes(".."), "source must be a repository path");
  const allowedPrefix = source.kind === "fixture" ? "assets/contract/" : source.kind === "historical_document" ? "docs/provider/" : "scripts/smoke/";
  assert(source.path.startsWith(allowedPrefix), "source path does not match its kind");
  const root = fs.realpathSync(repoRoot);
  const filename = fs.realpathSync(path.join(root, source.path));
  assert(filename.startsWith(`${root}${path.sep}`), "source escapes the repository");
  const stat = fs.statSync(filename);
  assert(stat.isFile() && stat.size <= 2 * 1024 * 1024, "source must be a bounded file");
  const bytes = fs.readFileSync(filename);
  hash(source.sha256, 64, "source");
  assert(crypto.createHash("sha256").update(bytes).digest("hex") === source.sha256, "source digest drifted");
  assert(typeof source.anchor === "string" && source.anchor.length > 0 && Buffer.byteLength(source.anchor) <= 240, "source anchor is invalid");
  if (source.kind === "fixture") {
    const artifact = JSON.parse(bytes.toString("utf8"));
    assert(artifact.implementationState === "local_complete" && artifact.liveState === "live_pending", "fixture artifact is not locally complete/pending");
    const groupedClass = { trae_solo: "trae", trae_raw_chat: "trae", trae_raw_chat_intl: "trae", codebuddy_oauth: "buddy", workbuddy_oauth: "buddy", grok_oauth: "grok" }[scope.providerType];
    assert(artifact.providerClass === scope.providerType || (groupedClass && artifact.providerClass === groupedClass), "fixture artifact belongs to another Provider class");
    assert(artifact.cases?.some((entry) => entry.id === source.anchor), "fixture case is not in its artifact");
  } else if (source.kind === "historical_document") {
    assert(bytes.toString("utf8").includes(source.anchor), "historical document anchor drifted");
  }
}

function validateReceiptSummary(entry, repoRoot) {
  const receipt = entry.receipt;
  exactKeys(receipt, ["verificationState", "liveState", "receiptSchemaVersion", "harnessRevision", "receiptSha256", "scopeDigest", "recordedOn", "operation", "scope", "generationRequests", "outcome"], "receipt summary");
  assert(entry.source.kind === "validated_receipt_summary", "live evidence must cite the existing receipt validator");
  const governed = receiptHarnesses[entry.scope.driverId];
  assert(governed && entry.source.path === governed[0], "receipt validator does not govern this driver");
  const contract = JSON.parse(fs.readFileSync(path.join(repoRoot, "assets/contract", governed[1]), "utf8"));
  const schema = contract.realAcceptance ?? contract.receiptSchema;
  const schemaVersion = schema?.receiptSchemaVersion ?? schema?.schemaVersion;
  assert(Number.isInteger(schemaVersion) && Number.isInteger(schema?.harnessRevision), "existing receipt contract has no supported version");
  assert(receipt.receiptSchemaVersion === schemaVersion && receipt.harnessRevision === schema.harnessRevision, "existing receipt schema/harness revision drifted");
  assert(receipt.verificationState === "live_verified" && receipt.liveState === "live_verified", "fixture receipt cannot become live evidence");
  hash(receipt.receiptSha256, 64, "receipt");
  hash(receipt.scopeDigest, 64, "private scope");
  assert(same(receipt.scope, entry.scope) && receipt.operation === entry.scope.feature && entry.source.anchor === entry.scope.feature && receipt.recordedOn === entry.recordedOn && receipt.generationRequests === entry.generationRequests && receipt.outcome === entry.outcome, "receipt scope does not exactly match the observation");
  if (Array.isArray(schema.operations)) {
    const operations = [...schema.operations, ...(contract.realAcceptanceExtensions ?? []).flatMap((extension) => extension.operations ?? [])];
    const operation = operations.find((operation) => operation.operation === entry.scope.feature);
    assert(operation && (!operation.model || operation.model === entry.scope.model), "receipt operation/model is outside its existing contract");
  }
  assert(entry.generationRequests !== null && Object.values(entry.scope).every((value) => value !== null), "live evidence requires a complete scope");
  // Private receipt integrity/freshness, identity generations and live results
  // remain the responsibility of the cited existing harness. The public index
  // stores its scoped attestation and never substitutes for running that gate.
}

export function validateOperationEvidence(view, registry, repoRoot) {
  if (view === undefined) return { entries: 0, historical: 0, fixture: 0, live: 0 };
  exactKeys(view, ["schemaVersion", "policy", "entries"], "view");
  assert(view.schemaVersion === 1, "view schema version changed");
  assert(same(view.policy, { readOnly: true, changesCoarseGates: false, grantsCapabilities: false, requiresExistingReceiptValidation: true }), "view must not become a gate or receipt validator");
  assert(Array.isArray(view.entries) && view.entries.length <= 512, "view must have at most 512 entries");
  const profiles = new Map(registry.profiles.map((profile) => [profile.profileId, profile]));
  const drivers = new Map(registry.drivers.map((driver) => [driver.driverId, driver]));
  const ids = new Set();
  const result = { entries: view.entries.length, historical: 0, fixture: 0, live: 0 };
  for (const entry of view.entries) {
    exactKeys(entry, entryKeys, "entry");
    short(entry.id, "id");
    assert(!ids.has(entry.id), "duplicate entry id");
    ids.add(entry.id);
    validateScope(entry.scope);
    date(entry.recordedOn);
    assert(entry.generationRequests === null || (Number.isSafeInteger(entry.generationRequests) && entry.generationRequests >= 0 && entry.generationRequests <= 1000000), "generation request count is invalid");
    assert(["success", "expected_failure", "rejected", "truncated", "duplicate_terminal", "cancelled", "capacity_shed"].includes(entry.outcome), "outcome is invalid");
    const profile = profiles.get(entry.scope.profileId);
    const driver = drivers.get(entry.scope.driverId);
    assert(profile?.driverBinding?.driverId === entry.scope.driverId && profile.compatibilityProviderType === entry.scope.providerType && driver, "profile/provider/driver scope drifted");
    assert(entry.scope.profileSchemaRevision === null || entry.scope.profileSchemaRevision === profile.profileSchemaRevision, "Profile schema revision drifted");
    assert(entry.scope.driverContractRevision === null || entry.scope.driverContractRevision === driver.driverContractRevision, "driver contract revision drifted");
    assert(["forward", "test", "discovery"].includes(entry.operation) && driver.operations?.[entry.operation] === "supported", "driver does not support this operation");
    const app = { messages: "claude", chat: "codex", responses: "codex", gemini: "gemini" }[entry.scope.surface];
    assert(entry.scope.surface === "management" || profile.app === app, "surface does not belong to its Profile");
    readSource(entry.source, repoRoot, entry.scope);
    if (entry.verificationLevel === "live_verified") {
      assert(entry.liveState === "live_verified", "live evidence has split truth");
      validateReceiptSummary(entry, repoRoot);
      result.live += 1;
    } else {
      assert(entry.liveState === "live_pending" && entry.receipt === null, "offline/historical evidence cannot cite a live receipt");
      if (entry.verificationLevel === "fixture_verified") {
        assert(entry.source.kind === "fixture", "fixture must cite a fixture artifact");
        result.fixture += 1;
      } else {
        assert(entry.verificationLevel === "historical_evidence" && entry.source.kind === "historical_document", "legacy documentation must remain historical evidence");
        result.historical += 1;
      }
    }
  }
  const serialized = JSON.stringify(view).toLowerCase();
  for (const forbidden of ["authorization", "cookie", "accesstoken", "refreshtoken", "prompt", "email", "opaquereasoning"]) assert(!serialized.includes(forbidden), `private field ${forbidden} is forbidden`);
  return result;
}

export function matchingOperationEvidence(view, exactScope) {
  validateScope(exactScope);
  return (view?.entries ?? []).filter((entry) => same(entry.scope, exactScope));
}
