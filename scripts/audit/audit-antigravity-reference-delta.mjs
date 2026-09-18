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
  const targetSources = [];
  for (const contract of target.contracts ?? []) {
    const localPath = path.resolve(root, contract.path || "");
    assert(
      contract.path && localPath.startsWith(`${root}${path.sep}`) && fs.existsSync(localPath),
      `${observation.id} target path is unavailable`,
    );
    const source = fs.readFileSync(localPath, "utf8");
    targetSources.push(source);
    assert(
      Array.isArray(contract.anchors) &&
        contract.anchors.length > 0 &&
        contract.anchors.every((anchor) => source.includes(anchor)),
      `${observation.id} target anchor is unavailable`,
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

console.log(
  `antigravity reference delta audit ok (${sourceDeltaIds.size} deltas, ${observationIds.size} immutable observations${
    checkSources ? ", external objects verified" : ", external check optional"
  })`,
);
