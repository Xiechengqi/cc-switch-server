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
  contract.format === "cc-switch-cursor-reference-delta" &&
    contract.schemaVersion === 2 &&
    contract.legacySchemaVersion === 1,
  "Cursor reference delta format changed",
);
assert(
  Number.isFinite(Date.parse(contract.capturedAt)) &&
    Number.isFinite(Date.parse(contract.updatedAt)) &&
    Date.parse(contract.updatedAt) >= Date.parse(contract.capturedAt),
  "Cursor evidence timestamps are invalid",
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
assert(
  contract.observationPolicy?.history ===
    "append_only; every legacy schema v1 content field is immutable" &&
    contract.observationPolicy?.sourceReadMode === "read_only_committed_git_objects" &&
    contract.observationPolicy?.targetReadMode === "committed_git_objects" &&
    contract.observationPolicy?.externalVerification?.includes(
      "never a build, test, release, or runtime dependency",
    ),
  "Cursor observation policy changed",
);

const immutableLegacyDigests = new Map([
  ["capturedAt", "18aca4884d737a6e7c2c3ee61b8abd0b8d19239c34d2385637cd99769060390e"],
  ["policy", "c081934b7b7cf9445c6af548e0e4a752627041d2f1722e824d2f32915d3ade7f"],
  ["sources", "e3fdbd8f0fd001f2423b733de2f114a47e1bdb61179bf8936e09f2e4bb265891"],
  [
    "incrementalReview",
    "be61f22aef092e51373d2abace4b32608efecae219a5de0eb2078ea6cd552b08",
  ],
  [
    "registryTruth",
    "9e474b418412d1206b36a0cdb4923496e468f326b9e5fbd083f852ef06951406",
  ],
  [
    "capabilities",
    "d31d921ec16ed80f5f29e247acbc65a7d8c84f24416ff0e19924f1f79c687fd2",
  ],
  [
    "enhancements",
    "1a514afdf423c65b96501c356bd111e0164937d7299969d5d89cb31d74a211ee",
  ],
  [
    "providerLifecycle",
    "91ea0887a580b714305f20bf3bd6a78306d458d967fd0502322385ac45c11898",
  ],
  [
    "realAcceptance",
    "b4db1d12757a40a5b2934a6cc0eb10d1fb6ca235cb05caa60410a671b8c26cb7",
  ],
  [
    "protobufFixtures",
    "5091d5aebf02af15fb01ceabc7a6f90ac54069ec01e73a360e43c5780a580244",
  ],
]);
for (const [field, digest] of immutableLegacyDigests) {
  assert(objectDigest(contract[field]) === digest, `Cursor legacy ${field} history changed`);
}

const sourceById = new Map();
const sourceRootById = new Map();
const sourceFileById = new Map();
for (const source of contract.sources ?? []) {
  assert(
    source.id && source.rootEnv && source.defaultRelativeRoot,
    "Cursor source metadata is incomplete",
  );
  assert(!sourceById.has(source.id), `duplicate Cursor source ${source.id}`);
  assert(commitPattern.test(source.commit), `${source.id} has an invalid commit`);
  assert(
    source.previousReviewedCommit === "a3ca33fa6442b59adc42976c795709eaf5351109",
    `${source.id} previous review point changed`,
  );
  assert(
    Array.isArray(source.files) && source.files.length === 6,
    `${source.id} must retain six reviewed evidence files`,
  );
  const sourceRoot = path.resolve(
    repoRoot,
    process.env[source.rootEnv] || source.defaultRelativeRoot,
  );
  const fileByPath = new Map();
  for (const file of source.files) {
    safeRelative(file.path, `${source.id} evidence path`);
    assert(!fileByPath.has(file.path), `${source.id} repeats ${file.path}`);
    assert(
      digestPattern.test(file.sha256),
      `${source.id}:${file.path} has invalid SHA-256`,
    );
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
  sourceById.set(source.id, source);
  sourceRootById.set(source.id, sourceRoot);
  sourceFileById.set(source.id, fileByPath);
}
assert(sourceById.size === 1 && sourceById.has("omniroute"), "Cursor source set changed");

const immutableSourceSnapshotDigests = new Map([
  [
    "omniroute-2026-09-19",
    "821050b62be6a73e50f2d2c34272c3384cdd5fefdc1f64a418d32c891bc46638",
  ],
]);
const sourceSnapshotById = new Map();
for (const snapshot of contract.sourceSnapshots ?? []) {
  assert(snapshot.id && !sourceSnapshotById.has(snapshot.id), "duplicate Cursor source snapshot");
  const source = sourceById.get(snapshot.sourceId);
  assert(source, `${snapshot.id} references an unknown source`);
  assert(
    snapshot.repository === "OmniRoute" &&
      snapshot.headCommit === source.commit &&
      commitPattern.test(snapshot.headTree),
    `${snapshot.id} has invalid committed source identity`,
  );
  assert(
    snapshot.worktreeClean === false &&
      snapshot.worktreeChangesExcluded === true &&
      snapshot.excludedWorktreeEntries === 22 &&
      snapshot.readMode === "read_only_committed_git_objects",
    `${snapshot.id} does not explicitly exclude the dirty worktree`,
  );
  assert(
    immutableSourceSnapshotDigests.get(snapshot.id) === objectDigest(snapshot),
    `${snapshot.id} changed after it was recorded`,
  );
  if (checkSources) {
    assert(
      gitTree(sourceRootById.get(snapshot.sourceId), snapshot.headCommit) ===
        snapshot.headTree,
      `${snapshot.id} HEAD tree drifted`,
    );
  }
  sourceSnapshotById.set(snapshot.id, snapshot);
}
assert(
  sourceSnapshotById.size === immutableSourceSnapshotDigests.size,
  "Cursor source snapshot history is incomplete",
);

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

const immutableObservationDigests = new Map([
  ["CUR-OBS-0001", "f3732163de10712d240bae6265bc301a62cdf12be94500e837ee9381b3e17524"],
  ["CUR-OBS-0002", "84e95d79f94d721056617b00387e6c728bce903d30cb50f9444cf96ecc76cbe8"],
  ["CUR-OBS-0003", "0b28cfaa9d0a116e6a3c1a54d4bd9b176ab78700e0f561a806ed915fb942d97b"],
  ["CUR-OBS-0004", "b1597967c483d33cfd2f3bff23e5b4e0956420dc4e365ff8e7a8a077a0a85c5a"],
  ["CUR-OBS-0005", "2d4a13ae09ae145cd5325778d65815ddc4568867cac778cc9554b356e7077701"],
  ["CUR-OBS-0006", "0c781f29fc5adc7cb7a6d8939c75a3a670f41c7787278c01483955dbe34241f3"],
  ["CUR-OBS-0007", "d16160313b7b1349ceb9e7394d2e59a02d556290cf5231546afa62a4b7ff7f3f"],
]);
const expectedEnhancementIds = new Set([
  "CUR-01",
  "CUR-02",
  "CUR-03",
  "CUR-N1",
  "CUR-N2",
  "CORE-N1",
  "LIVE-N1",
]);
const observationIds = new Set();
const observedEnhancementIds = new Set();
const observedDeltaIds = new Set();
for (const observation of contract.observations ?? []) {
  assert(observation.id && !observationIds.has(observation.id), "duplicate Cursor observation");
  observationIds.add(observation.id);
  assert(observation.providerFamily === "cursor", `${observation.id} changed provider family`);
  assert(
    Number.isFinite(Date.parse(observation.observedAt)),
    `${observation.id} has an invalid observedAt`,
  );
  assert(
    ["adopt", "differential", "live_gate", "reject"].includes(
      observation.disposition,
    ),
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
      reference.commit === snapshot.headCommit &&
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
      reference.symbols.every((symbol) => typeof symbol === "string" && symbol.trim()),
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
  const contractCommit = rejected
    ? target.baselineCommit
    : target.implementationCommit;
  const targetSources = [];
  for (const localContract of target.contracts ?? []) {
    safeRelative(
      localContract.path,
      `${observation.id}:${localContract.path ?? "<missing>"}`,
    );
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
    assert(
      gitTree(sourceRootById.get(reference.sourceId), reference.commit) ===
        reference.tree,
      `${observation.id} source tree drifted`,
    );
    const sourceText = reference.paths
      .map((sourcePath) =>
        gitFile(sourceRootById.get(reference.sourceId), reference.commit, sourcePath),
      )
      .join("\n");
    assert(
      reference.symbols.every((symbol) => sourceText.includes(symbol)),
      `${observation.id} source symbol drifted`,
    );
  }
}
assert(
  observationIds.size === immutableObservationDigests.size,
  "Cursor observation history is incomplete",
);
assert(
  JSON.stringify([...observedEnhancementIds].sort()) ===
    JSON.stringify([...expectedEnhancementIds].sort()),
  "Cursor enhancement observation coverage is incomplete",
);

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

const lifecycle = contract.providerLifecycle;
assert(
  lifecycle?.id === "CORE-N1" &&
    lifecycle.status === "fixture_verified" &&
    lifecycle.wireChanged === false &&
    lifecycle.boundary?.includes("binding, lease, Share, attempt, terminal, and usage ownership remain unchanged"),
  "Cursor Provider lifecycle boundary changed",
);
safeRelative(lifecycle.path, "Cursor Provider lifecycle path");
const lifecycleSource = fs.readFileSync(path.join(repoRoot, lifecycle.path), "utf8");
for (const anchor of lifecycle.anchors ?? []) {
  assert(lifecycleSource.includes(anchor), `Cursor lifecycle is missing ${anchor}`);
}
const forwarderSource = fs.readFileSync(path.join(repoRoot, "src/proxy/forwarder.rs"), "utf8");
assert(
  forwarderSource.includes("cursor::prepare_agentservice_request") &&
    forwarderSource.includes("cursor::forward_agentservice") &&
    !forwarderSource.includes("cursor::h2_client::CursorH2Timeouts"),
  "Cursor shared-forwarder lifecycle facade drifted",
);
assert(
  fs
    .readFileSync(path.join(repoRoot, "src/proxy/providers/mod.rs"), "utf8")
    .includes("mod cursor;"),
  "Cursor Provider lifecycle module is not registered",
);

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
assert(
  acceptance?.receiptSchemaVersion === 2 && acceptance.harnessRevision === 2,
  "Cursor receipt schema or harness revision changed",
);
const expectedChecks = [
  "bound_account_provider_share",
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
  "second_401_terminal",
  "absolute_deadline",
  "identity_generation_drift",
  "malformed_connect_fail_closed",
  "terminal_usage",
  "decoy_zero_requests",
  "secret_scan",
];
const expectedBodyHashes = [
  "catalog",
  "claude_stream",
  "claude_non_stream",
  "codex_stream",
  "codex_non_stream",
  "gemini_stream",
  "gemini_non_stream",
  "declared_tool",
  "image_input",
  "park_resume",
];
const expectedMeasurements = [
  "surfaceRuns",
  "catalogRuns",
  "toolRuns",
  "imageRuns",
  "parkResumeRuns",
];
const expectedDecisions = [
  "sameIdentity401",
  "second401",
  "identityGenerationDrift",
  "crossRailFallback",
  "crossAccountFallback",
  "crossProviderFallback",
  "crossShareFallback",
  "postCommitReplay",
  "staleCatalogWireAuthorization",
];
assert(Array.isArray(acceptance.rails) && acceptance.rails.length === 2, "Cursor needs two rails");
const rails = new Map(acceptance.rails.map((rail) => [rail.rail, rail]));
assert(
  rails.get("oauth")?.credentialOwner === "managed_account" &&
    rails.get("api_key")?.credentialOwner === "provider_static_secret",
  "Cursor OAuth/API-key credential ownership changed",
);
for (const rail of rails.values()) {
  assert(
    rail.status === "live_pending" && rail.receipt === null,
    `${rail.rail} improperly claims live evidence`,
  );
  assert(
    JSON.stringify(rail.requiredChecks) === JSON.stringify(expectedChecks) &&
      new Set(rail.requiredChecks).size === expectedChecks.length,
    `${rail.rail} acceptance check set or order changed`,
  );
  assert(
    JSON.stringify(rail.requiredBodyHashes) === JSON.stringify(expectedBodyHashes) &&
      new Set(rail.requiredBodyHashes).size === expectedBodyHashes.length,
    `${rail.rail} body-hash set changed`,
  );
  assert(
    JSON.stringify(rail.requiredMeasurements) === JSON.stringify(expectedMeasurements) &&
      new Set(rail.requiredMeasurements).size === expectedMeasurements.length,
    `${rail.rail} measurement set changed`,
  );
  assert(
    JSON.stringify(rail.requiredDecisions) === JSON.stringify(expectedDecisions) &&
      new Set(rail.requiredDecisions).size === expectedDecisions.length,
    `${rail.rail} recovery-decision set changed`,
  );
}

const receiptHarness = fs.readFileSync(
  path.join(repoRoot, "scripts/smoke/cursor-real.mjs"),
  "utf8",
);
for (const boundary of [
  "HARNESS_REVISION = 2",
  'targetCommit !== targetCommit',
  "Cursor receipt file must have mode 0600",
  'crossRailFallback: "disabled"',
  'providerType: "cursor_apikey"',
  'verificationState: "blocked_inputs"',
  'liveState: "live_pending"',
]) {
  assert(receiptHarness.includes(boundary), `Cursor receipt harness lost ${boundary}`);
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
  `cursor reference delta audit ok (${immutableLegacyDigests.size} legacy fields, ${observationIds.size} immutable observations, ${expectedChecks.length} checks per rail, ${rails.size} live-pending rails${
    checkSources ? ", external objects verified" : ", external check optional"
  })`,
);
