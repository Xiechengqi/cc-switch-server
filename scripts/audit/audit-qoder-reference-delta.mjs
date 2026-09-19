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
    contract.schemaVersion === 1,
  "Qoder reference delta format changed",
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
  for (const file of source.files ?? []) {
    safeRelative(file.path, `${source.id} evidence path`);
    assert(
      /^[a-f0-9]{64}$/.test(file.sha256),
      `${source.id}:${file.path} has an invalid SHA-256`,
    );
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
}

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
  `qoder reference delta audit ok (${capabilities.size} capabilities, revision ${truth.driverContractRevision}, ${acceptance.requiredChecks.length} real checks, external check ${checkSources ? "verified" : "optional"})`,
);
