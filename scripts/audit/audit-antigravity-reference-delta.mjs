#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { execFileSync } from "node:child_process";

const root = path.resolve(new URL("../..", import.meta.url).pathname);
const baseline = JSON.parse(
  fs.readFileSync(path.join(root, "assets/contract/antigravity-reference-delta.json"), "utf8"),
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

// Immutable observations describe their recorded Git objects. Current local
// capabilities and R4 contracts are checked separately against the worktree.
const targetObjectCache = new Map();
function targetGitObject(kind, commit, filePath) {
  const key = `${kind}:${commit}:${filePath ?? ""}`;
  if (!targetObjectCache.has(key)) {
    const args = kind === "tree"
      ? ["rev-parse", `${commit}^{tree}`]
      : ["show", `${commit}:${filePath}`];
    const content = execFileSync("git", ["-C", root, ...args], {
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
    });
    targetObjectCache.set(key, kind === "tree" ? content.trim() : content);
  }
  return targetObjectCache.get(key);
}

assert(
  baseline.format === "cc-switch-antigravity-reference-delta" &&
    baseline.schemaVersion === 2 &&
    baseline.legacySchemaVersion === 1,
  "Antigravity reference delta format changed",
);
assert(
  baseline.policy?.externalSources ===
    "read_only_optional_audit_input_never_runtime_dependency" &&
    baseline.policy?.scope?.includes("no cross-account fallback") &&
    baseline.policy?.scope?.includes("no cross-Provider fallback") &&
    baseline.policy?.recoveryBoundary?.includes("pre-commit"),
  "Antigravity fixed-binding or external-source boundary changed",
);

assert(
  baseline.observationPolicy?.history?.includes("append_only") &&
    baseline.observationPolicy?.sourceReadMode === "read_only_committed_git_objects" &&
    baseline.observationPolicy?.externalVerification?.includes("never a build or runtime dependency"),
  "Antigravity observation policy changed",
);

const sourceDeltaIds = new Set();
const sourceById = new Map();
const sourceDeltaByKey = new Map();
const sourceRootById = new Map();
for (const source of baseline.sources ?? []) {
  assert(source.id && source.rootEnv && source.defaultRelativeRoot, "Antigravity source metadata is incomplete");
  const sourceRoot = path.resolve(root, process.env[source.rootEnv] || source.defaultRelativeRoot);
  assert(!sourceById.has(source.id), `duplicate Antigravity source ${source.id}`);
  sourceById.set(source.id, source);
  sourceRootById.set(source.id, sourceRoot);
  for (const delta of source.deltas ?? []) {
    assert(!sourceDeltaIds.has(delta.id), `duplicate Antigravity delta ${delta.id}`);
    sourceDeltaIds.add(delta.id);
    sourceDeltaByKey.set(`${source.id}/${delta.id}`, delta);
    assert(commitPattern.test(delta.commit), `${delta.id} has an invalid commit`);
    assert(Array.isArray(delta.files) && delta.files.length > 0, `${delta.id} has no evidence files`);
    for (const file of delta.files) {
      assert(
        file.path && !path.isAbsolute(file.path) && !file.path.split("/").includes(".."),
        `${delta.id} has an unsafe path`,
      );
      assert(digestPattern.test(file.sha256), `${delta.id}:${file.path} has an invalid SHA-256`);
      if (checkSources) {
        assert(fs.existsSync(sourceRoot), `${source.id} source root is unavailable`);
        const content = execFileSync("git", ["-C", sourceRoot, "show", `${delta.commit}:${file.path}`], {
          encoding: null,
          maxBuffer: 64 * 1024 * 1024,
        });
        assert(sha256(content) === file.sha256, `${source.id}:${delta.id}:${file.path} drifted`);
      }
    }
  }
}

const immutableSourceSnapshotDigests = new Map([
  ["cliproxyapi-2026-09-18", "d4265b7924bb87c6b0963717128542fed0f04c997e35cfccdfdb912414c07d5c"],
  ["antigravity-manager-2026-09-18", "fd27d23c4b2c06b572c4ba57ff419cc2f7f0129f303b1eb226f3f8e80f335e19"],
  ["cliproxyapi-2026-09-28", "15e780d74d1498fe47743f2a6aeda0d1819d335e0a46a837217c126f59ed14e0"],
]);
const sourceSnapshotById = new Map();
for (const snapshot of baseline.sourceSnapshots ?? []) {
  assert(snapshot.id && !sourceSnapshotById.has(snapshot.id), "duplicate Antigravity source snapshot");
  assert(sourceById.has(snapshot.sourceId), `${snapshot.id} references an unknown source`);
  assert(snapshot.repository, `${snapshot.id} has no repository name`);
  assert(
    commitPattern.test(snapshot.headCommit) && commitPattern.test(snapshot.headTree),
    `${snapshot.id} has an invalid HEAD commit or tree`,
  );
  assert(
    snapshot.worktreeClean === true && snapshot.readMode === "read_only_committed_git_objects",
    `${snapshot.id} does not declare a clean read-only source snapshot`,
  );
  assert(
    immutableSourceSnapshotDigests.get(snapshot.id) === objectDigest(snapshot),
    `${snapshot.id} changed after it was recorded`,
  );
  sourceSnapshotById.set(snapshot.id, snapshot);
}
assert(
  sourceSnapshotById.size === immutableSourceSnapshotDigests.size,
  "Antigravity source snapshot history is incomplete",
);

const immutableObservationDigests = new Map([
  ["AG-OBS-0001", "f8fd170a566b57d02f9f24e594ec18ed2ae99c6a9d4e32086c7016e463c22932"],
  ["AG-OBS-0002", "0d30a04ccb4f670b7e967f614dd930da202049e9838527b24ccd102dfd0bb494"],
  ["AG-OBS-0003", "1a688d170391791a62dfd099818552691f9014b48330b74485b77af40aeaa72e"],
  ["AG-OBS-0004", "a32376662277652e254abd26f17add14ab796b9ab00766bc3d671df6fbf338b7"],
  ["AG-OBS-0005", "3dfda6caa1fc263219c07820ca2a483a6a9fea77a7e72412fda29d7a827c48e8"],
  ["AG-OBS-0006", "b0c6d1dd0a192f27c9f897a88e6993f1f12b8267a7f14875566ceda066aacdd4"],
  ["AG-OBS-0007", "036c6c6fe80db7f70daeb1f085423ca303fa1a09f7ca514c10b24773f15cb96c"],
  ["AG-OBS-0008", "c6875e54843be11c26d6eccadadb3a0320ee848440c5e6b30018b6d6aae87120"],
  ["AG-OBS-0009", "4db0475afd7c2ed3effd45b71e89b4e4c85a4d936dc0fd1bd27c00969b957173"],
  ["AG-OBS-0010", "445a67c58f4b7f4917e5db8226c3d5891d9a8bba311567e44e5f0594b35bf022"],
  ["AG-OBS-0011", "56fb9450198fb76b88adb86de868637cec2f26c662dbaa80df836d9209f40786"],
  ["AG-OBS-0012", "4adde8922de98605516f5965a6d3fc4d75350add1ebf4a9e3f404f0ff57a5731"],
  ["AG-OBS-0013", "7678ca793a6a2c29f04e7c980f384e669a25c43b4b4d4a645d0a6898eba6f70d"],
  ["AG-OBS-0014", "b669dea8772ecc66666b4bda7f2468111d1b852300357216943f689690c6cb82"],
  ["AG-OBS-0015", "664d7883c8005f7ea3285077125ca0fedf50c34ee51edc70f1e14e9b62788cec"],
  ["AG-OBS-0016", "fc8cb48e2fe0035a41a142937561b71a447bad933a5eeaf99aa3580314af8880"],
  ["AG-OBS-0017", "01c34d7e48885c8dfd99aae78094698408c7398b293748d4696bbd61fd96c646"],
  ["AG-OBS-0018", "1d2dc9748ef0f7df7eb57f599a256a7c89d3732adcbeb0b3ab6765691d493d79"],
  ["AG-OBS-0019", "2ff6d969016dca4cf7f6baccb29c055485cc84de8822894772f7003790927e53"],
  ["AG-OBS-0020", "c40e0ab9be522b350510059e3b3d506e03266578f8ca10e4f187d139f19851b6"],
  ["AG-OBS-0021", "bcdd8e99a5bb8c9c20b89870b5e50a57027b36d454e3a7a7a4c980334a94f2a4"],
  ["AG-OBS-0022", "59df4acb20f4d00fcf5dc393bd39f7b9cea359683d5551330d46e914aa0587ef"],
]);
const capabilityIds = new Set((baseline.capabilities ?? []).map((capability) => capability.id));
const observationIds = new Set();
for (const observation of baseline.observations ?? []) {
  assert(observation.id && !observationIds.has(observation.id), "duplicate Antigravity observation");
  observationIds.add(observation.id);
  assert(observation.providerFamily === "antigravity", `${observation.id} changed provider family`);
  assert(Number.isFinite(Date.parse(observation.observedAt)), `${observation.id} has an invalid observedAt`);
  assert(
    ["adopt", "differential", "live_gate", "reject"].includes(observation.disposition),
    `${observation.id} has an invalid disposition`,
  );
  assert(
    typeof observation.reason === "string" && observation.reason.trim().length > 0,
    `${observation.id} has no disposition reason`,
  );

  const reference = observation.reference ?? {};
  const snapshot = sourceSnapshotById.get(reference.snapshotId);
  const delta = sourceDeltaByKey.get(`${reference.sourceId}/${reference.deltaId}`);
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

  const mappedCapabilities = observation.capabilityIds ?? [];
  assert(
    mappedCapabilities.length > 0 && mappedCapabilities.every((id) => capabilityIds.has(id)),
    `${observation.id} has an invalid capability mapping`,
  );
  const target = observation.target ?? {};
  assert(
    commitPattern.test(target.baselineCommit) &&
      commitPattern.test(target.baselineTree) &&
      commitPattern.test(target.implementationCommit) &&
      commitPattern.test(target.implementationTree),
    `${observation.id} has an invalid target commit or tree`,
  );
  assert(target.baselineCommit !== target.implementationCommit, `${observation.id} has no target baseline delta`);
  assert(
    targetGitObject("tree", target.baselineCommit) === target.baselineTree &&
      targetGitObject("tree", target.implementationCommit) === target.implementationTree,
    `${observation.id} committed target tree drifted`,
  );
  const targetSources = [];
  for (const contract of target.contracts ?? []) {
    assert(
      typeof contract.path === "string" && contract.path &&
        !path.isAbsolute(contract.path) && !contract.path.split("/").includes(".."),
      `${observation.id} committed target path is unsafe`,
    );
    const source = targetGitObject("file", target.implementationCommit, contract.path);
    targetSources.push(source);
    assert(
      Array.isArray(contract.anchors) &&
        contract.anchors.length > 0 &&
        contract.anchors.every((anchor) => source.includes(anchor)),
      `${observation.id} committed target anchor is unavailable`,
    );
  }
  assert(targetSources.length > 0, `${observation.id} has no implementation contracts`);
  assert(
    Array.isArray(target.fixtureIds) &&
      target.fixtureIds.length > 0 &&
      target.fixtureIds.every((fixture) => targetSources.some((source) => source.includes(fixture))),
    `${observation.id} has an unmapped fixture`,
  );
  if (observation.disposition === "adopt") {
    assert(target.implementationCommit, `${observation.id} adopted evidence without an implementation commit`);
  }
  assert(
    immutableObservationDigests.get(observation.id) === objectDigest(observation),
    `${observation.id} changed after it was recorded`,
  );

  if (checkSources) {
    const sourceRoot = sourceRootById.get(reference.sourceId);
    const tree = execFileSync("git", ["-C", sourceRoot, "rev-parse", `${reference.commit}^{tree}`], {
      encoding: "utf8",
    }).trim();
    assert(tree === reference.tree, `${observation.id} source tree drifted`);
    const sourceText = reference.paths
      .map((sourcePath) =>
        execFileSync("git", ["-C", sourceRoot, "show", `${reference.commit}:${sourcePath}`], {
          encoding: "utf8",
          maxBuffer: 64 * 1024 * 1024,
        }),
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
  "Antigravity observation history is incomplete",
);

const expectedCapabilities = new Map([
  ["AG-01", "fixture_verified"],
  ["AG-02", "fixture_verified"],
  ["AG-03", "fixture_verified"],
  ["AG-04", "live_pending"],
  ["AG-N1", "fixture_verified"],
  ["AG-N2", "fixture_verified"],
  ["AG-N3", "fixture_verified"],
  ["AG-N4", "fixture_verified"],
  ["AG-N5", "fixture_verified"],
  ["AG-N6", "live_pending"],
  ["AG-N7", "fixture_verified"],
  ["AG-N8", "fixture_verified"],
  ["AG-N9", "fixture_verified"],
  ["AG-N10", "fixture_verified"],
  ["AG-N11", "fixture_verified"],
  ["CORE-N2-ANTIGRAVITY", "fixture_verified"],
]);
for (const capability of baseline.capabilities ?? []) {
  assert(
    expectedCapabilities.get(capability.id) === capability.status,
    `${capability.id} has an unsupported evidence state`,
  );
  expectedCapabilities.delete(capability.id);
  for (const contract of capability.contracts ?? []) {
    const localPath = path.resolve(root, contract.path);
    assert(localPath.startsWith(`${root}${path.sep}`) && fs.existsSync(localPath), `${capability.id} path is unavailable`);
    const source = fs.readFileSync(localPath, "utf8");
    for (const anchor of contract.anchors ?? []) {
      assert(source.includes(anchor), `${capability.id} is missing local anchor ${anchor}`);
    }
  }
  assert((capability.contracts ?? []).length > 0, `${capability.id} has no local contracts`);
}
assert(expectedCapabilities.size === 0, "Antigravity capability coverage is incomplete");
const requestTypeGate = (baseline.capabilities ?? []).find((capability) => capability.id === "AG-N6");
assert(
  requestTypeGate?.runtimeChange === false,
  "Antigravity requestType behavior changed without a real receipt",
);
const rails = baseline.realAcceptance?.rails ?? [];
const requiredChecks = [
  "bound_account_and_two_surface_bindings",
  "fresh_catalog",
  "authoritative_empty_catalog",
  "transient_stale_catalog",
  "claude_nonstream_stream_tool_usage",
  "gemini_nonstream_stream_tool_usage",
  "plain_request_type",
  "tools_request_type",
  "history_tool_request_type",
  "web_search_request_type",
  "mixed_tools_request_type",
  "reasoning_replay",
  "session_rollover",
  "same_account_first_401_recovery",
  "second_401_terminal",
  "structured_429_scope",
  "terminal_then_eof",
  "decoy_zero_requests",
  "compaction_gate_fail_closed",
  "secret_scan",
];
const requiredBodyHashes = [
  "claude_nonstream",
  "claude_stream",
  "gemini_nonstream",
  "gemini_stream",
  "plain_request",
  "tools_request",
  "history_tool_request",
  "web_search_request",
  "mixed_tools_request",
];
assert(
  baseline.realAcceptance?.receiptSchemaVersion === 1 &&
    baseline.realAcceptance?.harnessRevision === 1 &&
    JSON.stringify(baseline.realAcceptance?.requiredChecks) === JSON.stringify(requiredChecks) &&
    JSON.stringify(baseline.realAcceptance?.requiredBodyHashes) ===
      JSON.stringify(requiredBodyHashes),
  "Antigravity real acceptance receipt contract changed",
);
assert(
  JSON.stringify(rails.map((rail) => rail.rail).sort()) ===
    JSON.stringify(["agy_oauth", "antigravity_oauth"]) &&
    rails.every((rail) => rail.status === "live_pending" && rail.receipt === null),
  "Antigravity real rail gates changed without receipts",
);
assert(
  baseline.realAcceptance?.compaction?.status === "live_pending" &&
    baseline.realAcceptance?.compaction?.runtimeEnabled === false &&
    baseline.realAcceptance?.compaction?.receipt === null,
  "Antigravity compaction gate opened without a receipt",
);

const currentAcceptance = baseline.realAcceptanceExtensions ?? [];
assert(
  currentAcceptance.length === 1 &&
    currentAcceptance[0].id === "AG-LIVE-V2-2026-10-08" &&
    currentAcceptance[0].receiptSchemaVersion === 2 &&
    currentAcceptance[0].harnessRevision === 2 &&
    currentAcceptance[0].scopeDigestDomain === "cc-switch-server:antigravity-real-scope:v2" &&
    currentAcceptance[0].catalogDigestDomainPattern ===
      "cc-switch-server:antigravity-{app}-catalog:v2" &&
    currentAcceptance[0].catalogMaxAgeMs === 7_200_000 &&
    currentAcceptance[0].receiptMaxAgeMs === 86_400_000 &&
    currentAcceptance[0].clockSkewMs === 300_000 &&
    JSON.stringify(currentAcceptance[0].catalogSnapshotFields) ===
      JSON.stringify(["app", "model", "source", "stale", "fetchedAtMs", "digest"]) &&
    JSON.stringify(currentAcceptance[0].rails.map((entry) => entry.rail).sort()) ===
      JSON.stringify(["agy_oauth", "antigravity_oauth"]) &&
    currentAcceptance[0].rails.every((entry) => entry.status === "live_pending" && entry.receipt === null) &&
    currentAcceptance[0].compactionAutoEnable === false,
  "Antigravity current receipt extension or live gate changed",
);

const r4Ids = new Set(["R4-AG-01", "R4-AG-02", "R4-CL-02-DELEGATED"]);
for (const review of baseline.r4Reviews ?? []) {
  assert(r4Ids.delete(review.id), `unexpected or duplicate Antigravity R4 review ${review.id}`);
  assert(
    review.status === "working_tree_fixture_verified" &&
      review.liveAcceptance === "live_pending" &&
      ["adapt", "keep_delegated_contract"].includes(review.decision) &&
      Number.isFinite(Date.parse(review.reviewedAt)),
    `${review.id} claims an unsupported R4 evidence state`,
  );
  assert(Array.isArray(review.sources) && review.sources.length > 0, `${review.id} has no frozen sources`);
  for (const source of review.sources) {
    assert(
      sourceById.has(source.sourceId) &&
        commitPattern.test(source.snapshotCommit) && commitPattern.test(source.snapshotTree) &&
        source.readMode === "read_only_committed_git_objects" &&
        Array.isArray(source.files) && source.files.length > 0,
      `${review.id} has an invalid R4 source snapshot`,
    );
    const sourceRoot = sourceRootById.get(source.sourceId);
    if (checkSources) {
      const tree = execFileSync("git", ["-C", sourceRoot, "rev-parse", `${source.snapshotCommit}^{tree}`], { encoding: "utf8" }).trim();
      assert(tree === source.snapshotTree, `${review.id} source tree drifted`);
    }
    for (const file of source.files) {
      assert(file.path && !path.isAbsolute(file.path) && !file.path.split("/").includes("..") && digestPattern.test(file.sha256), `${review.id} has an unsafe source file`);
      if (checkSources) {
        const content = execFileSync("git", ["-C", sourceRoot, "show", `${source.snapshotCommit}:${file.path}`], { encoding: null, maxBuffer: 64 * 1024 * 1024 });
        assert(sha256(content) === file.sha256, `${review.id}:${file.path} source drifted`);
      }
    }
  }
  assert(
    commitPattern.test(review.target?.baselineCommit) &&
      review.target.implementationCommit === null && review.target.readMode === "working_tree" &&
      Array.isArray(review.target.contracts) && review.target.contracts.length > 0,
    `${review.id} must distinguish working-tree fixtures from committed implementation`,
  );
  assert(
    review.validationScope?.execution === "isolated_working_tree_snapshot" &&
      review.validationScope.baselineCommit === review.target.baselineCommit &&
      JSON.stringify(review.validationScope.providerSlices) === JSON.stringify(["Claude", "Qoder", "Antigravity"]) &&
      Number.isSafeInteger(review.validationScope.targetedAntigravityTests) &&
      review.validationScope.targetedAntigravityTests > 0 &&
      typeof review.validationScope.excludedConcurrentChanges === "string" &&
      review.validationScope.excludedConcurrentChanges.includes("Share Market") &&
      review.validationScope.liveAcceptance === "live_pending",
    `${review.id} must preserve the isolated validation and concurrent-change boundary`,
  );
  for (const contract of review.target.contracts) {
    assert(contract.path && !path.isAbsolute(contract.path) && !contract.path.split("/").includes(".."), `${review.id} target path is unsafe`);
    const content = fs.readFileSync(path.join(root, contract.path), "utf8");
    assert(Array.isArray(contract.anchors) && contract.anchors.length > 0 && contract.anchors.every((anchor) => content.includes(anchor)), `${review.id} fixture anchor is unavailable`);
  }
}
assert(r4Ids.size === 0, "Antigravity R4 fixture coverage is incomplete");

// The relocation of replay admission must retain current regression coverage;
// successful verification of an old commit cannot satisfy these contracts.
const residentReviewContracts = [
  ["src/proxy/providers/antigravity/mod.rs", [".get_accounted(&scope, now_ms, request_memory)"]],
  ["src/proxy/antigravity_replay/resident_tests.rs", [
    "antigravity_r4_review_unadmitted_miss_cannot_publish_after_capacity_returns",
    "antigravity_r4_review_request_copy_is_admitted_and_outlives_cache_entry",
    "antigravity_r4_review_replacement_refusal_removes_old_proof_and_fences_late_writer",
    "antigravity_r4_review_tombstone_count_expiry_and_drop_release_owners",
    "antigravity_r4_review_wire_limit_counts_escapes_while_resident_counts_capacity",
  ]],
  ["src/clients/oauth/antigravity_models/resident_tests.rs", [
    "antigravity_r4_review_catalog_snapshot_shares_payload_and_last_owner",
    "antigravity_r4_review_catalog_refusal_is_uncached_current_and_fences_older_republication",
    "antigravity_r4_review_catalog_revision_supersession_is_distinct_from_capacity_refusal",
  ]],
];
for (const [filePath, anchors] of residentReviewContracts) {
  const content = fs.readFileSync(path.join(root, filePath), "utf8");
  assert(
    anchors.every((anchor) => content.includes(anchor)),
    `Antigravity resident review current contract is unavailable: ${filePath}`,
  );
}
const r4Wire = JSON.parse(fs.readFileSync(path.join(root, "assets/contract/antigravity-r4-wire-delta.json"), "utf8"));
assert(
  r4Wire.format === "cc-switch-antigravity-r4-wire-delta" && r4Wire.schemaVersion === 1 &&
    r4Wire.evidenceState === "local_fixture_expectations" &&
    r4Wire.referenceExecution === "not_executed_read_only_committed_source_observations" &&
    JSON.stringify(r4Wire.rails) === JSON.stringify(["antigravity_oauth", "agy_oauth"]),
  "Antigravity R4 wire fixture evidence boundary changed",
);
assert(
  ["thought", "carrier", "call_text_call"].every((shape) => r4Wire.cases?.[shape]?.expectedFunctionCalls?.length === 2) &&
    r4Wire.currentToolPolicy?.mode === "NONE" && r4Wire.currentToolPolicy?.historyDoesNotAuthorizeCurrentCalls === true &&
    JSON.stringify(r4Wire.streamIdentity?.expectedGlobalIndexes) === "[0,1,2]" &&
    r4Wire.delegatedClaude?.liveAcceptance === "live_pending",
  "Antigravity R4 wire matrix is incomplete",
);

console.log(
  `antigravity reference delta audit ok (${sourceDeltaIds.size} deltas, ${observationIds.size} immutable observations, ${baseline.r4Reviews.length} R4 working-tree reviews${
    checkSources ? ", external objects verified" : ", external check optional"
  })`,
);
