#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { execFileSync } from "node:child_process";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const baselinePath = path.join(
  repoRoot,
  "assets/contract/claude-reference-delta.json",
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
  baseline.format === "cc-switch-claude-reference-delta" &&
    baseline.schemaVersion === 1,
  "Claude reference delta baseline format changed",
);
assert(
  baseline.policy?.externalSources ===
    "read_only_optional_audit_input_never_runtime_dependency",
  "Claude external-source boundary changed",
);
assert(
  baseline.policy?.scope?.includes("no pool") &&
    baseline.policy.scope.includes("no cross-account fallback") &&
    baseline.policy.scope.includes("no cross-Provider fallback"),
  "Claude fixed-binding scope changed",
);

const sourceDeltaIds = new Set();
for (const source of baseline.sources ?? []) {
  assert(source.id && source.rootEnv, "Claude audit source metadata is incomplete");
  assert(
    typeof source.defaultRelativeRoot === "string" &&
      source.defaultRelativeRoot.length > 0,
    `${source.id} has no default relative root`,
  );
  const sourceRoot = path.resolve(
    repoRoot,
    process.env[source.rootEnv] || source.defaultRelativeRoot,
  );
  for (const delta of source.deltas ?? []) {
    assert(!sourceDeltaIds.has(delta.id), `duplicate Claude delta ${delta.id}`);
    sourceDeltaIds.add(delta.id);
    assert(/^[a-f0-9]{40}$/.test(delta.commit), `${delta.id} has an invalid commit`);
    assert(
      Array.isArray(delta.files) && delta.files.length >= 2,
      `${delta.id} must pin implementation and fixture evidence`,
    );
    for (const file of delta.files) {
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
          ["-C", sourceRoot, "show", `${delta.commit}:${file.path}`],
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

const localIds = new Set();
for (const contract of baseline.localContracts ?? []) {
  assert(!localIds.has(contract.id), `duplicate local Claude contract ${contract.id}`);
  localIds.add(contract.id);
  assert(
    contract.status === "fixture_verified",
    `${contract.id} claims an unsupported evidence state`,
  );
  const localPath = path.resolve(repoRoot, contract.path);
  assert(
    localPath.startsWith(`${repoRoot}${path.sep}`) && fs.existsSync(localPath),
    `${contract.id} local fixture path is unavailable`,
  );
  const source = fs.readFileSync(localPath, "utf8");
  assert(
    Array.isArray(contract.anchors) && contract.anchors.length >= 1,
    `${contract.id} has no local fixture anchors`,
  );
  for (const anchor of contract.anchors) {
    assert(source.includes(anchor), `${contract.id} is missing local anchor ${anchor}`);
  }
}

assert(
  JSON.stringify([...sourceDeltaIds].sort()) ===
    JSON.stringify([...localIds].sort()),
  "Claude reference deltas and local fixture contracts do not match",
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
    JSON.stringify(operation.requiredChecks) ===
      JSON.stringify(expected.requiredChecks) &&
      JSON.stringify(operation.requiredBodyHashes) ===
        JSON.stringify(expected.requiredBodyHashes),
    `Claude operation ${expected.operation} receipt contract changed`,
  );
}

const providerModule = fs.readFileSync(
  path.join(repoRoot, "src/proxy/providers/claude/mod.rs"),
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
const forwarder = fs.readFileSync(path.join(repoRoot, "src/proxy/forwarder.rs"), "utf8");
for (const anchor of [
  "claude::record_quota_response_headers",
  "claude::handle_rate_limit",
  "claude::transport_replay_safe",
]) {
  assert(forwarder.includes(anchor), `Claude forwarder delegation is missing ${anchor}`);
}
const receiptHarness = fs.readFileSync(
  path.join(repoRoot, "scripts/smoke/claude-real-receipt.mjs"),
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
  path.join(repoRoot, "scripts/smoke/claude-oauth-real.mjs"),
  "utf8",
);
assert(
  legacyProbe.includes("Probe output is not live acceptance") &&
    !legacyProbe.includes("text.slice(0, 500)"),
  "Claude probe must stay redacted and subordinate to private receipts",
);

console.log(
  `claude reference delta audit ok (${localIds.size} contracts, ${expectedOperations.length} live-pending operations${
    checkSources ? ", external objects verified" : ", external check optional"
  })`,
);
