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
  contract.format === "cc-switch-kiro-reference-delta" &&
    contract.schemaVersion === 2 &&
    contract.legacySchemaVersion === 1,
  "Kiro reference delta format changed",
);
assert(
  Number.isFinite(Date.parse(contract.capturedAt)) &&
    Number.isFinite(Date.parse(contract.updatedAt)) &&
    Date.parse(contract.updatedAt) >= Date.parse(contract.capturedAt),
  "Kiro evidence timestamps are invalid",
);
assert(
  contract.policy?.externalSources === "read_only_optional_audit_input_never_runtime_dependency",
  "Kiro external-source boundary changed",
);
for (const invariant of [
  "no pool",
  "no rotation",
  "no credential-rail fallback",
  "no cross-region fallback",
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
  "Kiro observation policy changed",
);

const expectedLegacyFields = [
  "format",
  "schemaVersion",
  "capturedAt",
  "policy",
  "sources",
  "capabilities",
  "incrementalEnhancements",
  "fixtureAcceptance",
  "realAcceptance",
  "sharedCacheGate",
];
const legacy = contract.legacyV1;
assert(
  legacy?.path === "assets/contract/kiro-reference-delta.json" &&
    legacy.targetCommit === "1124082bc5a1799f03b9e9b63e2e410652497028" &&
    legacy.targetTree === "4c40bf67ce5c7e301bfde72874cb9fd1be741c6a" &&
    legacy.fileSha256 === "728d5b1b71b00caba869d1f0a518f1fd6e54ab5115afce842e82905e00cb9809" &&
    legacy.canonicalDigest === "a05869c3eecc15abb6029e306259ae8c0702b413fac927b7c65bf2e7067a33fc" &&
    JSON.stringify(legacy.fields) === JSON.stringify(expectedLegacyFields) &&
    objectDigest(legacy) === "2185cac059b76c5713a850964839fcce2fb1d2de86707876de2b2bc660c5ec94",
  "Kiro legacy v1 identity changed",
);
safeRelative(legacy.path, "Kiro legacy v1 path");
assert(
  gitTree(repoRoot, legacy.targetCommit) === legacy.targetTree,
  "Kiro legacy v1 target tree drifted",
);
const legacyRaw = gitFile(repoRoot, legacy.targetCommit, legacy.path, null);
assert(sha256(legacyRaw) === legacy.fileSha256, "Kiro legacy v1 file drifted");
const legacyContract = JSON.parse(legacyRaw.toString("utf8"));
assert(
  legacyContract.schemaVersion === 1 &&
    JSON.stringify(Object.keys(legacyContract)) === JSON.stringify(expectedLegacyFields),
  "Kiro committed legacy v1 field set changed",
);
const reconstructedLegacy = {};
for (const field of expectedLegacyFields) {
  const legacyValue = legacyContract[field];
  const currentValue = field === "schemaVersion" ? contract.legacySchemaVersion : contract[field];
  assert(
    digestPattern.test(legacy.fieldDigests?.[field]) &&
      objectDigest(legacyValue) === legacy.fieldDigests[field] &&
      objectDigest(currentValue) === legacy.fieldDigests[field],
    `Kiro legacy ${field} history changed`,
  );
  reconstructedLegacy[field] = currentValue;
}
assert(
  objectDigest(reconstructedLegacy) === legacy.canonicalDigest &&
    objectDigest(
      Object.fromEntries(expectedLegacyFields.map((field) => [field, legacyContract[field]])),
    ) === legacy.canonicalDigest,
  "Kiro legacy v1 canonical digest changed",
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
  for (const delta of source.deltas ?? []) {
    assert(delta.id, `${source.id} has an unnamed delta`);
    assert(
      Array.isArray(delta.commits) &&
        delta.commits.length > 0 &&
        delta.commits.every((commit) => /^[a-f0-9]{40}$/.test(commit)),
      `${source.id}:${delta.id} has invalid commits`,
    );
    assert(delta.files?.length > 0, `${source.id}:${delta.id} has no evidence files`);
    for (const file of delta.files) {
      safeRelative(file.path, `${source.id}:${delta.id} evidence path`);
      assert(
        delta.commits.includes(file.commit),
        `${source.id}:${delta.id}:${file.path} references an unpinned commit`,
      );
      assert(
        /^[a-f0-9]{64}$/.test(file.sha256),
        `${source.id}:${delta.id}:${file.path} has invalid SHA-256`,
      );
      if (checkSources) {
        assert(fs.existsSync(sourceRoot), `${source.id} source root is unavailable`);
        const content = execFileSync(
          "git",
          ["-C", sourceRoot, "show", `${file.commit}:${file.path}`],
          { encoding: null, maxBuffer: 64 * 1024 * 1024 },
        );
        assert(
          sha256(content) === file.sha256,
          `${source.id}:${delta.id}:${file.path} drifted from the reviewed Git object`,
        );
      }
    }
  }
}

assert(
  objectDigest(contract.sourceExtensions) ===
    "20371c3723bec8b69ae0799be87eb8f5b01451330e85b49b6eab21303a25b1d0",
  "Kiro source extension history changed",
);
const sourceById = new Map();
const sourceRootById = new Map();
const sourceFileById = new Map();
for (const source of [...(contract.sources ?? []), ...(contract.sourceExtensions ?? [])]) {
  assert(
    source.id && source.rootEnv && source.defaultRelativeRoot,
    "Kiro source metadata is incomplete",
  );
  assert(!sourceById.has(source.id), `duplicate Kiro source ${source.id}`);
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
  sourceById.size === 2 &&
    sourceById.has("kiro-rs") &&
    sourceById.has("kiro-rs-head"),
  "Kiro source set changed",
);
const currentSource = sourceById.get("kiro-rs-head");
assert(
  currentSource.commit === "be0c04219d9d1b93b7fe5c3d7b9e7c9cf0d05863" &&
    currentSource.previousReviewedCommit === "22d2c2d0695ba350890072c19990f54782827ae5" &&
    currentSource.files.length === 14,
  "Kiro current committed source history changed",
);

const immutableSourceSnapshotDigests = new Map([
  [
    "kiro-rs-2026-09-19",
    "8ff18226bce2f55aa6225b8ada7135374116c078ded3fb51ca2c7ba5c358c479",
  ],
]);
const sourceSnapshotById = new Map();
for (const snapshot of contract.sourceSnapshots ?? []) {
  assert(snapshot.id && !sourceSnapshotById.has(snapshot.id), "duplicate Kiro source snapshot");
  const source = sourceById.get(snapshot.sourceId);
  assert(source, `${snapshot.id} references an unknown source`);
  assert(
    snapshot.repository === "kiro.rs" &&
      snapshot.headCommit === source.commit &&
      snapshot.headTree === "5e656c1bf70aac0224a68251b2065966db01e933" &&
      snapshot.worktreeClean === true &&
      snapshot.worktreeChangesExcluded === false &&
      snapshot.excludedWorktreeEntries === 0 &&
      snapshot.readMode === "read_only_committed_git_objects",
    `${snapshot.id} has invalid committed source identity`,
  );
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
  "Kiro source snapshot history is incomplete",
);

assert(
  objectDigest(contract.reviewedRejections) ===
    "ff35a1a3a19a71b8471e1b74fbffdfbdab61c9383af68ec3f1df710274e5a1c0",
  "Kiro reviewed rejection history changed",
);
const reviewedRejections = new Map(
  (contract.reviewedRejections ?? []).map((entry) => [entry.id, entry]),
);
assert(
  reviewedRejections.size === 3 &&
    reviewedRejections.get("KI-R1")?.status === "rejected" &&
    reviewedRejections.get("KI-R1")?.behaviors.includes("account_pool_selection_or_rotation") &&
    reviewedRejections.get("KI-R2")?.status === "rejected" &&
    reviewedRejections.get("KI-R2")?.behaviors.includes("reference_redis_runtime_dependency") &&
    reviewedRejections.get("KI-R3")?.status === "rejected" &&
    reviewedRejections.get("KI-R3")?.behaviors.includes("receipt_auto_enables_compaction"),
  "Kiro rejection boundaries changed",
);

const immutableObservationDigests = new Map([
  ["KI-OBS-0001", "53ddcea63ea2a9e75b273cbcdaded2f0094ee09cfdfbfe64dd8af3416490030a"],
  ["KI-OBS-0002", "74d4ce13815bb61987e93ac669c5519a5a89b8e529cb1df06cf4b17547e367ee"],
  ["KI-OBS-0003", "c4ed55de6a553a6ea3d3371eb3dcab60c0e7fa508b8169a54754b1ce4aeda0d3"],
  ["KI-OBS-0004", "605f4c52e7de14e4fd4be8309155fc15acf2dfa66bc5879bb5c8b40500c651cb"],
  ["KI-OBS-0005", "37aa7ee8dd00d991bdd6fce0ebd26ccb8b73773e99caa5e963037cccfa882436"],
  ["KI-OBS-0006", "3fb9a847471cc059309c95954869a607365302178cb5c351562894824bf1f451"],
  ["KI-OBS-0007", "cd3099c0d207ee3399d6329d339b0280b22c76736734f74f65d69ceb52d18ef7"],
  ["KI-OBS-0008", "1a78e3d2da16082a2f18a4f45f39973786b971b2efbcfba893ad0aebb5156127"],
  ["KI-OBS-0009", "519038ef40174450f2f4d496bb84315bf31264ecc6bdbb8bd5631b192f143fc0"],
  ["KI-OBS-0010", "ab139969f4564e175209391c8ce75dad9364cd4ff330f054b94604d15bb541ea"],
  ["KI-OBS-0011", "78fdbf5d963fb7793cc30c865fc9674d50b62ae48cb227919bc82a77c22f2266"],
  ["KI-OBS-0012", "9b4393cb33aa4977c72d38320413fad6567f504e027d6c4e1dc307d5f97bf176"],
  ["KI-OBS-0013", "a5378092cba17dc28ebb7d4bc81eaa837e009952a2691b43e7839bc840d752d1"],
]);
const expectedEnhancementIds = new Set([
  "KI-01",
  "KI-02",
  "KI-03",
  "KI-04",
  "KI-05",
  "KI-N1",
  "KI-N2",
  "KI-N3",
  "CORE-N1",
  "LIVE-N1",
  "KI-R1",
  "KI-R2",
  "KI-R3",
]);
const expectedDispositions = new Map([
  ["KI-OBS-0001", "adopt"],
  ["KI-OBS-0002", "differential"],
  ["KI-OBS-0003", "differential"],
  ["KI-OBS-0004", "live_gate"],
  ["KI-OBS-0005", "live_gate"],
  ["KI-OBS-0006", "adopt"],
  ["KI-OBS-0007", "differential"],
  ["KI-OBS-0008", "live_gate"],
  ["KI-OBS-0009", "differential"],
  ["KI-OBS-0010", "live_gate"],
  ["KI-OBS-0011", "reject"],
  ["KI-OBS-0012", "reject"],
  ["KI-OBS-0013", "reject"],
]);
const observationIds = new Set();
const observedEnhancementIds = new Set();
const observedDeltaIds = new Set();
for (const observation of contract.observations ?? []) {
  assert(observation.id && !observationIds.has(observation.id), "duplicate Kiro observation");
  observationIds.add(observation.id);
  assert(observation.providerFamily === "kiro", `${observation.id} changed provider family`);
  assert(Number.isFinite(Date.parse(observation.observedAt)), `${observation.id} has invalid time`);
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
      reference.commit === source.commit &&
      reference.tree === snapshot.headTree,
    `${observation.id} changed committed source identity`,
  );
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
    digestPattern.test(reference.sourceDigest) &&
      reference.sourceDigest === expectedSourceDigest,
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
      assert(
        sha256(content) === file.sha256,
        `${observation.id}:${file.path} source object drifted`,
      );
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
  "Kiro observation history is incomplete",
);
assert(
  JSON.stringify([...observedEnhancementIds].sort()) ===
    JSON.stringify([...expectedEnhancementIds].sort()),
  "Kiro enhancement observation coverage is incomplete",
);

assert(
  objectDigest(contract.evidenceExtensions) ===
    "3bbe6eb43e0c7ba1d4e37b686b2b367d3aa0e158fd80057b8bd76eb701f6f770",
  "Kiro evidence extension history changed",
);
const evidenceExtensions = new Map(
  (contract.evidenceExtensions ?? []).map((entry) => [entry.id, entry]),
);
assert(
  evidenceExtensions.size === 2 &&
    evidenceExtensions.has("CORE-N1") &&
    evidenceExtensions.has("LIVE-N1"),
  "Kiro CORE/LIVE evidence extension set changed",
);
for (const [id, expectedCount] of [
  ["CORE-N1", 2],
  ["LIVE-N1", 3],
]) {
  const extension = evidenceExtensions.get(id);
  assert(
    Array.isArray(extension.localEvidence) && extension.localEvidence.length === expectedCount,
    `${id} local evidence set changed`,
  );
  for (const evidence of extension.localEvidence) {
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
  evidenceExtensions.get("CORE-N1").status === "fixture_verified" &&
    evidenceExtensions.get("CORE-N1").wireChanged === false &&
    evidenceExtensions.get("CORE-N1").bindingChanged === false &&
    evidenceExtensions.get("CORE-N1").attemptBudgetChanged === false,
  "CORE-N1 changed Kiro wire, binding, or attempt budget",
);
assert(
  evidenceExtensions.get("LIVE-N1").status === "live_pending" &&
    evidenceExtensions.get("LIVE-N1").authKindRegionScoped === true &&
    evidenceExtensions.get("LIVE-N1").receiptCount === 8 &&
    evidenceExtensions.get("LIVE-N1").receiptsIndependent === true &&
    evidenceExtensions.get("LIVE-N1").runtimeAutoEnable === false,
  "LIVE-N1 receipt isolation or runtime gate changed",
);

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

const sourceDeltaIds = new Set(
  (contract.sources ?? []).flatMap((source) =>
    (source.deltas ?? []).map((delta) => delta.id),
  ),
);
const incrementalEnhancements = new Map(
  (contract.incrementalEnhancements ?? []).map((enhancement) => [
    enhancement.id,
    enhancement,
  ]),
);
assert(
  incrementalEnhancements.size === 3,
  "Kiro incremental contract must contain KI-N1 through KI-N3",
);
for (let index = 1; index <= 3; index += 1) {
  const id = `KI-N${index}`;
  const enhancement = incrementalEnhancements.get(id);
  assert(enhancement, `Kiro incremental contract is missing ${id}`);
  assert(
    sourceDeltaIds.has(enhancement.referenceDelta),
    `${id} references an unknown source delta`,
  );
  assert(
    Array.isArray(enhancement.evidence) && enhancement.evidence.length > 0,
    `${id} must pin local evidence`,
  );
  for (const evidence of enhancement.evidence) {
    safeRelative(evidence.path, `${id} local evidence path`);
    const localPath = path.join(repoRoot, evidence.path);
    assert(fs.existsSync(localPath), `${id} local evidence path is unavailable`);
    const local = fs.readFileSync(localPath, "utf8");
    for (const anchor of evidence.anchors ?? []) {
      assert(local.includes(anchor), `${id} is missing local anchor ${anchor}`);
    }
  }
}
const toolNames = incrementalEnhancements.get("KI-N1");
assert(toolNames?.status === "fixture_verified", "KI-N1 must remain fixture_verified");
assert(
  JSON.stringify(toolNames.resolutionOrder) ===
    JSON.stringify([
      "exact_upstream_mapping",
      "exact_original_declaration",
      "unique_namespaced_child",
      "ordinary_unmatched_name",
    ]) &&
    toolNames.ambiguousChild === "KIRO_EVENT_STREAM_INVALID",
  "KI-N1 tool-name resolution order or ambiguity contract changed",
);
for (const invariant of ["current request", "excludes history", "Account", "Share", "session"] ) {
  assert(toolNames.registryScope?.includes(invariant), `KI-N1 registry scope lost ${invariant}`);
}
const fallback = incrementalEnhancements.get("KI-N2");
assert(fallback?.status === "fixture_verified", "KI-N2 must remain fixture_verified");
assert(
  JSON.stringify(fallback.allowedProfilelessRetry) ===
    JSON.stringify([
      "403_with_profile_arn",
      "400_Improperly_formed_request_with_profile_arn",
      "400_Invalid_profileArn_with_profile_arn",
    ]),
  "KI-N2 allowed profileArn fallback set changed",
);
assert(
  JSON.stringify(fallback.forbiddenFallbackClasses) ===
    JSON.stringify([
      "401",
      "429",
      "5xx",
      "timeout",
      "tls",
      "transport",
      "decode",
      "invalid_success_contract",
    ]) && fallback.usageHostFallbackEnabled === false,
  "KI-N2 widened a failure or host fallback boundary",
);
const compact = incrementalEnhancements.get("KI-N3");
assert(
  compact?.status === "fixture_verified_fail_closed" &&
    compact.runtimeEnabled === false &&
    compact.liveStatus === "live_pending" &&
    compact.receipt === null &&
    compact.zeroUpstreamBeforeReject === true,
  "KI-N3 remote compaction gate opened without live evidence",
);

const fixtureChecks = contract.fixtureAcceptance?.checks ?? [];
assert(
  fixtureChecks.length === 16 && new Set(fixtureChecks).size === 16,
  "Kiro fixture acceptance matrix changed",
);
const acceptance = contract.realAcceptance;
assert(acceptance?.receiptSchemaVersion === 1, "Kiro receipt schema version changed");
assert(
  acceptance.harnessRevision === 1,
  "Kiro receipt harness revision changed",
);
assert(
  acceptance.requiredChecks?.length === 29 &&
    new Set(acceptance.requiredChecks).size === 29,
  "Kiro real acceptance checks changed",
);
assert(
  acceptance.requiredBodyHashes?.length === 13 &&
    new Set(acceptance.requiredBodyHashes).size === 13,
  "Kiro real acceptance body hashes changed",
);
assert(
  acceptance.requiredMeasurements?.length === 7 &&
    new Set(acceptance.requiredMeasurements).size === 7,
  "Kiro real acceptance measurements changed",
);
assert(
  JSON.stringify(acceptance.requiredDecisions) ===
    JSON.stringify([
      "sameAccount401",
      "second401",
      "identityGenerationDrift",
      "crossAuthKindFallback",
      "crossRegionFallback",
      "crossAccountFallback",
      "crossProviderFallback",
      "crossShareFallback",
      "postCommitReplay",
      "catalogStale",
      "compactRuntime",
      "receiptAutoEnablesCompact",
      "sharedCacheRuntime",
    ]),
  "Kiro real acceptance recovery decisions changed",
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
for (const localPath of [
  "scripts/smoke/kiro-real-receipt.mjs",
  "scripts/audit/kiro-real-receipt.test.mjs",
]) {
  assert(fs.existsSync(path.join(repoRoot, localPath)), `Kiro acceptance harness is missing ${localPath}`);
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
  `kiro reference delta audit ok (${expectedLegacyFields.length} legacy fields, ${sourceSnapshotById.size} committed-object snapshot, ${observationIds.size} immutable observations, ${reviewedRejections.size} rejection boundaries, ${receipts.length} pending receipts${
    checkSources ? ", external objects verified" : ", external check optional"
  })`,
);
