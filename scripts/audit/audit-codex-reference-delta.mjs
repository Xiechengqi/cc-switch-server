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

const baseline = JSON.parse(fs.readFileSync(baselinePath, "utf8"));
assert(
  baseline.format === "cc-switch-codex-reference-delta" &&
    baseline.schemaVersion === 1,
  "Codex reference delta baseline format changed",
);
assert(
  baseline.policy?.externalSources ===
    "read_only_optional_audit_input_never_runtime_dependency",
  "Codex external-source boundary changed",
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
  baseline.policy?.recoveryBoundary ===
    "same-account, pre-commit, shared total attempt budget",
  "Codex recovery boundary changed",
);

for (const source of baseline.sources ?? []) {
  assert(source.id && source.rootEnv, "Codex audit source metadata is incomplete");
  const sourceRoot = path.resolve(
    repoRoot,
    process.env[source.rootEnv] || source.defaultRelativeRoot,
  );
  for (const delta of source.deltas ?? []) {
    assert(delta.id, "Codex source delta is missing an id");
    assert(
      Array.isArray(delta.commits) &&
        delta.commits.length > 0 &&
        delta.commits.every((commit) => /^[a-f0-9]{40}$/.test(commit)),
      `${delta.id} has invalid commits`,
    );
    assert(
      Array.isArray(delta.files) && delta.files.length >= 2,
      `${delta.id} must pin implementation and fixture evidence`,
    );
    for (const file of delta.files) {
      assert(
        delta.commits.includes(file.commit),
        `${delta.id}:${file.path} uses an undeclared commit`,
      );
      assert(
        typeof file.path === "string" &&
          file.path.length > 0 &&
          !path.isAbsolute(file.path) &&
          !file.path.split("/").includes(".."),
        `${delta.id} has an unsafe evidence path`,
      );
      assert(
        /^[a-f0-9]{64}$/.test(file.sha256),
        `${delta.id}:${file.path} has an invalid SHA-256`,
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
          `${source.id}:${delta.id}:${file.path} drifted from the reviewed object`,
        );
      }
    }
  }
}

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

const sourceDeltaIds = new Set(
  (baseline.sources ?? []).flatMap((source) =>
    (source.deltas ?? []).map((delta) => delta.id),
  ),
);
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
  `codex reference delta audit ok (${capabilities.size} baseline capabilities, ${incrementalEnhancements.size} incremental enhancements${
    checkSources ? ", external objects verified" : ", external check optional"
  })`,
);
