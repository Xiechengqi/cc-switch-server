#!/usr/bin/env node

import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const contractPath = path.join(
  repoRoot,
  "assets/contract/qoder-reference-delta.json",
);
const registryPath = path.join(repoRoot, "assets/contract/provider-registry.json");
const coveragePath = path.join(repoRoot, "assets/contract/provider-coverage.json");
const oraclePath = path.join(repoRoot, "assets/contract/qoder-cli-oracle.json");
const checkSources = process.argv.includes("--check-sources");

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
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

function exact(actual, expected, label) {
  assert(
    JSON.stringify(actual) === JSON.stringify(expected),
    `${label} drifted from the reviewed Qoder contract`,
  );
}

const contract = readJson(contractPath);
assert(
  contract.format === "cc-switch-qoder-reference-delta" &&
    contract.schemaVersion === 2 &&
    contract.legacySchemaVersion === 1,
  "Qoder reference delta format changed",
);
assert(
  Number.isFinite(Date.parse(contract.capturedAt)) &&
    Number.isFinite(Date.parse(contract.updatedAt)) &&
    Date.parse(contract.updatedAt) >= Date.parse(contract.capturedAt),
  "Qoder evidence timestamps are invalid",
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
  "Qoder observation policy changed",
);
assert(
  contract.policy?.externalSources ===
    "read_only_optional_audit_input_never_runtime_dependency",
  "Qoder external-source boundary changed",
);
for (const invariant of [
  "single bound Provider/Share/Account",
  "same-account same-rail pre-commit recovery only",
  "no pool",
  "no rotation",
  "no credential fallback",
  "no cross-site fallback",
  "no cross-account fallback",
  "no cross-Provider fallback",
]) {
  assert(contract.policy?.scope?.includes(invariant), `Qoder scope lost ${invariant}`);
}
assert(
  contract.policy?.liveEvidence?.includes("independent real receipts") &&
    contract.policy.liveEvidence.includes("live_pending"),
  "Qoder fixture/live evidence boundary changed",
);

const expectedLegacyFields = [
  "format",
  "schemaVersion",
  "capturedAt",
  "policy",
  "sources",
  "incrementalReview",
  "contractMap",
  "capabilities",
  "enhancements",
  "upgradeWorkflow",
  "realAcceptance",
];
const legacy = contract.legacyV1;
assert(
  legacy?.path === "assets/contract/qoder-reference-delta.json" &&
    legacy.targetCommit === "2cc8a017dee0f5ca1ab4189ad87db35c391669ea" &&
    legacy.targetTree === "251c191c0ec4bcaec6ab1659b9a9e39ddafa8587" &&
    legacy.fileSha256 === "20fb3bcaa7e7b20543c567329b4f84dbcf52d0d4f18d6f88bc5f6bba1bdf6208" &&
    legacy.canonicalDigest === "72599461d33f31e448619b4b1efcc6b8767b8544e14e8534be11d20911fa121d" &&
    JSON.stringify(legacy.fields) === JSON.stringify(expectedLegacyFields) &&
    objectDigest(legacy) === "7a02132d38e9ede4e7d23cc9604e22943bb1f29850d68daad348f06124e0b353",
  "Qoder legacy v1 identity changed",
);
safeRelative(legacy.path, "Qoder legacy v1 path");
assert(
  gitTree(repoRoot, legacy.targetCommit) === legacy.targetTree,
  "Qoder legacy v1 target tree drifted",
);
const legacyRaw = gitFile(repoRoot, legacy.targetCommit, legacy.path, null);
assert(sha256(legacyRaw) === legacy.fileSha256, "Qoder legacy v1 file drifted");
const legacyContract = JSON.parse(legacyRaw.toString("utf8"));
assert(
  legacyContract.schemaVersion === 1 &&
    JSON.stringify(Object.keys(legacyContract)) === JSON.stringify(expectedLegacyFields),
  "Qoder committed legacy v1 field set changed",
);
const reconstructedLegacy = {};
for (const field of expectedLegacyFields) {
  const legacyValue = legacyContract[field];
  const currentValue = field === "schemaVersion" ? contract.legacySchemaVersion : contract[field];
  assert(
    digestPattern.test(legacy.fieldDigests?.[field]) &&
      objectDigest(legacyValue) === legacy.fieldDigests[field] &&
      objectDigest(currentValue) === legacy.fieldDigests[field],
    `Qoder legacy ${field} history changed`,
  );
  reconstructedLegacy[field] = currentValue;
}
assert(
  objectDigest(reconstructedLegacy) === legacy.canonicalDigest &&
    objectDigest(
      Object.fromEntries(expectedLegacyFields.map((field) => [field, legacyContract[field]])),
    ) === legacy.canonicalDigest,
  "Qoder legacy v1 canonical digest changed",
);

const sourceById = new Map();
const sourceRootById = new Map();
const sourceFileById = new Map();
for (const source of contract.sources ?? []) {
  assert(source.id && source.rootEnv, "Qoder source metadata is incomplete");
  assert(/^[a-f0-9]{40}$/.test(source.commit), `${source.id} has an invalid commit`);
  assert(
    source.previousReviewedCommit === "3488b4a9208c41e4f9db4108ef5133cb3710648c",
    `${source.id} previous review point changed`,
  );
  assert(Array.isArray(source.files) && source.files.length === 3, `${source.id} evidence files changed`);
  const sourceRoot = path.resolve(
    repoRoot,
    process.env[source.rootEnv] || source.defaultRelativeRoot,
  );
  const fileByPath = new Map();
  for (const file of source.files ?? []) {
    safeRelative(file.path, `${source.id} evidence path`);
    assert(!fileByPath.has(file.path), `${source.id} repeats ${file.path}`);
    assert(
      /^[a-f0-9]{64}$/.test(file.sha256),
      `${source.id}:${file.path} has an invalid SHA-256`,
    );
    fileByPath.set(file.path, file);
    if (checkSources) {
      const content = execFileSync(
        "git",
        ["-C", sourceRoot, "show", `${source.commit}:${file.path}`],
        { encoding: null, maxBuffer: 16 * 1024 * 1024 },
      );
      assert(
        sha256(content) === file.sha256,
        `${source.id}:${file.path} drifted from the reviewed Git object`,
      );
    }
  }
  assert(!sourceById.has(source.id), `duplicate Qoder source ${source.id}`);
  sourceById.set(source.id, source);
  sourceRootById.set(source.id, sourceRoot);
  sourceFileById.set(source.id, fileByPath);
}
assert(
  sourceById.size === 1 && sourceById.has("tokenrouter"),
  "Qoder source set changed",
);

const immutableSourceSnapshotDigests = new Map([
  [
    "tokenrouter-2026-09-19",
    "49f2e635416af98d3ee98721a78d2993c584f2e84b1a68250a199f5e4a6b5e37",
  ],
]);
const sourceSnapshotById = new Map();
for (const snapshot of contract.sourceSnapshots ?? []) {
  assert(snapshot.id && !sourceSnapshotById.has(snapshot.id), "duplicate Qoder source snapshot");
  const source = sourceById.get(snapshot.sourceId);
  assert(source, `${snapshot.id} references an unknown source`);
  assert(
    snapshot.repository === "TokenRouter" &&
      snapshot.headCommit === source.commit &&
      snapshot.headTree === "b205061c4f0d9b53fbffb0242f34796e10b18251" &&
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
  "Qoder source snapshot history is incomplete",
);

assert(
  objectDigest(contract.reviewedRejections) ===
    "ca8d2aaee28cb226e4a5405738bbdba900572563ec4792de0a608959f7456c25",
  "Qoder reviewed rejection history changed",
);
const reviewedRejections = new Map(
  (contract.reviewedRejections ?? []).map((entry) => [entry.id, entry]),
);
assert(
  reviewedRejections.size === 3 &&
    reviewedRejections.get("QD-R1")?.status === "rejected" &&
    reviewedRejections
      .get("QD-R1")
      ?.behaviors.includes("account_pool_selection_or_rotation") &&
    reviewedRejections.get("QD-R2")?.status === "rejected" &&
    reviewedRejections
      .get("QD-R2")
      ?.behaviors.includes("commercial_billing_or_key_management") &&
    reviewedRejections.get("QD-R3")?.status === "rejected" &&
    reviewedRejections
      .get("QD-R3")
      ?.behaviors.includes("missing_unique_terminal_or_eof_counts_as_success"),
  "Qoder rejection boundaries changed",
);

const immutableObservationDigests = new Map([
  ["QD-OBS-0001", "f2ab4f229db3e9dff86601493a78e713f6209e46842a68c30200d636285a4c06"],
  ["QD-OBS-0002", "cbe21e88fbd2073dba1d70265ee950f02c407c7aa061fbde62ada55a478d57dc"],
  ["QD-OBS-0003", "04aafe68d365841bbf2a2a3d044152d2e16f04ad686720d14544a4390cfa545f"],
  ["QD-OBS-0004", "8699b2584ff55fb390476f8c3f6ed76db7711e6d6490f30d491c7e93771f8e94"],
  ["QD-OBS-0005", "8ede2d039c7eda168a056a462c041ce2079fb52433214ae6bbe8c26811053eb9"],
  ["QD-OBS-0006", "c20e4eaa07852708414a06c26db96b5d280e15c02e1bb716fe810ef21a463376"],
  ["QD-OBS-0007", "11485b21da8b8be0effbe591c26e0b166f702482493fbf069e7fbbfea28d9a3f"],
  ["QD-OBS-0008", "284ff405962f0c2a8e6624deea71a6e0e12e0b4aad539a60bbd0ec971b02bd53"],
  ["QD-OBS-0009", "ecdd1ff9342e7c8633fc35214bacd4a77ce86253d0fbc0237b1bdc5dc224689c"],
  ["QD-OBS-0010", "218367eb612b3ca136829e900b95a127d96b9c163284aec79f2a1e0637e7bd31"],
]);
const expectedEnhancementIds = new Set([
  "QD-01",
  "QD-02",
  "QD-03",
  "QD-N1",
  "QD-N2",
  "CORE-N1",
  "LIVE-N1",
  "QD-R1",
  "QD-R2",
  "QD-R3",
]);
const expectedDispositions = new Map([
  ["QD-OBS-0001", "differential"],
  ["QD-OBS-0002", "live_gate"],
  ["QD-OBS-0003", "differential"],
  ["QD-OBS-0004", "live_gate"],
  ["QD-OBS-0005", "differential"],
  ["QD-OBS-0006", "adopt"],
  ["QD-OBS-0007", "live_gate"],
  ["QD-OBS-0008", "reject"],
  ["QD-OBS-0009", "reject"],
  ["QD-OBS-0010", "reject"],
]);
const observationIds = new Set();
const observedEnhancementIds = new Set();
const observedDeltaIds = new Set();
for (const observation of contract.observations ?? []) {
  assert(observation.id && !observationIds.has(observation.id), "duplicate Qoder observation");
  observationIds.add(observation.id);
  assert(observation.providerFamily === "qoder", `${observation.id} changed provider family`);
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
  "Qoder observation history is incomplete",
);
assert(
  JSON.stringify([...observedEnhancementIds].sort()) ===
    JSON.stringify([...expectedEnhancementIds].sort()),
  "Qoder enhancement observation coverage is incomplete",
);

assert(
  objectDigest(contract.evidenceExtensions) ===
    "75e3dc6d6553a51108ffb096d4f23926b7a57e6f49d2e704286acc7c61526133",
  "Qoder evidence extension history changed",
);
const evidenceExtensions = new Map(
  (contract.evidenceExtensions ?? []).map((entry) => [entry.id, entry]),
);
assert(
  evidenceExtensions.size === 2 &&
    evidenceExtensions.has("CORE-N1") &&
    evidenceExtensions.has("LIVE-N1"),
  "Qoder CORE/LIVE evidence extension set changed",
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
  "CORE-N1 changed Qoder wire, binding, or attempt budget",
);
assert(
  evidenceExtensions.get("LIVE-N1").status === "live_pending" &&
    evidenceExtensions.get("LIVE-N1").siteRailScoped === true &&
    evidenceExtensions.get("LIVE-N1").receiptCount === 3 &&
    evidenceExtensions.get("LIVE-N1").receiptsIndependent === true &&
    evidenceExtensions.get("LIVE-N1").livePromotionBlockedByUnobservedChecks === true,
  "LIVE-N1 receipt isolation or live gate changed",
);

const incrementalReview = contract.incrementalReview;
assert(incrementalReview?.id === "QD-N2", "Qoder incremental review id changed");
assert(
  incrementalReview.fromCommit === "3488b4a9208c41e4f9db4108ef5133cb3710648c" &&
    incrementalReview.toCommit === "7faf9469bc6957716923b5b4a98665c0fb9715e0",
  "Qoder incremental review range changed",
);
assert(
  incrementalReview.qoderWireDelta === "none" &&
    incrementalReview.observedChange === "shared_error_helper_signature_adaptation_only" &&
    incrementalReview.reviewedDiffSha256 ===
      "384062fd869020540b945f654179f250ad75650f3fb5427c7e355b710b824eb2" &&
    incrementalReview.oracleRemainsPrimary === true &&
    incrementalReview.productionCodeChanged === false,
  "QD-N2 no-wire-delta conclusion changed",
);
if (checkSources) {
  const source = contract.sources.find((candidate) => candidate.id === "tokenrouter");
  assert(source, "Qoder TokenRouter source is missing");
  const sourceRoot = path.resolve(
    repoRoot,
    process.env[source.rootEnv] || source.defaultRelativeRoot,
  );
  const reviewedDiff = execFileSync(
    "git",
    [
      "-C",
      sourceRoot,
      "diff",
      `${incrementalReview.fromCommit}..${incrementalReview.toCommit}`,
      "--",
      "backend/internal/handler/qoder_gateway_handler.go",
    ],
    { encoding: null, maxBuffer: 16 * 1024 * 1024 },
  );
  assert(
    sha256(reviewedDiff) === incrementalReview.reviewedDiffSha256,
    "Qoder reviewed helper-only diff changed",
  );
}

const enhancements = new Map(
  (contract.enhancements ?? []).map((enhancement) => [enhancement.id, enhancement]),
);
assert(enhancements.size === 2, "Qoder enhancement set changed");
assert(
  enhancements.get("QD-N1")?.status === "live_pending" &&
    JSON.stringify(enhancements.get("QD-N1")?.rails) ===
      JSON.stringify(["global_oauth", "global_pat", "cn_oauth"]) &&
    enhancements.get("QD-N1")?.receiptsIndependent === true,
  "QD-N1 three-rail live gate changed",
);
assert(
  enhancements.get("QD-N2")?.status === "reviewed_no_wire_delta" &&
    enhancements.get("QD-N2")?.oracleRemainsPrimary === true &&
    enhancements.get("QD-N2")?.productionCodeChanged === false,
  "QD-N2 oracle-first conclusion changed",
);

const capabilities = new Map(
  (contract.capabilities ?? []).map((capability) => [capability.id, capability]),
);
assert(capabilities.size === 3, "Qoder contract must contain QD-01 through QD-03");
for (const id of ["QD-01", "QD-02", "QD-03"]) {
  const capability = capabilities.get(id);
  assert(capability, `Qoder contract is missing ${id}`);
  assert(
    capability.status === (id === "QD-02" ? "live_pending" : "fixture_verified"),
    `${id} has an invalid evidence state`,
  );
  assert(Array.isArray(capability.evidence) && capability.evidence.length > 0, `${id} lacks evidence`);
  for (const evidence of capability.evidence) {
    safeRelative(evidence.path, `${id} local path`);
    const source = fs.readFileSync(path.join(repoRoot, evidence.path), "utf8");
    for (const anchor of evidence.anchors ?? []) {
      assert(source.includes(anchor), `${id} is missing ${evidence.path} anchor ${anchor}`);
    }
  }
}

const truth = contract.contractMap;
const registry = readJson(registryPath);
const driver = registry.drivers?.find((candidate) => candidate.driverId === truth.driverId);
assert(driver, "special.qoder_cosy is absent from the Provider registry");
assert(
  driver.driverContractRevision === truth.driverContractRevision &&
    truth.driverContractRevision === 2,
  "Qoder driver contract revision must be 2",
);
assert(driver.optionSchemaId === truth.optionSchemaId, "Qoder option schema drifted");
exact(driver.operations, truth.operations, "Qoder supported operations");
const conformance = registry.conformance?.find(
  (candidate) => candidate.driverId === truth.driverId,
);
assert(conformance, "Qoder conformance is absent from the Provider registry");
exact(
  Object.fromEntries(
    Object.keys(truth.conformance).map((operation) => [operation, conformance[operation]]),
  ),
  truth.conformance,
  "Qoder conformance",
);

const profiles = registry.profiles
  .filter((profile) => profile.compatibilityProviderType === truth.providerType)
  .sort((left, right) => left.profileId.localeCompare(right.profileId));
exact(
  profiles.map((profile) => profile.profileId),
  [...truth.profiles].sort(),
  "Qoder profile inventory",
);
for (const profile of profiles) {
  assert(
    profile.driverBinding?.driverId === truth.driverId &&
      profile.profileSchemaRevision === truth.profileSchemaRevision &&
      profile.credentialPolicy?.accountProviderType === truth.providerType,
    `${profile.profileId} drifted from the Qoder managed-account contract`,
  );
}

const oracle = readJson(oraclePath);
assert(oracle.providerType === truth.providerType, "Qoder oracle ProviderType drifted");
assert(oracle.schemaVersion === truth.oracle.schemaVersion, "Qoder oracle schema drifted");
assert(oracle.offlineState === truth.oracle.offlineState, "Qoder offline evidence drifted");
assert(oracle.liveState === truth.oracle.liveState, "Qoder live evidence drifted");
exact(
  oracle.rails.map((rail) => rail.id).sort(),
  truth.oracle.rails,
  "Qoder credential rails",
);

const coverage = readJson(coveragePath);
exact(coverage.verification?.qoder, oracle.verification, "Qoder coverage verification counts");
const coverageEntry = coverage.providerTypes?.find((entry) => entry.id === truth.providerType);
assert(coverageEntry?.presentInServer === true, "Qoder is absent from Provider coverage");
exact(
  [...coverageEntry.apps].sort(),
  profiles.map((profile) => profile.app).sort(),
  "Qoder Registry/coverage app mapping",
);

exact(
  contract.upgradeWorkflow?.orderedStages,
  [
    "freeze_package_name_version_integrity_and_bundle_digest",
    "extract_stable_wire_from_immutable_official_package",
    "cross_check_secondary_read_only_reference",
    "update_oracle_and_make_mutation_tests_fail_on_old_contract",
    "update_rust_after_oracle_review",
    "regenerate_contract_map_and_provider_coverage",
    "run_offline_and_loopback_gates",
    "keep_each_rail_live_pending_until_independent_receipt",
  ],
  "Qoder CLI upgrade stage order",
);
exact(
  contract.upgradeWorkflow?.forbidden,
  [
    "runtime_dependency_on_external_repository_or_npm_bundle",
    "fixture_and_rust_coherent_update_without_independent_digest",
    "cross_site_package_reuse",
    "one_rail_receipt_promotes_another_rail",
  ],
  "Qoder CLI upgrade forbidden set",
);

const acceptance = contract.realAcceptance;
assert(
  acceptance?.receiptSchemaVersion === 2 && acceptance.harnessRevision === 2,
  "Qoder real receipt schema/harness revision changed",
);
exact(
  acceptance.scopeBindings,
  [
    "target_commit",
    "credential_rail_and_site",
    "account_auth_and_token_generations",
    "three_surface_provider_revisions_and_runtime_digests",
    "share_revision_and_binding_digest",
    "exact_model",
    "three_fresh_catalog_snapshots",
    "six_request_body_hashes",
  ],
  "Qoder real receipt scope bindings",
);
assert(
  oracle.receiptSchema?.schemaVersion === acceptance.receiptSchemaVersion &&
    oracle.receiptSchema?.harnessRevision === acceptance.harnessRevision,
  "Qoder oracle/reference receipt revisions diverged",
);
assert(
  Array.isArray(acceptance?.requiredChecks) &&
    acceptance.requiredChecks.length === 14 &&
    new Set(acceptance.requiredChecks).size === 14,
  "Qoder real acceptance must retain 14 unique checks",
);
const liveRails = new Map((acceptance.rails ?? []).map((rail) => [rail.rail, rail]));
assert(liveRails.size === 3, "Qoder needs three independent live rails");
for (const [rail, site] of [
  ["cn_oauth", "cn"],
  ["global_oauth", "global"],
  ["global_pat", "global"],
]) {
  const entry = liveRails.get(rail);
  assert(
    entry?.site === site && entry.status === "live_pending" && entry.receipt === null,
    `${rail} improperly claims live evidence`,
  );
}

console.log(
  `qoder reference delta audit ok (${expectedLegacyFields.length} legacy fields, ${sourceSnapshotById.size} committed-object snapshot, ${observationIds.size} immutable observations, ${reviewedRejections.size} rejection boundaries, ${capabilities.size} capabilities, revision ${truth.driverContractRevision}, ${acceptance.requiredChecks.length} real checks, external check ${checkSources ? "verified" : "optional"})`,
);
