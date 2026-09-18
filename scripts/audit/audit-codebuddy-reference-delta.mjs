#!/usr/bin/env node

import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const contract = JSON.parse(
  fs.readFileSync(path.join(repoRoot, "assets/contract/codebuddy-reference-delta.json"), "utf8"),
);
const checkSources = process.argv.includes("--check-sources");

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function safeRelative(value, label) {
  assert(
    typeof value === "string" && value && !path.isAbsolute(value) && !value.split("/").includes(".."),
    `${label} is not a safe relative path`,
  );
}

function digest(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

assert(
  contract.format === "cc-switch-codebuddy-reference-delta" && contract.schemaVersion === 1,
  "CodeBuddy reference delta format changed",
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
      assert(digest(content) === file.sha256, `${source.id}:${file.path} source hash drifted`);
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
          digest(content) === file.sha256,
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
  acceptance?.receiptSchemaVersion === 1 &&
    acceptance.requiredChecks.length === 16 &&
    new Set(acceptance.requiredChecks).size === 16,
  "CodeBuddy acceptance must retain 16 unique checks",
);
const sites = new Map(acceptance.sites.map((site) => [site.site, site]));
for (const site of ["intl", "cn"]) {
  assert(
    sites.get(site)?.status === "live_pending" && sites.get(site)?.receipt === null,
    `${site} improperly claims live evidence`,
  );
}

console.log(
  `codebuddy reference delta audit ok (${capabilities.size} capabilities, ${enhancements.size} enhancements, ${fixtureChecks.length} fixture checks, ${acceptance.requiredChecks.length} real checks, external check ${checkSources ? "verified" : "optional"})`,
);
