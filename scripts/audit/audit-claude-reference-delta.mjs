#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { execFileSync } from "node:child_process";

const root = path.resolve(new URL("../..", import.meta.url).pathname);
const baseline = JSON.parse(
  fs.readFileSync(path.join(root, "assets/contract/claude-reference-delta.json"), "utf8"),
);
const checkSources = process.argv.includes("--check-sources");
const assert = (condition, message) => {
  if (!condition) throw new Error(message);
};
const sha256 = (value) => crypto.createHash("sha256").update(value).digest("hex");
const canonical = (value) => {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, nested]) => [key, canonical(nested)]),
    );
  }
  return value;
};
const objectDigest = (value) => sha256(JSON.stringify(canonical(value)));
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

assert(
  baseline.format === "cc-switch-claude-reference-delta" &&
    baseline.schemaVersion === 2 &&
    baseline.legacySchemaVersion === 1,
  "Claude reference delta format changed",
);
assert(
  Number.isFinite(Date.parse(baseline.capturedAt)) &&
    Number.isFinite(Date.parse(baseline.updatedAt)) &&
    Date.parse(baseline.updatedAt) >= Date.parse(baseline.capturedAt),
  "Claude evidence timestamps are invalid",
);
assert(
  baseline.policy?.externalSources ===
    "read_only_optional_audit_input_never_runtime_dependency" &&
    baseline.policy?.scope?.includes("no pool") &&
    baseline.policy.scope.includes("no cross-account fallback") &&
    baseline.policy.scope.includes("no cross-Provider fallback") &&
    baseline.policy?.liveEvidence?.includes("do not imply live Anthropic acceptance"),
  "Claude fixed-binding or external-source boundary changed",
);
assert(
  baseline.observationPolicy?.history?.includes("append_only") &&
    baseline.observationPolicy?.sourceReadMode === "read_only_committed_git_objects" &&
    baseline.observationPolicy?.targetReadMode === "committed_git_objects" &&
    baseline.observationPolicy?.externalVerification?.includes(
      "never a build, test, release, or runtime dependency",
    ),
  "Claude observation policy changed",
);

const immutableLegacyDigests = new Map([
  ["sources", "ffce113a0e55746cae275ea5a99cfff2276dd0c8a166873ced22064137a16cd0"],
  [
    "localContracts",
    "685c1afe4ea1baa639b1acf5c408e8c615b4405bc2cd2f1fab2f623ddd514c57",
  ],
  [
    "realAcceptance",
    "8e598b64f81ef354c539758551914af7765ccecba3df7d965f0ef2e66fa6841f",
  ],
]);
for (const [field, digest] of immutableLegacyDigests) {
  assert(objectDigest(baseline[field]) === digest, `Claude legacy ${field} history changed`);
}

const sourceById = new Map();
const sourceRootById = new Map();
const sourceDeltaIds = new Set();
const legacySourceDeltaIds = new Set();
const sourceDeltaByKey = new Map();

function registerDelta(sourceId, delta, legacy) {
  assert(delta.id && !sourceDeltaIds.has(delta.id), `duplicate Claude delta ${delta.id}`);
  assert(commitPattern.test(delta.commit), `${delta.id} has an invalid commit`);
  assert(
    Array.isArray(delta.files) && delta.files.length >= 2,
    `${delta.id} must pin implementation and fixture evidence`,
  );
  sourceDeltaIds.add(delta.id);
  if (legacy) legacySourceDeltaIds.add(delta.id);
  sourceDeltaByKey.set(`${sourceId}/${delta.id}`, delta);
  for (const file of delta.files) {
    assertSafePath(file.path, `${delta.id}:${file.path ?? "<missing>"}`);
    assert(digestPattern.test(file.sha256), `${delta.id}:${file.path} has an invalid SHA-256`);
    if (checkSources) {
      const sourceRoot = sourceRootById.get(sourceId);
      const content = gitFile(sourceRoot, delta.commit, file.path, null);
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
    "Claude source metadata is incomplete",
  );
  assert(!sourceById.has(source.id), `duplicate Claude source ${source.id}`);
  const sourceRoot = path.resolve(
    root,
    process.env[source.rootEnv] || source.defaultRelativeRoot,
  );
  if (checkSources) assert(fs.existsSync(sourceRoot), `${source.id} source root is unavailable`);
  sourceById.set(source.id, source);
  sourceRootById.set(source.id, sourceRoot);
  for (const delta of source.deltas ?? []) registerDelta(source.id, delta, true);
}

for (const extension of baseline.sourceExtensions ?? []) {
  const source = sourceById.get(extension.sourceId);
  assert(source, `Claude source extension ${extension.sourceId} has no legacy source`);
  assert(
    Array.isArray(extension.deltas) && extension.deltas.length > 0,
    `${extension.sourceId} source extension has no deltas`,
  );
  for (const delta of extension.deltas) registerDelta(source.id, delta, false);
}

const immutableSourceSnapshotDigests = new Map([
  ["omniroute-2026-09-19", "8a8d5f55dd3a30919b8d65291c64f4fe449a596cf2dd88e63acf75b5387275c9"],
  [
    "cliproxyapi-2026-09-19",
    "7482b03b5422de54996015f0ba9073435e9568bedb801faeabac0bd5fce42b2d",
  ],
]);
const sourceSnapshotById = new Map();
for (const snapshot of baseline.sourceSnapshots ?? []) {
  assert(snapshot.id && !sourceSnapshotById.has(snapshot.id), "duplicate Claude source snapshot");
  assert(sourceById.has(snapshot.sourceId), `${snapshot.id} references an unknown source`);
  assert(snapshot.repository, `${snapshot.id} has no repository name`);
  assert(
    commitPattern.test(snapshot.headCommit) && commitPattern.test(snapshot.headTree),
    `${snapshot.id} has an invalid HEAD commit or tree`,
  );
  assert(
    snapshot.readMode === "read_only_committed_git_objects" &&
      ((snapshot.worktreeClean === true && snapshot.worktreeChangesExcluded === false) ||
        (snapshot.worktreeClean === false && snapshot.worktreeChangesExcluded === true)),
    `${snapshot.id} does not declare a safe read-only source snapshot`,
  );
  assert(
    immutableSourceSnapshotDigests.get(snapshot.id) === objectDigest(snapshot),
    `${snapshot.id} changed after it was recorded`,
  );
  if (checkSources) {
    const tree = gitTree(sourceRootById.get(snapshot.sourceId), snapshot.headCommit);
    assert(tree === snapshot.headTree, `${snapshot.id} HEAD tree drifted`);
  }
  sourceSnapshotById.set(snapshot.id, snapshot);
}
assert(
  sourceSnapshotById.size === immutableSourceSnapshotDigests.size,
  "Claude source snapshot history is incomplete",
);

const immutableObservationDigests = new Map([
  ["CL-OBS-0001", "98a2dea2e3c23a91e6255dbcba4be00f529b335b59bc44c06c11a9b229ab2cdd"],
  ["CL-OBS-0002", "1f4975945ad09aa8e4137906bb77028a9e27cf26663fc9dc35da8e3889c190fd"],
  ["CL-OBS-0003", "e467f1b2639c42ec5b47819b370d1db133bbd60ed8aac81d1482ec2ace9a85f8"],
  ["CL-OBS-0004", "1ace27dc9bf89bdbc42d13c4d0b7dff7b8f3257300f5dced588ce55e8d5ae002"],
  ["CL-OBS-0005", "d71e5907da90c2ff1fe9d66842ec1dc3e51e043159df4ea109e75fadb8eebdcd"],
  ["CL-OBS-0006", "959b9c050240e7c81516c073d998931a34bc0c554478399a35b8ed21c09e397f"],
  ["CL-OBS-0007", "854f555d72d6a4a07f15b95ade3a7e777b1b72404ed041b824897728739d125a"],
  ["CL-OBS-0008", "27906b96e3d32a853919f09b6786d2536510fe3c8da9ab0a3bf55f0333369be1"],
  ["CL-OBS-0009", "305059ba3be467c5ef115f5e3977fd9f1acb5de2f493c3d0ce6115d90a94f242"],
  ["CL-OBS-0010", "2613335bf98bbc2fd89dd6fe7385e6da27299d146be30567f230fa0af74c718e"],
  ["CL-OBS-0011", "78c248cf47d184f6fc8049308ade5a6e8024e6ef17794abaffd3f35537948b64"],
  ["CL-OBS-0012", "0ea15c51dea16ee523df0ed7b1f320b7405b41e16f9b56b30328f42ddf28965c"],
  ["CL-OBS-0013", "668063ac2a715cafb46980165278cb5b31277e1a48645186c9351da356e4af7f"],
  ["CL-OBS-0014", "f1e71a3c98544d255b560b9cf087f9b4bab0e486af777308d33d140bdd63443a"],
  ["CL-OBS-0015", "9af39e8156399d207a2fe39d2e7b7adbb556c3967149cff7e04f1c5870d5abf6"],
]);
const expectedEnhancementIds = new Set([
  "CL-01",
  "CL-02",
  "CL-03",
  "CL-04",
  "CL-05",
  "CL-06",
  "CL-N1",
  "CL-N2",
  "CL-N3",
  "CL-N4",
  "CL-R1",
  "CORE-N1",
  "CORE-N2",
  "LIVE-N1",
]);
const observedEnhancementIds = new Set();
const observedDeltaKeys = new Set();
const observationIds = new Set();
for (const observation of baseline.observations ?? []) {
  assert(observation.id && !observationIds.has(observation.id), "duplicate Claude observation");
  observationIds.add(observation.id);
  assert(observation.providerFamily === "claude", `${observation.id} changed provider family`);
  assert(Number.isFinite(Date.parse(observation.observedAt)), `${observation.id} has an invalid observedAt`);
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
  assert(snapshot && snapshot.sourceId === reference.sourceId, `${observation.id} has an invalid source snapshot`);
  assert(snapshot.repository === reference.repository, `${observation.id} changed repository identity`);
  assert(delta && delta.commit === reference.commit, `${observation.id} changed its frozen source delta`);
  assert(
    commitPattern.test(reference.commit) && commitPattern.test(reference.tree),
    `${observation.id} has an invalid reference commit or tree`,
  );
  assert(
    JSON.stringify(reference.paths) === JSON.stringify(delta.files.map((file) => file.path)),
    `${observation.id} source paths do not match the frozen delta`,
  );
  assert(
    Array.isArray(reference.symbols) &&
      reference.symbols.length > 0 &&
      reference.symbols.every((symbol) => typeof symbol === "string" && symbol.trim()),
    `${observation.id} has no source symbols`,
  );
  const expectedSourceDigest = sha256(
    JSON.stringify({
      sourceId: reference.sourceId,
      deltaId: reference.deltaId,
      commit: delta.commit,
      files: delta.files,
    }),
  );
  assert(
    digestPattern.test(reference.sourceDigest) && reference.sourceDigest === expectedSourceDigest,
    `${observation.id} source digest drifted`,
  );
  observedDeltaKeys.add(deltaKey);

  const target = observation.target ?? {};
  assert(
    commitPattern.test(target.baselineCommit) && commitPattern.test(target.baselineTree),
    `${observation.id} has an invalid target baseline commit or tree`,
  );
  assert(
    gitTree(root, target.baselineCommit) === target.baselineTree,
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
        gitTree(root, target.implementationCommit) === target.implementationTree,
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
    const source = gitFile(root, contractCommit, contract.path);
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
    assert(
      gitTree(sourceRoot, reference.commit) === reference.tree,
      `${observation.id} source tree drifted`,
    );
    const sourceText = reference.paths
      .map((sourcePath) => gitFile(sourceRoot, reference.commit, sourcePath))
      .join("\n");
    assert(
      reference.symbols.every((symbol) => sourceText.includes(symbol)),
      `${observation.id} source symbol drifted`,
    );
  }
}
assert(
  observationIds.size === immutableObservationDigests.size,
  "Claude observation history is incomplete",
);
assert(
  JSON.stringify([...observedEnhancementIds].sort()) ===
    JSON.stringify([...expectedEnhancementIds].sort()),
  "Claude enhancement observation coverage is incomplete",
);
assert(
  [...sourceDeltaByKey.keys()].every((key) => observedDeltaKeys.has(key)),
  "Claude frozen source deltas are not fully mapped to observations",
);

const expectedCapabilities = new Map([["CORE-N2-CLAUDE", "fixture_verified"]]);
for (const capability of baseline.capabilities ?? []) {
  assert(
    expectedCapabilities.get(capability.id) === capability.status,
    `${capability.id} has an unsupported evidence state`,
  );
  expectedCapabilities.delete(capability.id);
  assert(
    Array.isArray(capability.contracts) && capability.contracts.length > 0,
    `${capability.id} has no local contracts`,
  );
  for (const contract of capability.contracts) {
    assertSafePath(contract.path, `${capability.id}:${contract.path ?? "<missing>"}`);
    const localPath = path.resolve(root, contract.path);
    assert(
      localPath.startsWith(`${root}${path.sep}`) && fs.existsSync(localPath),
      `${capability.id} local contract path is unavailable`,
    );
    const source = fs.readFileSync(localPath, "utf8");
    assert(
      Array.isArray(contract.anchors) &&
        contract.anchors.length > 0 &&
        contract.anchors.every((anchor) => source.includes(anchor)),
      `${capability.id} has an unavailable local contract anchor`,
    );
  }
}
assert(expectedCapabilities.size === 0, "Claude capability coverage is incomplete");

const localIds = new Set();
for (const contract of baseline.localContracts ?? []) {
  assert(!localIds.has(contract.id), `duplicate local Claude contract ${contract.id}`);
  localIds.add(contract.id);
  assert(contract.status === "fixture_verified", `${contract.id} claims an unsupported evidence state`);
  assertSafePath(contract.path, `${contract.id}:${contract.path ?? "<missing>"}`);
  const localPath = path.resolve(root, contract.path);
  assert(
    localPath.startsWith(`${root}${path.sep}`) && fs.existsSync(localPath),
    `${contract.id} local fixture path is unavailable`,
  );
  const source = fs.readFileSync(localPath, "utf8");
  assert(
    Array.isArray(contract.anchors) &&
      contract.anchors.length > 0 &&
      contract.anchors.every((anchor) => source.includes(anchor)),
    `${contract.id} has an unavailable local fixture anchor`,
  );
}
assert(
  JSON.stringify([...legacySourceDeltaIds].sort()) === JSON.stringify([...localIds].sort()),
  "Claude legacy reference deltas and local fixture contracts do not match",
);

const expectedOperations = [
  {
    operation: "oauth_inference",
    requiredChecks: [
      "bound_account_provider_share",
      "count_tokens",
      "messages_nonstream_usage",
      "messages_stream_terminal",
      "tool_roundtrip",
      "caqs_opaque_replay",
      "initial_turn_billing_fingerprint",
      "standalone_tool_output",
      "same_account_first_401_recovery",
      "second_401_terminal",
      "rate_limit_scope_matrix",
      "terminal_then_late_disconnect",
      "decoy_zero_requests",
      "secret_scan",
    ],
    requiredBodyHashes: [
      "count_tokens",
      "messages_nonstream",
      "messages_stream",
      "tool_roundtrip",
    ],
  },
  {
    operation: "max_5x_plan",
    requiredChecks: [
      "bound_account_provider_share",
      "forced_quota_refresh",
      "canonical_max_5x",
      "fresh_plan_evidence",
      "plan_conflict_false",
      "decoy_zero_requests",
      "secret_scan",
    ],
    requiredBodyHashes: ["quota_projection"],
  },
  {
    operation: "max_20x_plan",
    requiredChecks: [
      "bound_account_provider_share",
      "forced_quota_refresh",
      "canonical_max_20x",
      "fresh_plan_evidence",
      "plan_conflict_false",
      "decoy_zero_requests",
      "secret_scan",
    ],
    requiredBodyHashes: ["quota_projection"],
  },
  {
    operation: "fable_5_1",
    requiredChecks: [
      "bound_account_provider_share",
      "fresh_max_20x_plan",
      "exact_fable_5_1_model",
      "messages_nonstream_usage",
      "messages_stream_terminal",
      "fable_7d_oi_scope",
      "no_model_fallback",
      "decoy_zero_requests",
      "secret_scan",
    ],
    requiredBodyHashes: ["messages_nonstream", "messages_stream"],
  },
];
const acceptance = baseline.realAcceptance;
assert(
  acceptance?.receiptSchemaVersion === 1 && acceptance?.harnessRevision === 1,
  "Claude real-acceptance receipt schema changed",
);
assert(
  typeof acceptance.scopeDigest === "string" &&
    acceptance.scopeDigest.includes("target commit") &&
    acceptance.scopeDigest.includes("Account generation") &&
    acceptance.scopeDigest.includes("Provider binding") &&
    acceptance.scopeDigest.includes("Share revision"),
  "Claude real-acceptance scope is incomplete",
);
assert(
  Array.isArray(acceptance.operations) &&
    acceptance.operations.length === expectedOperations.length,
  "Claude real-acceptance operations changed",
);
for (const expected of expectedOperations) {
  const operation = acceptance.operations.find(
    (candidate) => candidate.operation === expected.operation,
  );
  assert(operation, `Claude operation ${expected.operation} is missing`);
  assert(
    operation.status === "live_pending" && operation.receipt === null,
    `Claude operation ${expected.operation} claims unsupported live evidence`,
  );
  assert(
    JSON.stringify(operation.requiredChecks) === JSON.stringify(expected.requiredChecks) &&
      JSON.stringify(operation.requiredBodyHashes) ===
        JSON.stringify(expected.requiredBodyHashes),
    `Claude operation ${expected.operation} receipt contract changed`,
  );
}

const providerModule = fs.readFileSync(
  path.join(root, "src/proxy/providers/claude/mod.rs"),
  "utf8",
);
for (const anchor of [
  "record_quota_response_headers",
  "handle_rate_limit",
  "classify_rate_limit",
  "transport_replay_safe",
]) {
  assert(providerModule.includes(anchor), `Claude Provider module is missing ${anchor}`);
}
const forwarder = fs.readFileSync(path.join(root, "src/proxy/forwarder.rs"), "utf8");
for (const anchor of [
  "claude::record_quota_response_headers",
  "claude::handle_rate_limit",
  "claude::transport_replay_safe",
]) {
  assert(forwarder.includes(anchor), `Claude forwarder delegation is missing ${anchor}`);
}
const receiptHarness = fs.readFileSync(
  path.join(root, "scripts/smoke/claude-real-receipt.mjs"),
  "utf8",
);
for (const anchor of [
  "CC_SWITCH_CLAUDE_HARNESS_MODE",
  "contract_verified",
  "live_pending",
  "oauth.claude_messages",
  "providerBindingDigest",
]) {
  assert(receiptHarness.includes(anchor), `Claude receipt harness is missing ${anchor}`);
}
const legacyProbe = fs.readFileSync(
  path.join(root, "scripts/smoke/claude-oauth-real.mjs"),
  "utf8",
);
assert(
  legacyProbe.includes("Probe output is not live acceptance") &&
    !legacyProbe.includes("text.slice(0, 500)"),
  "Claude probe must stay redacted and subordinate to private receipts",
);

console.log(
  `claude reference delta audit ok (${sourceDeltaIds.size} deltas, ${observationIds.size} immutable observations, ${expectedOperations.length} live-pending operations${
    checkSources ? ", external objects verified" : ", external check optional"
  })`,
);
