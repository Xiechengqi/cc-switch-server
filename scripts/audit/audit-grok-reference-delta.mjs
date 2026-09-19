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

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, nested]) => [key, canonical(nested)]),
    );
  }
  return value;
}

function objectDigest(value) {
  return sha256(JSON.stringify(canonical(value)));
}

const commitPattern = /^[a-f0-9]{40}$/;
const digestPattern = /^[a-f0-9]{64}$/;

function safeRelative(value, label) {
  assert(
    typeof value === "string" &&
      value.length > 0 &&
      !path.isAbsolute(value) &&
      !value.split("/").includes(".."),
    `${label} is not a safe relative path`,
  );
}

function gitTree(repository, commit) {
  return execFileSync("git", ["-C", repository, "rev-parse", `${commit}^{tree}`], {
    encoding: "utf8",
  }).trim();
}

function gitFile(repository, commit, filePath, encoding = "utf8") {
  return execFileSync("git", ["-C", repository, "show", `${commit}:${filePath}`], {
    encoding,
    maxBuffer: 64 * 1024 * 1024,
  });
}

const contract = JSON.parse(fs.readFileSync(contractPath, "utf8"));
assert(
  contract.format === "cc-switch-grok-reference-delta" &&
    contract.schemaVersion === 2 &&
    contract.legacySchemaVersion === 1,
  "Grok reference delta format changed",
);
assert(
  Number.isFinite(Date.parse(contract.capturedAt)) &&
    Number.isFinite(Date.parse(contract.updatedAt)) &&
    Date.parse(contract.updatedAt) >= Date.parse(contract.capturedAt),
  "Grok evidence timestamps are invalid",
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
assert(
  contract.policy?.liveEvidence?.includes("fixtures never prove live OAuth") &&
    contract.policy.liveEvidence.includes("remote compaction"),
  "Grok fixture/live boundary changed",
);

assert(
  contract.observationPolicy?.history ===
    "append_only; every schema v1 content field is immutable" &&
    contract.observationPolicy?.sourceReadMode === "read_only_committed_git_objects" &&
    contract.observationPolicy?.targetReadMode === "committed_git_objects" &&
    contract.observationPolicy?.externalVerification?.includes(
      "never a build, test, release or runtime dependency",
    ) &&
    objectDigest(contract.observationPolicy) ===
      "f92535b5fb4e777048729ccd8731bd4a469b241455d2eb31a4474ed62e1f8774",
  "Grok observation policy changed",
);

const expectedLegacyFields = [
  "format",
  "schemaVersion",
  "capturedAt",
  "policy",
  "sources",
  "capabilities",
  "enhancements",
  "fixtureAcceptance",
  "realAcceptance",
  "remoteCompactionGate",
];
const legacy = contract.legacyV1;
assert(
  legacy?.path === "assets/contract/grok-reference-delta.json" &&
    legacy.targetCommit === "269850ac6922e763c6429ab1d06b5d69843646c8" &&
    legacy.targetTree === "3a8668ad7af8b6545aad45bf2962b6e57cdffc16" &&
    legacy.fileSha256 === "54bba90fa332c416e53fc0c3d9408250eea6141c8403102429d4d20bc83f0214" &&
    legacy.canonicalDigest === "1124873f44cd8e95cc87a313def458e9b36626e08056e73ad1bc9f4602e63bcf" &&
    JSON.stringify(legacy.fields) === JSON.stringify(expectedLegacyFields) &&
    objectDigest(legacy) === "9e038f7b5989dfbd97a317603f677c3e47a643ed86a3f587bdf01abe7200e69f",
  "Grok legacy v1 identity changed",
);
safeRelative(legacy.path, "Grok legacy v1 path");
assert(
  gitTree(repoRoot, legacy.targetCommit) === legacy.targetTree,
  "Grok legacy v1 target tree drifted",
);
const legacyRaw = gitFile(repoRoot, legacy.targetCommit, legacy.path, null);
assert(sha256(legacyRaw) === legacy.fileSha256, "Grok legacy v1 file drifted");
const legacyContract = JSON.parse(legacyRaw.toString("utf8"));
assert(
  legacyContract.schemaVersion === 1 &&
    JSON.stringify(Object.keys(legacyContract)) === JSON.stringify(expectedLegacyFields),
  "Grok committed legacy v1 field set changed",
);
const reconstructedLegacy = {};
for (const field of expectedLegacyFields) {
  const legacyValue = legacyContract[field];
  const currentValue = field === "schemaVersion" ? contract.legacySchemaVersion : contract[field];
  assert(
    digestPattern.test(legacy.fieldDigests?.[field]) &&
      objectDigest(legacyValue) === legacy.fieldDigests[field] &&
      objectDigest(currentValue) === legacy.fieldDigests[field],
    `Grok legacy ${field} history changed`,
  );
  reconstructedLegacy[field] = currentValue;
}
assert(
  objectDigest(reconstructedLegacy) === legacy.canonicalDigest &&
    objectDigest(
      Object.fromEntries(expectedLegacyFields.map((field) => [field, legacyContract[field]])),
    ) === legacy.canonicalDigest,
  "Grok legacy v1 canonical digest changed",
);

assert(
  objectDigest(contract.sourceExtensions) ===
    "948594aa54e0483eb22de484c840b269ad527366d0a64a63879ec1c88d7846ea",
  "Grok source extension history changed",
);
const sourceById = new Map();
const sourceRootById = new Map();
const sourceFileById = new Map();
for (const source of [...(contract.sources ?? []), ...(contract.sourceExtensions ?? [])]) {
  assert(
    source.id && source.rootEnv && source.defaultRelativeRoot,
    "Grok source metadata is incomplete",
  );
  assert(!sourceById.has(source.id), `duplicate Grok source ${source.id}`);
  assert(commitPattern.test(source.commit), `${source.id} has an invalid commit`);
  const sourceRoot = path.resolve(
    repoRoot,
    process.env[source.rootEnv] || source.defaultRelativeRoot,
  );
  const fileByPath = new Map();
  for (const file of source.files ?? []) {
    safeRelative(file.path, `${source.id} evidence path`);
    assert(!fileByPath.has(file.path), `${source.id} repeats ${file.path}`);
    assert(digestPattern.test(file.sha256), `${source.id}:${file.path} has invalid SHA-256`);
    fileByPath.set(file.path, file);
    if (checkSources) {
      assert(fs.existsSync(sourceRoot), `${source.id} source root is unavailable`);
      const content = gitFile(sourceRoot, source.commit, file.path, null);
      assert(
        sha256(content) === file.sha256,
        `${source.id}:${file.path} drifted from the reviewed Git object`,
      );
    }
  }
  assert(fileByPath.size > 0, `${source.id} has no frozen evidence files`);
  sourceById.set(source.id, source);
  sourceRootById.set(source.id, sourceRoot);
  sourceFileById.set(source.id, fileByPath);
}
assert(
  sourceById.size === 2 && sourceById.has("grok2api") && sourceById.has("sub2api"),
  "Grok source set changed",
);
const grok2api = sourceById.get("grok2api");
assert(
  grok2api.commit === "906b9493b099d192381c698d4e320fafeccb851c" &&
    grok2api.qualityPolicyCommit === "7f3f3d3ce030d5cf946b0bbe994f3026775fa308" &&
    Array.isArray(grok2api.historicalCommits) &&
    grok2api.historicalCommits.length === 8 &&
    grok2api.files.length === 10,
  "grok2api frozen history changed",
);
const sub2api = sourceById.get("sub2api");
assert(
  sub2api.commit === "ab99d56e9626e6cd731592dae8553c9758a0efa2" &&
    sub2api.files.length === 4,
  "sub2api rejected-source history changed",
);

const immutableSourceSnapshotDigests = new Map([
  ["grok2api-2026-09-19", "d9a5a4cccc9c5decb96f033bdc696b9a6af647b4602a3a3fbc8278744c2590d8"],
  ["sub2api-2026-09-19", "c837f83cee57f13f6fd14510402c7b996777f9ad7d1b878751c65e7dcfcb6b77"],
]);
const sourceSnapshotById = new Map();
for (const snapshot of contract.sourceSnapshots ?? []) {
  assert(snapshot.id && !sourceSnapshotById.has(snapshot.id), "duplicate Grok source snapshot");
  const source = sourceById.get(snapshot.sourceId);
  assert(source, `${snapshot.id} references an unknown source`);
  assert(
    snapshot.headCommit === source.commit &&
      commitPattern.test(snapshot.headTree) &&
      snapshot.readMode === "read_only_committed_git_objects",
    `${snapshot.id} has invalid committed source identity`,
  );
  if (snapshot.sourceId === "grok2api") {
    assert(
      snapshot.repository === "grok2api" &&
        snapshot.worktreeClean === true &&
        snapshot.worktreeChangesExcluded === false &&
        snapshot.excludedWorktreeEntries === 0,
      `${snapshot.id} clean-worktree declaration changed`,
    );
  } else {
    assert(
      snapshot.repository === "sub2api" &&
        snapshot.worktreeClean === false &&
        snapshot.worktreeChangesExcluded === true &&
        snapshot.excludedWorktreeEntries === 6,
      `${snapshot.id} does not exclude all six worktree entries`,
    );
  }
  assert(
    immutableSourceSnapshotDigests.get(snapshot.id) === objectDigest(snapshot),
    `${snapshot.id} changed after it was recorded`,
  );
  if (checkSources) {
    assert(
      gitTree(sourceRootById.get(snapshot.sourceId), snapshot.headCommit) === snapshot.headTree,
      `${snapshot.id} HEAD tree drifted`,
    );
  }
  sourceSnapshotById.set(snapshot.id, snapshot);
}
assert(
  sourceSnapshotById.size === immutableSourceSnapshotDigests.size,
  "Grok source snapshot history is incomplete",
);

assert(
  objectDigest(contract.reviewedRejections) ===
    "51e2d1745215d7d64f947f1e148941f12e7230c2913ebb056f5627879c977628",
  "Grok reviewed rejection history changed",
);
const reviewedRejections = new Map(
  (contract.reviewedRejections ?? []).map((entry) => [entry.id, entry]),
);
const rejectedBoundary = reviewedRejections.get("GR-R1");
assert(
  reviewedRejections.size === 1 &&
    rejectedBoundary?.status === "rejected" &&
    rejectedBoundary.productionCodeChanged === false &&
    JSON.stringify(rejectedBoundary.behaviors) ===
      JSON.stringify([
        "visible_or_plaintext_ratio_automatic_retry",
        "account_pool_selection_or_rotation",
        "commercial_composite_routing_or_billing",
        "local_soft_quota_gate",
      ]) &&
    rejectedBoundary.boundary.includes("diagnostics cannot authorize retries or routing"),
  "GR-R1 rejection boundary changed",
);

const immutableObservationDigests = new Map([
  ["GR-OBS-0001", "b5d7e5f332f2181bbe0034c1ed3e3bd443f1113cb2928bf395002ef8d9b27099"],
  ["GR-OBS-0002", "fc9886903ab1964537105630e18568f78906d0b005172bee041ee4290ae7e99b"],
  ["GR-OBS-0003", "993683c6964889b7f2e658cce475048389bde2c1eec10a9058bcd85d9ac6c7b6"],
  ["GR-OBS-0004", "69937bac2a9bc8a284e19c2d1bfabdb186f44774f3ec864d04cfc2de1e8c9fff"],
  ["GR-OBS-0005", "ec43d4883c8b78e30e7a1804308851503125ffb531af7b1c2a1889a887f71206"],
  ["GR-OBS-0006", "8aef1f9a704715887ff740c90a563e1fe494efb0fbe3b6a72770b81736bc5cee"],
  ["GR-OBS-0007", "62b89a4959fafdea1cf6b4f096cfb9a551133429a1c3900384b26edeff374815"],
  ["GR-OBS-0008", "924cc1ce48eff0a0b43265211e9c2ad865890f2fb9a96e8481c8bce6592187af"],
  ["GR-OBS-0009", "af09e7c34cff0d9699e8956fb58a766c186e3dfb93f4e4a39ae83193c3adbc75"],
  ["GR-OBS-0010", "f4c6f439470de9db744c0387d56679aeb3d1b70d40424a56221b456211d47404"],
]);
const expectedEnhancementIds = new Set([
  "GR-01",
  "GR-02",
  "GR-03",
  "GR-04",
  "GR-05",
  "GR-N1",
  "CORE-N1",
  "LIVE-N1",
  "GR-R1",
]);
const expectedDispositions = new Map([
  ["GR-OBS-0001", "adopt"],
  ["GR-OBS-0002", "differential"],
  ["GR-OBS-0003", "differential"],
  ["GR-OBS-0004", "live_gate"],
  ["GR-OBS-0005", "live_gate"],
  ["GR-OBS-0006", "differential"],
  ["GR-OBS-0007", "reject"],
  ["GR-OBS-0008", "differential"],
  ["GR-OBS-0009", "live_gate"],
  ["GR-OBS-0010", "reject"],
]);
const observationIds = new Set();
const observedEnhancementIds = new Set();
const observedDeltaIds = new Set();
for (const observation of contract.observations ?? []) {
  assert(observation.id && !observationIds.has(observation.id), "duplicate Grok observation");
  observationIds.add(observation.id);
  assert(observation.providerFamily === "grok", `${observation.id} changed provider family`);
  assert(Number.isFinite(Date.parse(observation.observedAt)), `${observation.id} has an invalid observedAt`);
  assert(
    observation.disposition === expectedDispositions.get(observation.id),
    `${observation.id} has an invalid disposition`,
  );
  assert(
    typeof observation.reason === "string" && observation.reason.trim().length > 0,
    `${observation.id} has no disposition reason`,
  );
  assert(
    Array.isArray(observation.enhancementIds) &&
      observation.enhancementIds.length > 0 &&
      observation.enhancementIds.every((id) => expectedEnhancementIds.has(id)),
    `${observation.id} has an invalid enhancement mapping`,
  );
  for (const id of observation.enhancementIds) observedEnhancementIds.add(id);

  const reference = observation.reference ?? {};
  const source = sourceById.get(reference.sourceId);
  const snapshot = sourceSnapshotById.get(reference.snapshotId);
  assert(
    source && snapshot?.sourceId === reference.sourceId,
    `${observation.id} has an invalid source snapshot`,
  );
  assert(
    reference.repository === snapshot.repository &&
      commitPattern.test(reference.commit) &&
      commitPattern.test(reference.tree),
    `${observation.id} changed committed source identity`,
  );
  const allowedCommits = new Set([source.commit]);
  if (source.qualityPolicyCommit) allowedCommits.add(source.qualityPolicyCommit);
  assert(allowedCommits.has(reference.commit), `${observation.id} references an unfrozen source commit`);
  assert(
    typeof reference.deltaId === "string" &&
      reference.deltaId.length > 0 &&
      !observedDeltaIds.has(reference.deltaId),
    `${observation.id} has an invalid or duplicate source delta`,
  );
  observedDeltaIds.add(reference.deltaId);
  assert(
    Array.isArray(reference.paths) &&
      reference.paths.length > 0 &&
      new Set(reference.paths).size === reference.paths.length,
    `${observation.id} has invalid source paths`,
  );
  const fileByPath = sourceFileById.get(reference.sourceId);
  const files = reference.paths.map((sourcePath) => {
    safeRelative(sourcePath, `${observation.id}:${sourcePath ?? "<missing>"}`);
    const file = fileByPath.get(sourcePath);
    assert(file, `${observation.id} references an unfrozen source path`);
    return file;
  });
  assert(
    Array.isArray(reference.symbols) &&
      reference.symbols.length > 0 &&
      reference.symbols.every(
        (symbol) => typeof symbol === "string" && symbol.trim().length > 0,
      ),
    `${observation.id} has no source symbols`,
  );
  const expectedSourceDigest = objectDigest({
    sourceId: reference.sourceId,
    deltaId: reference.deltaId,
    commit: reference.commit,
    tree: reference.tree,
    files,
  });
  assert(
    digestPattern.test(reference.sourceDigest) && reference.sourceDigest === expectedSourceDigest,
    `${observation.id} source digest drifted`,
  );

  const target = observation.target ?? {};
  assert(
    commitPattern.test(target.baselineCommit) &&
      commitPattern.test(target.baselineTree) &&
      gitTree(repoRoot, target.baselineCommit) === target.baselineTree,
    `${observation.id} has an invalid target baseline object`,
  );
  const rejected = observation.disposition === "reject";
  if (rejected) {
    assert(
      target.implementationCommit === null && target.implementationTree === null,
      `${observation.id} rejected behavior must not claim an implementation`,
    );
    assert(
      Array.isArray(target.fixtureIds) && target.fixtureIds.length === 0,
      `${observation.id} rejected behavior must not claim fixtures`,
    );
  } else {
    assert(
      commitPattern.test(target.implementationCommit) &&
        commitPattern.test(target.implementationTree) &&
        target.baselineCommit !== target.implementationCommit &&
        gitTree(repoRoot, target.implementationCommit) === target.implementationTree,
      `${observation.id} has an invalid target implementation object`,
    );
    assert(
      Array.isArray(target.fixtureIds) && target.fixtureIds.length > 0,
      `${observation.id} has no implementation fixtures`,
    );
  }
  const contractCommit = rejected ? target.baselineCommit : target.implementationCommit;
  const targetSources = [];
  for (const localContract of target.contracts ?? []) {
    safeRelative(localContract.path, `${observation.id}:${localContract.path ?? "<missing>"}`);
    const targetSource = gitFile(repoRoot, contractCommit, localContract.path);
    targetSources.push(targetSource);
    assert(
      Array.isArray(localContract.anchors) &&
        localContract.anchors.length > 0 &&
        localContract.anchors.every((anchor) => targetSource.includes(anchor)),
      `${observation.id} has an unavailable committed target anchor`,
    );
  }
  assert(targetSources.length > 0, `${observation.id} has no target contracts`);
  if (!rejected) {
    assert(
      target.fixtureIds.every((fixture) =>
        targetSources.some((targetSource) => targetSource.includes(fixture)),
      ),
      `${observation.id} has an unmapped fixture`,
    );
  }
  assert(
    immutableObservationDigests.get(observation.id) === objectDigest(observation),
    `${observation.id} changed after it was recorded`,
  );

  if (checkSources) {
    const sourceRoot = sourceRootById.get(reference.sourceId);
    assert(
      gitTree(sourceRoot, reference.commit) === reference.tree,
      `${observation.id} source tree drifted`,
    );
    const sourceTexts = [];
    for (const file of files) {
      const content = gitFile(sourceRoot, reference.commit, file.path, null);
      assert(sha256(content) === file.sha256, `${observation.id}:${file.path} source object drifted`);
      sourceTexts.push(content.toString("utf8"));
    }
    const sourceText = sourceTexts.join("\n");
    assert(
      reference.symbols.every((symbol) => sourceText.includes(symbol)),
      `${observation.id} source symbol drifted`,
    );
  }
}
assert(
  observationIds.size === immutableObservationDigests.size,
  "Grok observation history is incomplete",
);
assert(
  JSON.stringify([...observedEnhancementIds].sort()) ===
    JSON.stringify([...expectedEnhancementIds].sort()),
  "Grok enhancement observation coverage is incomplete",
);

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
for (const [id, expectedCount] of [
  ["GR-N1", 3],
  ["CORE-N1", 2],
  ["LIVE-N1", 3],
]) {
  const enhancement = enhancements.get(id);
  assert(
    Array.isArray(enhancement.localEvidence) && enhancement.localEvidence.length === expectedCount,
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
  enhancements.get("CORE-N1").status === "fixture_verified" &&
    enhancements.get("CORE-N1").wireChanged === false &&
    enhancements.get("CORE-N1").bindingChanged === false &&
    enhancements.get("CORE-N1").recoveryBudgetChanged === false,
  "CORE-N1 changed Grok wire, binding, or recovery budget",
);
assert(
  enhancements.get("LIVE-N1").status === "live_pending" &&
    enhancements.get("LIVE-N1").operationScoped === true &&
    enhancements.get("LIVE-N1").receiptsIndependent === true &&
    enhancements.get("LIVE-N1").runtimeAutoEnable === false,
  "LIVE-N1 receipt isolation changed",
);

const providerModule = fs.readFileSync(
  path.join(repoRoot, "src/proxy/providers/grok/mod.rs"),
  "utf8",
);
for (const anchor of [
  "prepare_http_replay",
  "prepare_transport_replay",
  "clear_rejected_and_reset",
  "binding_is_current",
]) {
  assert(providerModule.includes(anchor), `Grok Provider facade is missing ${anchor}`);
}
const providerIndex = fs.readFileSync(path.join(repoRoot, "src/proxy/providers/mod.rs"), "utf8");
assert(providerIndex.includes("mod grok;"), "Grok Provider facade is not registered");
const forwarderSource = fs.readFileSync(path.join(repoRoot, "src/proxy/forwarder.rs"), "utf8");
for (const anchor of [
  "grok_provider::prepare_http_replay",
  "grok_provider::ReplayStreamWrite::new",
  "grok_provider::commit",
]) {
  assert(forwarderSource.includes(anchor), `Grok shared forwarder lost ${anchor}`);
}

const fixtureChecks = contract.fixtureAcceptance?.checks ?? [];
assert(
  fixtureChecks.length === 14 && new Set(fixtureChecks).size === 14,
  "Grok fixture matrix changed",
);
assert(
  JSON.stringify(contract.fixtureAcceptance?.transportRails) ===
    JSON.stringify(["http", "sse", "websocket"]),
  "Grok transport fixture matrix changed",
);

const acceptance = contract.realAcceptance;
assert(
  acceptance?.receiptSchemaVersion === 1 && acceptance.harnessRevision === 1,
  "Grok receipt schema or harness revision changed",
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

const receiptHarness = fs.readFileSync(
  path.join(repoRoot, "scripts/smoke/grok-real-receipt.mjs"),
  "utf8",
);
for (const boundary of [
  "Grok normal-path probing and acceptance are intentionally separate",
  "current target commit is unavailable",
  "Grok receipt file must stay outside the repository",
  "Grok receipt file must have mode 0600",
  "receipt.targetCommit !== targetCommit",
  'verificationState: "blocked_inputs"',
  'liveState: "live_pending"',
]) {
  assert(receiptHarness.includes(boundary), `Grok receipt harness lost ${boundary}`);
}
const probeHarness = fs.readFileSync(
  path.join(repoRoot, "scripts/smoke/grok-oauth-real.mjs"),
  "utf8",
);
assert(
  probeHarness.includes("probe_only") && probeHarness.includes("live_pending"),
  "Grok normal-path probe claimed live acceptance",
);

console.log(
  `grok reference delta audit ok (${expectedLegacyFields.length} legacy fields, ${sourceSnapshotById.size} source snapshots, ${observationIds.size} immutable observations, ${fixtureChecks.length} fixture checks, ${[...operations.values()].reduce((total, entry) => total + entry.requiredChecks.length, 0)} operation checks${
    checkSources ? ", external objects verified" : ", external check optional"
  })`,
);
