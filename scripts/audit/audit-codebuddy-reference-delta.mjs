#!/usr/bin/env node

import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const contractPath = path.join(
  repoRoot,
  "assets/contract/codebuddy-reference-delta.json",
);
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
  contract.format === "cc-switch-codebuddy-reference-delta" &&
    contract.schemaVersion === 2 &&
    contract.legacySchemaVersion === 1,
  "CodeBuddy reference delta format changed",
);
assert(
  Number.isFinite(Date.parse(contract.capturedAt)) &&
    Number.isFinite(Date.parse(contract.updatedAt)) &&
    Date.parse(contract.updatedAt) >= Date.parse(contract.capturedAt),
  "CodeBuddy evidence timestamps are invalid",
);
assert(
  contract.policy?.externalSources === "read_only_optional_audit_input_never_runtime_dependency",
  "CodeBuddy external-source boundary changed",
);
assert(
  contract.policy?.sourceWorktree?.includes("committed Git objects") &&
    contract.policy.sourceWorktree.includes("untracked and modified files are excluded"),
  "CodeBuddy committed-object evidence boundary changed",
);
for (const invariant of [
  "single bound Provider/Share/Account",
  "immutable site",
  "same-account same-rail pre-commit recovery only",
  "no pool",
  "no rotation",
  "no credential fallback",
  "no domain fallback",
  "no cross-account fallback",
  "no cross-Provider fallback",
]) {
  assert(contract.policy.scope.includes(invariant), `CodeBuddy scope lost ${invariant}`);
}
assert(
  contract.policy.liveEvidence.includes("separate complete real receipts") &&
    contract.policy.liveEvidence.includes("never upgrades either site"),
  "CodeBuddy live-evidence boundary changed",
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
  "CodeBuddy observation policy changed",
);

const expectedLegacyFields = [
  "format",
  "schemaVersion",
  "capturedAt",
  "policy",
  "sources",
  "registryTruth",
  "capabilities",
  "incrementalEnhancements",
  "fixtureAcceptance",
  "providerLifecycle",
  "realAcceptance",
];
const legacy = contract.legacyV1;
assert(
  legacy?.path === "assets/contract/codebuddy-reference-delta.json" &&
    legacy.targetCommit === "d81056c786196357c1074336a06a6cb847c9f9d6" &&
    legacy.targetTree === "3ad3b2aa8ee0b18d2c36bec839e610bb0e2176c7" &&
    legacy.fileSha256 ===
      "9300a4810b773ab7e0352f3a5729e6afe3a315a16d73faed43338247ac106d44" &&
    legacy.canonicalDigest ===
      "7a4c85306f4c272070dd5fcb0a812a223eb904fbf07806503ce276a644004c3c" &&
    JSON.stringify(legacy.fields) === JSON.stringify(expectedLegacyFields) &&
    objectDigest(legacy) ===
      "31b40499e2b33833424f75f93dd07b0c87b8b0255b897fff24e1dec16c2f1688",
  "CodeBuddy legacy v1 identity changed",
);
safeRelative(legacy.path, "CodeBuddy legacy v1 path");
assert(
  gitTree(repoRoot, legacy.targetCommit) === legacy.targetTree,
  "CodeBuddy legacy v1 target tree drifted",
);
const legacyRaw = gitFile(repoRoot, legacy.targetCommit, legacy.path, null);
assert(sha256(legacyRaw) === legacy.fileSha256, "CodeBuddy legacy v1 file drifted");
const legacyContract = JSON.parse(legacyRaw.toString("utf8"));
assert(
  legacyContract.schemaVersion === 1 &&
    JSON.stringify(Object.keys(legacyContract)) === JSON.stringify(expectedLegacyFields),
  "CodeBuddy committed legacy v1 field set changed",
);
const reconstructedLegacy = {};
for (const field of expectedLegacyFields) {
  const legacyValue = legacyContract[field];
  const currentValue = field === "schemaVersion" ? contract.legacySchemaVersion : contract[field];
  assert(
    digestPattern.test(legacy.fieldDigests?.[field]) &&
      objectDigest(legacyValue) === legacy.fieldDigests[field] &&
      objectDigest(currentValue) === legacy.fieldDigests[field],
    `CodeBuddy legacy ${field} history changed`,
  );
  reconstructedLegacy[field] = currentValue;
}
assert(
  objectDigest(reconstructedLegacy) === legacy.canonicalDigest &&
    objectDigest(
      Object.fromEntries(expectedLegacyFields.map((field) => [field, legacyContract[field]])),
    ) === legacy.canonicalDigest,
  "CodeBuddy legacy v1 canonical digest changed",
);

const sourceDeltas = new Set();
for (const source of contract.sources ?? []) {
  assert(/^[a-f0-9]{40}$/.test(source.commit), `${source.id} has an invalid commit`);
  const root = path.resolve(repoRoot, process.env[source.rootEnv] || source.defaultRelativeRoot);
  for (const file of source.files ?? []) {
    safeRelative(file.path, `${source.id} evidence path`);
    assert(/^[a-f0-9]{64}$/.test(file.sha256), `${source.id}:${file.path} has an invalid hash`);
    if (checkSources) {
      const content = execFileSync("git", ["-C", root, "show", `${source.commit}:${file.path}`], {
        encoding: null,
        maxBuffer: 32 * 1024 * 1024,
      });
      assert(sha256(content) === file.sha256, `${source.id}:${file.path} source hash drifted`);
    }
  }
  for (const delta of source.deltas ?? []) {
    assert(delta.id && !sourceDeltas.has(delta.id), `${source.id} has a duplicate delta id`);
    sourceDeltas.add(delta.id);
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
        `${source.id}:${delta.id}:${file.path} has an invalid hash`,
      );
      if (checkSources) {
        const content = execFileSync(
          "git",
          ["-C", root, "show", `${file.commit}:${file.path}`],
          { encoding: null, maxBuffer: 32 * 1024 * 1024 },
        );
        assert(
          sha256(content) === file.sha256,
          `${source.id}:${delta.id}:${file.path} source hash drifted`,
        );
      }
    }
  }
}
assert(
  contract.sources?.find((source) => source.id === "cli2api-current-review")?.commit ===
    "624874a0331f5e8f012ef104b633e69826487458",
  "CodeBuddy current cli2api review commit changed",
);
assert(
  JSON.stringify([...sourceDeltas].sort()) ===
    JSON.stringify([
      "deepseek_v4_1_flash",
      "interrupted_tool_round_11148",
      "namespace_and_reasoning_history",
      "stream_cancellation_fixture",
    ]),
  "CodeBuddy incremental source delta set changed",
);

assert(
  objectDigest(contract.sourceExtensions) ===
    "a5e9294c1ef1f4cfdbac046c774f395032c698bdf244211e091757c14be64ba2",
  "CodeBuddy source extension history changed",
);
const sourceById = new Map();
const sourceRootById = new Map();
const sourceFileById = new Map();
for (const source of [...(contract.sources ?? []), ...(contract.sourceExtensions ?? [])]) {
  assert(
    source.id && source.rootEnv && source.defaultRelativeRoot,
    "CodeBuddy source metadata is incomplete",
  );
  assert(!sourceById.has(source.id), `duplicate CodeBuddy source ${source.id}`);
  assert(commitPattern.test(source.commit), `${source.id} has an invalid commit`);
  const sourceRoot = path.resolve(
    repoRoot,
    process.env[source.rootEnv] || source.defaultRelativeRoot,
  );
  const fileByPath = new Map();
  for (const file of source.files ?? []) {
    safeRelative(file.path, `${source.id} evidence path`);
    assert(!fileByPath.has(file.path), `${source.id} repeats ${file.path}`);
    assert(digestPattern.test(file.sha256), `${source.id}:${file.path} has an invalid SHA-256`);
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
  sourceById.size === 4 && sourceById.has("cli2api-head"),
  "CodeBuddy source set changed",
);
const currentSource = sourceById.get("cli2api-head");
assert(
  currentSource.commit === "624874a0331f5e8f012ef104b633e69826487458" &&
    currentSource.extendsSourceId === "cli2api-current-review" &&
    currentSource.files.length === 15,
  "CodeBuddy current committed source history changed",
);

const immutableSourceSnapshotDigests = new Map([
  [
    "cli2api-2026-09-19",
    "a99ec47f17e63557ca58b24bda178c9185205c2678087d7ae337bec197862f8e",
  ],
]);
const sourceSnapshotById = new Map();
for (const snapshot of contract.sourceSnapshots ?? []) {
  assert(
    snapshot.id && !sourceSnapshotById.has(snapshot.id),
    "duplicate CodeBuddy source snapshot",
  );
  const source = sourceById.get(snapshot.sourceId);
  assert(source, `${snapshot.id} references an unknown source`);
  assert(
    snapshot.repository === "cli2api" &&
      snapshot.headCommit === source.commit &&
      snapshot.headTree === "c2f02a394e1381adf8291949822ce4b08a48dfb3" &&
      snapshot.worktreeClean === false &&
      snapshot.worktreeChangesExcluded === true &&
      snapshot.excludedWorktreeEntries === 2 &&
      JSON.stringify(snapshot.excludedWorktreePaths) ===
        JSON.stringify(["proxy.html", "proxy.md"]) &&
      snapshot.readMode === "read_only_committed_git_objects",
    `${snapshot.id} does not explicitly exclude the two untracked files`,
  );
  for (const excludedPath of snapshot.excludedWorktreePaths) {
    safeRelative(excludedPath, `${snapshot.id} excluded worktree path`);
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
  "CodeBuddy source snapshot history is incomplete",
);

assert(
  objectDigest(contract.reviewedRejections) ===
    "ea57687f37287122e5f77de55b31d26f7f2ee5c56b31df590a01b69b5e76ce69",
  "CodeBuddy reviewed rejection history changed",
);
const reviewedRejections = new Map(
  (contract.reviewedRejections ?? []).map((entry) => [entry.id, entry]),
);
assert(
  reviewedRejections.size === 3 &&
    reviewedRejections.get("CB-R1")?.status === "rejected" &&
    reviewedRejections
      .get("CB-R1")
      ?.behaviors.includes("account_pool_selection_or_rotation") &&
    reviewedRejections.get("CB-R2")?.status === "rejected" &&
    reviewedRejections
      .get("CB-R2")
      ?.behaviors.includes("commercial_api_key_management") &&
    reviewedRejections.get("CB-R3")?.status === "rejected" &&
    reviewedRejections
      .get("CB-R3")
      ?.behaviors.includes("missing_unique_terminal_or_eof_counts_as_success"),
  "CodeBuddy rejection boundaries changed",
);

const immutableObservationDigests = new Map([
  ["CB-OBS-0001", "5d0a3bfea659cd6e1ed37a3000302019768af5ba979b7900f9a2fe2a1f5485c6"],
  ["CB-OBS-0002", "ff5146e40985375cac8a9bb42eeb67e10f52c4faacba4b09de05b13efc1f74da"],
  ["CB-OBS-0003", "c84817124cfd5a617e6add3ddbbfcbc574a0b7e1a07549cc251835190d0cbbe3"],
  ["CB-OBS-0004", "cb8f203765b65152c4de4d390a225ebbc85f48d1e732e780b5a2bbf5e4b59c90"],
  ["CB-OBS-0005", "36aad47d45796959ace2925b7dd9147586bbcb47d01b7071f9d50f9675810521"],
  ["CB-OBS-0006", "9bb887156e070841d30bcacbe6233d0956a2ab71f9b4559bfdb71738f2b14a34"],
  ["CB-OBS-0007", "2b2cf5c5cd8c7f803c9a157be02e1a2cf16eae9eef9d7d720d6300bf0632a12a"],
  ["CB-OBS-0008", "77220a9c1705b21607745e26b583501f429bfdc368206051848e5b098a15b49d"],
  ["CB-OBS-0009", "1c5a8fb2f508380e6c70497f33e217ecbb371564d013bb4d224dfc8a289ad142"],
  ["CB-OBS-0010", "fe285b03112a9347a9d4a9678cfebc0a363ebeab8d9ef4b8ac8e4f8fe85dbd5e"],
  ["CB-OBS-0011", "b6018b1092efc00e6cc5744240f668ac1cfb8e1d68f4fc5cc842ade096c25bef"],
  ["CB-OBS-0012", "e358f8903a5a7ee7f7f34b73381cea3d545d20e49cacc59e7a096ae30cd843ae"],
  ["CB-OBS-0013", "a7c5bdcf34082c3b2a1e24d7cfe21d5186931ac53a54f40c9cb071f81189e02a"],
  ["CB-OBS-0014", "d93a801d31e2be1cebab486a77dbd676b0ef83456f8a3b49bc32073dd0d4020f"],
]);
const expectedEnhancementIds = new Set([
  "CB-01",
  "CB-02",
  "CB-03",
  "CB-04",
  "CB-N1",
  "CB-N2",
  "CB-N3",
  "CB-N4",
  "CB-N5",
  "CORE-N1",
  "LIVE-N1",
  "CB-R1",
  "CB-R2",
  "CB-R3",
]);
const expectedDispositions = new Map([
  ["CB-OBS-0001", "differential"],
  ["CB-OBS-0002", "differential"],
  ["CB-OBS-0003", "differential"],
  ["CB-OBS-0004", "live_gate"],
  ["CB-OBS-0005", "differential"],
  ["CB-OBS-0006", "differential"],
  ["CB-OBS-0007", "differential"],
  ["CB-OBS-0008", "live_gate"],
  ["CB-OBS-0009", "differential"],
  ["CB-OBS-0010", "differential"],
  ["CB-OBS-0011", "live_gate"],
  ["CB-OBS-0012", "reject"],
  ["CB-OBS-0013", "reject"],
  ["CB-OBS-0014", "reject"],
]);
const observationIds = new Set();
const observedEnhancementIds = new Set();
const observedDeltaIds = new Set();
for (const observation of contract.observations ?? []) {
  assert(
    observation.id && !observationIds.has(observation.id),
    "duplicate CodeBuddy observation",
  );
  observationIds.add(observation.id);
  assert(
    observation.providerFamily === "codebuddy",
    `${observation.id} changed provider family`,
  );
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
      observation.enhancementIds.length === 1 &&
      expectedEnhancementIds.has(observation.enhancementIds[0]),
    `${observation.id} has an invalid enhancement mapping`,
  );
  observedEnhancementIds.add(observation.enhancementIds[0]);

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
  "CodeBuddy observation history is incomplete",
);
assert(
  JSON.stringify([...observedEnhancementIds].sort()) ===
    JSON.stringify([...expectedEnhancementIds].sort()),
  "CodeBuddy enhancement observation coverage is incomplete",
);

const capabilities = new Map(contract.capabilities.map((item) => [item.id, item]));
assert(capabilities.size === 4, "CodeBuddy contract must contain CB-01 through CB-04");
for (let index = 1; index <= 4; index += 1) {
  const id = `CB-0${index}`;
  const capability = capabilities.get(id);
  assert(capability, `CodeBuddy contract is missing ${id}`);
  assert(
    capability.status === (id === "CB-04" ? "live_pending" : "fixture_verified"),
    `${id} evidence state changed`,
  );
  safeRelative(capability.path, `${id} local path`);
  const local = fs.readFileSync(path.join(repoRoot, capability.path), "utf8");
  for (const anchor of capability.anchors ?? []) {
    assert(local.includes(anchor), `${id} is missing local anchor ${anchor}`);
  }
}

const enhancements = new Map(
  (contract.incrementalEnhancements ?? []).map((item) => [item.id, item]),
);
assert(enhancements.size === 5, "CodeBuddy contract must contain CB-N1 through CB-N5");
for (let index = 1; index <= 5; index += 1) {
  const id = `CB-N${index}`;
  const enhancement = enhancements.get(id);
  assert(enhancement, `CodeBuddy contract is missing ${id}`);
  assert(
    sourceDeltas.has(enhancement.referenceDelta),
    `${id} references an unknown external delta`,
  );
  for (const evidence of enhancement.evidence ?? []) {
    safeRelative(evidence.path, `${id} local path`);
    const local = fs.readFileSync(path.join(repoRoot, evidence.path), "utf8");
    for (const anchor of evidence.anchors ?? []) {
      assert(local.includes(anchor), `${id} is missing local anchor ${anchor}`);
    }
  }
}
const toolRepair = enhancements.get("CB-N1");
assert(
  toolRepair.status === "fixture_verified" &&
    toolRepair.fabricatesToolResults === false &&
    toolRepair.validatesHistoricalArgumentsJson === true,
  "CB-N1 tool-round repair boundary changed",
);
const reasoningHistory = enhancements.get("CB-N2");
assert(
  reasoningHistory.status === "fixture_verified" &&
    reasoningHistory.upstreamAssistantReasoningField === "reasoning",
  "CB-N2 reasoning-history contract changed",
);
const namespaceTools = enhancements.get("CB-N3");
assert(
  namespaceTools.status === "fixture_verified_existing_shared_bridge" &&
    namespaceTools.bareChildFallback === false,
  "CB-N3 namespace contract changed",
);
const deepseek = enhancements.get("CB-N4");
assert(
  deepseek.status === "live_pending" &&
    deepseek.runtimeEnabled === false &&
    deepseek.receipt === null,
  "CB-N4 Deepseek gate opened without live evidence",
);
const cancellation = enhancements.get("CB-N5");
assert(
  cancellation.status === "fixture_verified_existing_shared_guard" &&
    cancellation.providerBranchAdded === false,
  "CB-N5 cancellation contract changed",
);

const fixtureChecks = contract.fixtureAcceptance?.checks ?? [];
assert(
  fixtureChecks.length === 12 && new Set(fixtureChecks).size === 12,
  "CodeBuddy fixture acceptance matrix changed",
);

const lifecycle = contract.providerLifecycle;
assert(
  lifecycle?.id === "CORE-N1" &&
    lifecycle.status === "fixture_verified" &&
    lifecycle.wireChanged === false &&
    lifecycle.bindingChanged === false &&
    lifecycle.attemptBudgetChanged === false &&
    lifecycle.localEvidence?.length === 2,
  "CodeBuddy Provider lifecycle boundary changed",
);
for (const evidence of lifecycle.localEvidence) {
  safeRelative(evidence.path, "CORE-N1 local path");
  const localPath = path.join(repoRoot, evidence.path);
  assert(fs.existsSync(localPath), `CORE-N1 local path is unavailable: ${evidence.path}`);
  const local = fs.readFileSync(localPath, "utf8");
  for (const anchor of evidence.anchors ?? []) {
    assert(local.includes(anchor), `CORE-N1 is missing local anchor ${anchor}`);
  }
}

const registry = JSON.parse(
  fs.readFileSync(path.join(repoRoot, "assets/contract/provider-registry.json"), "utf8"),
);
const truth = contract.registryTruth;
const driver = registry.drivers.find((item) => item.driverId === truth.driverId);
assert(driver, "CodeBuddy driver is absent from the registry");
assert(driver.driverContractRevision === 2, "CodeBuddy driver revision must be 2");
assert(
  JSON.stringify(driver.operations) === JSON.stringify(truth.operations),
  "CodeBuddy operation support drifted",
);
const conformance = registry.conformance.find((item) => item.driverId === truth.driverId);
for (const [operation, state] of Object.entries(truth.conformance)) {
  assert(conformance?.[operation] === state, `CodeBuddy ${operation} conformance drifted`);
}
const profiles = registry.profiles.filter(
  (profile) => profile.compatibilityProviderType === "codebuddy_oauth",
);
assert(profiles.length === 3, "CodeBuddy requires exactly three app profiles");
assert(
  profiles.every(
    (profile) =>
      profile.driverBinding?.driverId === truth.driverId &&
      profile.credentialPolicy?.accountProviderType === "codebuddy_oauth",
  ),
  "CodeBuddy profiles lost managed-account ownership",
);

const acceptance = contract.realAcceptance;
assert(
  acceptance?.receiptSchemaVersion === 2 &&
    acceptance.harnessRevision === 2 &&
    acceptance.requiredChecks.length === 16 &&
    new Set(acceptance.requiredChecks).size === 16,
  "CodeBuddy acceptance must retain 16 unique checks",
);
for (const [field, count] of [
  ["scopeBindings", 8],
  ["requiredBodyHashes", 6],
  ["requiredMeasurements", 12],
  ["requiredDecisions", 15],
]) {
  assert(
    acceptance[field]?.length === count && new Set(acceptance[field]).size === count,
    `CodeBuddy acceptance ${field} changed`,
  );
}
const sites = new Map(acceptance.sites.map((site) => [site.site, site]));
for (const site of ["intl", "cn"]) {
  assert(
    sites.get(site)?.status === "live_pending" && sites.get(site)?.receipt === null,
    `${site} improperly claims live evidence`,
  );
}

for (const [relativePath, anchors] of [
  [
    "scripts/smoke/codebuddy-real-receipt.mjs",
    [
      "const HARNESS_REVISION = 2",
      "codebuddy_live_model_catalog",
      "sameAccountFirst401",
      "const expectedLive = fixtureMode ? \"live_pending\" : \"live_verified\"",
    ],
  ],
  [
    "scripts/audit/codebuddy-real-receipt.test.mjs",
    [
      "CodeBuddy Intl and CN fixture receipts stay contract-only",
      "CodeBuddy receipt scope binds all six body hashes",
      "CodeBuddy Provider binding mismatch fails before receipt acceptance",
    ],
  ],
]) {
  const local = fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
  for (const anchor of anchors) {
    assert(local.includes(anchor), `${relativePath} is missing ${anchor}`);
  }
}

console.log(
  `codebuddy reference delta audit ok (${expectedLegacyFields.length} legacy fields, ${sourceSnapshotById.size} dirty-worktree-excluded snapshot, ${observationIds.size} immutable observations, ${reviewedRejections.size} rejection boundaries, ${capabilities.size} capabilities, ${enhancements.size} enhancements, ${acceptance.requiredChecks.length} real checks, receipt v${acceptance.receiptSchemaVersion}${
    checkSources ? ", external objects verified" : ", external check optional"
  })`,
);
