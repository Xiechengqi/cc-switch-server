#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { execFileSync } from "node:child_process";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const baselinePath = path.join(
  repoRoot,
  "assets/contract/codex-reference-delta.json",
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

function assertSafePath(value, label) {
  assert(
    typeof value === "string" &&
      value.length > 0 &&
      !path.isAbsolute(value) &&
      !value.split("/").includes(".."),
    `${label} has an unsafe path`,
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

const baseline = JSON.parse(fs.readFileSync(baselinePath, "utf8"));
assert(
  baseline.format === "cc-switch-codex-reference-delta" &&
    baseline.schemaVersion === 2 &&
    baseline.legacySchemaVersion === 1,
  "Codex reference delta baseline format changed",
);
assert(
  Number.isFinite(Date.parse(baseline.capturedAt)) &&
    Number.isFinite(Date.parse(baseline.updatedAt)) &&
    Date.parse(baseline.updatedAt) >= Date.parse(baseline.capturedAt),
  "Codex evidence timestamps are invalid",
);
for (const invariant of [
  "no pool",
  "no rotation",
  "no credential-rail fallback",
  "no cross-account fallback",
  "no cross-Provider fallback",
]) {
  assert(
    baseline.policy?.scope?.includes(invariant),
    `Codex fixed-binding scope lost ${invariant}`,
  );
}
assert(
  baseline.policy?.externalSources ===
    "read_only_optional_audit_input_never_runtime_dependency" &&
    baseline.policy?.recoveryBoundary ===
      "same-account, pre-commit, shared total attempt budget" &&
    baseline.policy?.liveEvidence?.includes("do not imply live ChatGPT entitlement"),
  "Codex recovery, live-evidence, or external-source boundary changed",
);
assert(
  baseline.observationPolicy?.history?.includes("append_only") &&
    baseline.observationPolicy?.sourceReadMode === "read_only_committed_git_objects" &&
    baseline.observationPolicy?.targetReadMode === "committed_git_objects" &&
    baseline.observationPolicy?.externalVerification?.includes(
      "never a build, test, release, or runtime dependency",
    ),
  "Codex observation policy changed",
);

const immutableLegacyDigests = new Map([
  ["policy", "f23c3eab5ceb9ca04506af5b627f0646416e97e16f7ed6834af0a2ea8abf44fd"],
  ["sources", "41fb047a548c3f7ba4248c2bc2c6086e3d9d7dbc58f4df22f729e0cb8ef23967"],
  ["capabilities", "713ca68594d9f0a23802efc23d4a1a8c28879ddabfb8021558cc54fbd2c36ab1"],
  [
    "incrementalEnhancements",
    "cae24b61ceec0cd3b180ff224f17cb3c5e297e8560246797c2b841e217cf05d3",
  ],
  ["realAcceptance", "078a65ba2c43d8d3decc9f306d2154727c095a2471d99a37e0d5e5b9a9347026"],
  ["wireGoldens", "6783f006cd5b98eca793f47639fbd9cce0cad9e8e68115758ba6c0fa88f25beb"],
]);
for (const [field, digest] of immutableLegacyDigests) {
  assert(objectDigest(baseline[field]) === digest, `Codex legacy ${field} history changed`);
}

const sourceById = new Map();
const sourceRootById = new Map();
const sourceDeltaIds = new Set();
const sourceDeltaByKey = new Map();

function registerDelta(sourceId, delta) {
  assert(delta.id && !sourceDeltaIds.has(delta.id), `duplicate Codex delta ${delta.id}`);
  assert(
    Array.isArray(delta.commits) &&
      delta.commits.length > 0 &&
      new Set(delta.commits).size === delta.commits.length &&
      delta.commits.every((commit) => commitPattern.test(commit)),
    `${delta.id} has invalid commits`,
  );
  assert(
    Array.isArray(delta.files) && delta.files.length >= 2,
    `${delta.id} must pin implementation and fixture evidence`,
  );
  sourceDeltaIds.add(delta.id);
  sourceDeltaByKey.set(`${sourceId}/${delta.id}`, delta);
  for (const file of delta.files) {
    assert(
      delta.commits.includes(file.commit),
      `${delta.id}:${file.path} uses an undeclared commit`,
    );
    assertSafePath(file.path, `${delta.id}:${file.path ?? "<missing>"}`);
    assert(
      digestPattern.test(file.sha256),
      `${delta.id}:${file.path} has an invalid SHA-256`,
    );
    if (checkSources) {
      const content = gitFile(sourceRootById.get(sourceId), file.commit, file.path, null);
      assert(
        sha256(content) === file.sha256,
        `${sourceId}:${delta.id}:${file.path} drifted from the reviewed object`,
      );
    }
  }
}

for (const source of baseline.sources ?? []) {
  assert(
    source.id && source.rootEnv && source.defaultRelativeRoot,
    "Codex audit source metadata is incomplete",
  );
  assert(!sourceById.has(source.id), `duplicate Codex source ${source.id}`);
  const sourceRoot = path.resolve(
    repoRoot,
    process.env[source.rootEnv] || source.defaultRelativeRoot,
  );
  if (checkSources) {
    assert(fs.existsSync(sourceRoot), `${source.id} source root is unavailable`);
  }
  sourceById.set(source.id, source);
  sourceRootById.set(source.id, sourceRoot);
  for (const delta of source.deltas ?? []) registerDelta(source.id, delta);
}

for (const extension of baseline.sourceExtensions ?? []) {
  assert(sourceById.has(extension.sourceId), "Codex source extension has no legacy source");
  assert(
    Array.isArray(extension.deltas) && extension.deltas.length > 0,
    `${extension.sourceId} source extension has no deltas`,
  );
  for (const delta of extension.deltas) registerDelta(extension.sourceId, delta);
}

const immutableSourceSnapshotDigests = new Map([
  ["cliproxyapi-2026-09-19", "7482b03b5422de54996015f0ba9073435e9568bedb801faeabac0bd5fce42b2d"],
  ["codex2api-2026-09-19", "392793aa5484f5995e76d864053967e7a48a0993bcc0687951278e2a20c7ec42"],
]);
const sourceSnapshotById = new Map();
for (const snapshot of baseline.sourceSnapshots ?? []) {
  assert(snapshot.id && !sourceSnapshotById.has(snapshot.id), "duplicate Codex source snapshot");
  assert(sourceById.has(snapshot.sourceId), `${snapshot.id} references an unknown source`);
  assert(snapshot.repository, `${snapshot.id} has no repository name`);
  assert(
    commitPattern.test(snapshot.headCommit) && commitPattern.test(snapshot.headTree),
    `${snapshot.id} has an invalid HEAD commit or tree`,
  );
  assert(
    snapshot.readMode === "read_only_committed_git_objects" &&
      snapshot.worktreeClean === true &&
      snapshot.worktreeChangesExcluded === false,
    `${snapshot.id} does not declare a clean read-only source snapshot`,
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
  "Codex source snapshot history is incomplete",
);

const immutableObservationDigests = new Map([
  ["CX-OBS-0001", "32e6512ff62c59c6ea12063aa60265f3c07ea1f0984a4f1a5245ac9f34106de3"],
  ["CX-OBS-0002", "46c19e4bcbae733e2e4b5d3802b298106370048bf325e723abd654449541570f"],
  ["CX-OBS-0003", "3584318a5e6a311ec80eaf41825642a41a786a452ffe458d8c64c498870b9e02"],
  ["CX-OBS-0004", "fcc9fd8db6baad908a3950e29ab0d7134331ddb40453d7ff6b19b3d391ac17d8"],
  ["CX-OBS-0005", "a128edc58e0c89e8edcbcbc274e3d9c5de0a4fdef1eba4042c8c76679d826a07"],
  ["CX-OBS-0006", "ac4e101b28f3e269690caa7d8899f6ea09af7d0936e1caeafcfd729f2b27e1b7"],
  ["CX-OBS-0007", "2dcf10b7feda01a0477cb88fe3d6b738d41c2fafaba197bc1af93e78188db713"],
  ["CX-OBS-0008", "4f8d72813da963475133c5c391bb1735136945925b0801b44a059bfb044d7dee"],
  ["CX-OBS-0009", "839276010a09a0d6cfda7a696e9269b0eafca49fdcd48373e8e3cc510c29dcf8"],
  ["CX-OBS-0010", "e9aa9b35d0efb21dd9798e30260f7f7487c56b3f5b24e607ac5f427758b997d1"],
  ["CX-OBS-0011", "df783db25d767ed6cfb55138cdf227ada51b2fe8abc19e58ac46e0c80f8a85a9"],
  ["CX-OBS-0012", "10ae993de5900ca4c407c529a8d4fa52b87371f7b5dd81ad280d4c8ccf801a68"],
  ["CX-OBS-0013", "d83df5c1012b6538f174303dede9dfee026b9d530e8d3eee7c4c5d35c0c9ffc4"],
]);
const expectedEnhancementIds = new Set([
  "CX-01",
  "CX-02",
  "CX-03",
  "CX-04",
  "CX-05",
  "CX-06",
  "CX-N1",
  "CX-N2",
  "CX-N3",
  "CX-N4",
  "CX-R1",
  "CORE-N1",
  "CORE-N2",
  "LIVE-N1",
]);
const observedEnhancementIds = new Set();
const observedDeltaKeys = new Set();
const observationIds = new Set();
for (const observation of baseline.observations ?? []) {
  assert(observation.id && !observationIds.has(observation.id), "duplicate Codex observation");
  observationIds.add(observation.id);
  assert(observation.providerFamily === "codex", `${observation.id} changed provider family`);
  assert(Number.isFinite(Date.parse(observation.observedAt)), `${observation.id} has invalid observedAt`);
  assert(
    ["adopt", "differential", "live_gate", "reject"].includes(observation.disposition),
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
  const snapshot = sourceSnapshotById.get(reference.snapshotId);
  const deltaKey = `${reference.sourceId}/${reference.deltaId}`;
  const delta = sourceDeltaByKey.get(deltaKey);
  assert(
    snapshot && snapshot.sourceId === reference.sourceId,
    `${observation.id} has an invalid source snapshot`,
  );
  assert(snapshot.repository === reference.repository, `${observation.id} changed repository identity`);
  assert(delta, `${observation.id} references an unknown frozen source delta`);
  assert(
    Array.isArray(reference.objects) && reference.objects.length > 0,
    `${observation.id} has no committed source objects`,
  );
  const objectCommits = new Set();
  for (const object of reference.objects) {
    assert(
      commitPattern.test(object.commit) && commitPattern.test(object.tree),
      `${observation.id} has an invalid reference commit or tree`,
    );
    assert(!objectCommits.has(object.commit), `${observation.id} repeats a source commit`);
    objectCommits.add(object.commit);
    assert(
      Array.isArray(object.paths) && object.paths.length > 0,
      `${observation.id}:${object.commit} has no source paths`,
    );
    for (const sourcePath of object.paths) {
      assertSafePath(sourcePath, `${observation.id}:${sourcePath ?? "<missing>"}`);
    }
    assert(
      Array.isArray(object.symbols) &&
        object.symbols.length > 0 &&
        object.symbols.every((symbol) => typeof symbol === "string" && symbol.trim()),
      `${observation.id}:${object.commit} has no source symbols`,
    );
  }
  assert(
    JSON.stringify([...objectCommits].sort()) === JSON.stringify([...delta.commits].sort()),
    `${observation.id} source commits do not match the frozen delta`,
  );
  for (const file of delta.files) {
    assert(
      reference.objects.some(
        (object) => object.commit === file.commit && object.paths.includes(file.path),
      ),
      `${observation.id} omitted frozen source path ${file.commit}:${file.path}`,
    );
  }
  const expectedSourceDigest = sha256(
    JSON.stringify({
      sourceId: reference.sourceId,
      deltaId: reference.deltaId,
      commits: delta.commits,
      files: delta.files,
    }),
  );
  assert(
    digestPattern.test(reference.sourceDigest) &&
      reference.sourceDigest === expectedSourceDigest,
    `${observation.id} source digest drifted`,
  );
  observedDeltaKeys.add(deltaKey);

  const target = observation.target ?? {};
  assert(
    commitPattern.test(target.baselineCommit) && commitPattern.test(target.baselineTree),
    `${observation.id} has an invalid target baseline commit or tree`,
  );
  assert(
    gitTree(repoRoot, target.baselineCommit) === target.baselineTree,
    `${observation.id} target baseline tree drifted`,
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
        commitPattern.test(target.implementationTree),
      `${observation.id} has an invalid target implementation commit or tree`,
    );
    assert(
      target.baselineCommit !== target.implementationCommit &&
        gitTree(repoRoot, target.implementationCommit) === target.implementationTree,
      `${observation.id} target implementation tree drifted`,
    );
    assert(
      Array.isArray(target.fixtureIds) && target.fixtureIds.length > 0,
      `${observation.id} has no implementation fixtures`,
    );
  }
  const contractCommit = rejected ? target.baselineCommit : target.implementationCommit;
  const targetSources = [];
  for (const contract of target.contracts ?? []) {
    assertSafePath(contract.path, `${observation.id}:${contract.path ?? "<missing>"}`);
    const source = gitFile(repoRoot, contractCommit, contract.path);
    targetSources.push(source);
    assert(
      Array.isArray(contract.anchors) &&
        contract.anchors.length > 0 &&
        contract.anchors.every((anchor) => source.includes(anchor)),
      `${observation.id} has an unavailable committed target anchor`,
    );
  }
  assert(targetSources.length > 0, `${observation.id} has no target contracts`);
  if (!rejected) {
    assert(
      target.fixtureIds.every((fixture) =>
        targetSources.some((source) => source.includes(fixture)),
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
    for (const object of reference.objects) {
      assert(
        gitTree(sourceRoot, object.commit) === object.tree,
        `${observation.id}:${object.commit} source tree drifted`,
      );
      const sourceText = object.paths
        .map((sourcePath) => gitFile(sourceRoot, object.commit, sourcePath))
        .join("\n");
      assert(
        object.symbols.every((symbol) => sourceText.includes(symbol)),
        `${observation.id}:${object.commit} source symbol drifted`,
      );
    }
  }
}
assert(
  observationIds.size === immutableObservationDigests.size,
  "Codex observation history is incomplete",
);
assert(
  JSON.stringify([...observedEnhancementIds].sort()) ===
    JSON.stringify([...expectedEnhancementIds].sort()),
  "Codex enhancement observation coverage is incomplete",
);
assert(
  [...sourceDeltaByKey.keys()].every((key) => observedDeltaKeys.has(key)),
  "Codex frozen source deltas are not fully mapped to observations",
);

const capabilities = new Map(
  (baseline.capabilities ?? []).map((capability) => [capability.id, capability]),
);
assert(capabilities.size === 6, "Codex enhancement contract must contain CX-01 through CX-06");
for (let index = 1; index <= 6; index += 1) {
  const id = `CX-0${index}`;
  const capability = capabilities.get(id);
  assert(capability, `Codex enhancement contract is missing ${id}`);
  const localPath = path.resolve(repoRoot, capability.path);
  assert(
    localPath.startsWith(`${repoRoot}${path.sep}`) && fs.existsSync(localPath),
    `${id} local contract path is unavailable`,
  );
  const source = fs.readFileSync(localPath, "utf8");
  for (const anchor of capability.anchors ?? []) {
    assert(source.includes(anchor), `${id} is missing local anchor ${anchor}`);
  }
}
for (const id of ["CX-01", "CX-02", "CX-04"]) {
  assert(
    capabilities.get(id)?.status === "fixture_verified",
    `${id} must remain fixture_verified`,
  );
}
for (const id of ["CX-03", "CX-05", "CX-06"]) {
  const capability = capabilities.get(id);
  assert(capability?.status === "live_pending", `${id} must remain live_pending`);
  assert(capability.runtimeEnabled === false, `${id} cannot be enabled without evidence`);
}

const incrementalEnhancements = new Map(
  (baseline.incrementalEnhancements ?? []).map((enhancement) => [
    enhancement.id,
    enhancement,
  ]),
);
assert(
  incrementalEnhancements.size === 4,
  "Codex incremental contract must contain CX-N1 through CX-N4",
);
for (let index = 1; index <= 4; index += 1) {
  const id = `CX-N${index}`;
  const enhancement = incrementalEnhancements.get(id);
  assert(enhancement, `Codex incremental contract is missing ${id}`);
  assert(
    enhancement.status === "fixture_verified",
    `${id} must be fixture_verified without claiming live evidence`,
  );
  assert(
    sourceDeltaIds.has(enhancement.referenceDelta),
    `${id} references an unknown source delta`,
  );
  assert(
    Array.isArray(enhancement.evidence) && enhancement.evidence.length > 0,
    `${id} must pin local evidence`,
  );
  for (const evidence of enhancement.evidence) {
    const localPath = path.resolve(repoRoot, evidence.path);
    assert(
      localPath.startsWith(`${repoRoot}${path.sep}`) && fs.existsSync(localPath),
      `${id} local evidence path is unavailable`,
    );
    const source = fs.readFileSync(localPath, "utf8");
    for (const anchor of evidence.anchors ?? []) {
      assert(source.includes(anchor), `${id} is missing local anchor ${anchor}`);
    }
  }
}
assert(
  incrementalEnhancements.get("CX-N3")?.replayBoundary?.includes("same Account") &&
    incrementalEnhancements.get("CX-N3")?.replayBoundary?.includes("never replayed"),
  "CX-N3 lost its fixed-binding stale-socket replay boundary",
);
const memoryContract = incrementalEnhancements.get("CX-N4");
assert(
  memoryContract?.errorContract?.httpStatus === 503 &&
    memoryContract.errorContract.retryAfterSeconds === 1 &&
    memoryContract.errorContract.code === "cc_switch_request_memory_exhausted" &&
    memoryContract.errorContract.websocketTerminal === "error_then_close",
  "CX-N4 stable memory-exhaustion error contract changed",
);
for (const invariant of [
  "active upstream",
  "HTTP replay",
  "account fallback",
  "Provider fallback",
  "rail fallback",
  "site fallback",
]) {
  assert(
    memoryContract?.fallbackBoundary?.includes(invariant),
    `CX-N4 memory boundary lost ${invariant}`,
  );
}
assert(
  capabilities.get("CX-05").benchmarkEvidence === false &&
    capabilities.get("CX-05").upstreamReceipt === false,
  "CX-05 requires both benchmark and upstream receipt gates",
);
assert(
  capabilities.get("CX-06").dependsOn === "CX-03",
  "CX-06 must remain gated on GPT Image 2.5 evidence",
);

const imageVariants = capabilities.get("CX-03").variants ?? [];
assert(imageVariants.length === 3, "CX-03 must track exactly three image variants");
const expectedModels = [
  "gpt-image-2.5",
  "gpt-image-2.5-flare",
  "gpt-image-2.5-sunburst",
];
const evidenceFields = [
  "generation",
  "multipartEdit",
  "normalization",
  "sizeQuality",
  "usage",
  "error",
  "quotaCooldown",
];
for (const model of expectedModels) {
  const variant = imageVariants.find((candidate) => candidate.model === model);
  assert(variant, `CX-03 is missing independent gate ${model}`);
  assert(
    evidenceFields.every((field) => variant[field] === "live_pending"),
    `${model} improperly claims live image evidence`,
  );
}
const realAcceptance = baseline.realAcceptance;
assert(
  realAcceptance?.receiptSchemaVersion === 1 &&
    realAcceptance.harnessRevision === 1,
  "Codex real-acceptance receipt schema changed",
);
const acceptanceOperations = new Map(
  (realAcceptance.operations ?? []).map((operation) => [
    operation.operation,
    operation,
  ]),
);
assert(
  acceptanceOperations.size === 4,
  "Codex live gate must contain three image variants and WS prewarm",
);
const expectedOperations = new Map([
  ["gpt_image_2_5", "gpt-image-2.5"],
  ["gpt_image_2_5_flare", "gpt-image-2.5-flare"],
  ["gpt_image_2_5_sunburst", "gpt-image-2.5-sunburst"],
  ["ws_prewarm", null],
]);
for (const [operationId, expectedModel] of expectedOperations) {
  const operation = acceptanceOperations.get(operationId);
  assert(operation, `Codex real acceptance is missing ${operationId}`);
  assert(
    operation.model === expectedModel &&
      operation.status === "live_pending" &&
      operation.receipt === null,
    `${operationId} improperly claims live evidence`,
  );
  assert(
    Array.isArray(operation.requiredChecks) &&
      operation.requiredChecks.length >= 10 &&
      new Set(operation.requiredChecks).size === operation.requiredChecks.length &&
      operation.requiredChecks.includes("bound_account_provider_share") &&
      operation.requiredChecks.includes("decoy_zero_requests") &&
      operation.requiredChecks.includes("secret_scan"),
    `${operationId} has an incomplete or duplicate check set`,
  );
  assert(
    Array.isArray(operation.requiredBodyHashes) &&
      operation.requiredBodyHashes.length >= 5 &&
      new Set(operation.requiredBodyHashes).size === operation.requiredBodyHashes.length,
    `${operationId} has an incomplete body-hash set`,
  );
  assert(
    Array.isArray(operation.requiredMeasurements) &&
      operation.requiredMeasurements.length === 5 &&
      new Set(operation.requiredMeasurements).size === operation.requiredMeasurements.length,
    `${operationId} has an incomplete measurement set`,
  );
}
for (const operationId of [
  "gpt_image_2_5",
  "gpt_image_2_5_flare",
  "gpt_image_2_5_sunburst",
]) {
  const checks = acceptanceOperations.get(operationId).requiredChecks;
  for (const check of [
    "generation_nonstream_b64",
    "generation_stream_terminal",
    "multipart_edit_large_input",
    "responses_image_tool_sse",
    "responses_image_tool_json",
    "usage_present",
    "error_mapping_bounded",
    "quota_usage_limit_account_cooldown",
    "capacity_or_unknown_share_model_cooldown",
    "capability_restart_persistence",
    "shared_store_cross_replica",
    "cloudflare_stream_flush",
    "no_model_fallback",
    "no_post_commit_replay",
  ]) {
    assert(checks.includes(check), `${operationId} lost required check ${check}`);
  }
}
for (const check of [
  "upstream_prewarm_accepted",
  "benchmark_sample_set",
  "stable_ttfb_benefit",
  "same_session_connection_reuse",
  "pre_response_create_http_fallback",
  "post_response_create_no_replay",
  "shared_attempt_budget",
]) {
  assert(
    acceptanceOperations.get("ws_prewarm").requiredChecks.includes(check),
    `ws_prewarm lost required check ${check}`,
  );
}
for (const publishPath of [
  "assets/contract/provider-registry.json",
  "src/proxy/codex_models.rs",
]) {
  const source = fs.readFileSync(path.join(repoRoot, publishPath), "utf8");
  for (const model of expectedModels) {
    assert(!source.includes(model), `${model} was published by ${publishPath} before evidence`);
  }
}

const imageProbe = fs.readFileSync(
  path.join(repoRoot, "scripts/smoke/codex-images-real.mjs"),
  "utf8",
);
assert(
  imageProbe.includes('verificationState: "probe_only"') &&
    imageProbe.includes('liveState: "live_pending"') &&
    !imageProbe.includes("boundedText(Buffer.concat(chunks))") &&
    !imageProbe.includes("value.error?.message"),
  "Codex image probe may claim acceptance or expose raw upstream failures",
);
const receiptHarness = fs.readFileSync(
  path.join(repoRoot, "scripts/smoke/codex-real-receipt.mjs"),
  "utf8",
);
for (const operationId of expectedOperations.keys()) {
  assert(
    receiptHarness.includes(operationId),
    `Codex receipt harness is missing ${operationId}`,
  );
}
for (const boundary of [
  'verificationState: "blocked_inputs"',
  'liveState: "live_pending"',
  "receipt file must stay outside the repository",
  "receipt file must have mode 0600",
  "crossAccountFallback: \"disabled\"",
  "postCommitReplay: \"disabled\"",
]) {
  assert(
    receiptHarness.includes(boundary),
    `Codex receipt harness lost boundary ${boundary}`,
  );
}

const golden = baseline.wireGoldens;
assert(
  golden?.capacityFailure?.input?.sequence_number ===
    golden?.capacityFailure?.expected?.sequence_number,
  "Codex capacity golden lost sequence_number",
);
assert(
  golden?.capacityFailure?.input?.error?.details?.retry?.opaque?.length === 3 &&
    JSON.stringify(golden.capacityFailure.input.error.details) ===
      JSON.stringify(golden.capacityFailure.expected.error.details),
  "Codex capacity golden lost nested error details",
);
assert(
  golden?.successfulFrames?.at(0)?.type === "response.created" &&
    golden?.successfulFrames?.at(-1)?.type === "response.completed",
  "Codex first/last frame golden changed",
);
assert(
  golden?.namedToolOutputRequest?.input?.[0]?.name &&
    !golden.namedToolOutputRequest.input[0].call_id,
  "Codex named tool-output golden no longer covers missing call_id",
);
assert(
  golden?.websocketHttpFallback?.allowedOnlyBeforeResponseCreateSent === true &&
    golden.websocketHttpFallback.sameProvider === true &&
    golden.websocketHttpFallback.sameAccount === true &&
    golden.websocketHttpFallback.sharedAttemptBudget === true &&
    golden.websocketHttpFallback.postCommitReplay === false,
  "Codex WS to HTTP fallback boundary changed",
);

console.log(
  `codex reference delta audit ok (${sourceDeltaIds.size} deltas, ${observationIds.size} immutable observations, ${acceptanceOperations.size} live-pending operations${
    checkSources ? ", external objects verified" : ", external check optional"
  })`,
);
