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
for (const publishPath of [
  "assets/contract/provider-registry.json",
  "src/proxy/codex_models.rs",
]) {
  const source = fs.readFileSync(path.join(repoRoot, publishPath), "utf8");
  for (const model of expectedModels) {
    assert(!source.includes(model), `${model} was published by ${publishPath} before evidence`);
  }
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
  `codex reference delta audit ok (${capabilities.size} capabilities${
    checkSources ? ", external objects verified" : ", external check optional"
  })`,
);
