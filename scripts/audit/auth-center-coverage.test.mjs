import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import { auditAuthCenterCoverage } from "./auth-center-coverage.mjs";

const read = (file) => fs.readFileSync(new URL(`../../${file}`, import.meta.url), "utf8");
const accountManagers = read("src/domain/accounts/managers.rs");
const model = read("src/domain/providers/model.rs");
const registry = read("web-src/src/components/settings/authCenterProviders.tsx");
const variantToId = new Map([...model.matchAll(/Self::([A-Za-z0-9]+)\s*=>\s*"([^"]+)"/g)]
  .map((match) => [match[1], match[2]]));
const knownEntryGaps = JSON.parse(read("assets/contract/web-runtime-contract.json")).authCenter.knownEntryGaps;
const audit = (overrides = {}) => auditAuthCenterCoverage({ accountManagers, variantToId, registry, knownEntryGaps, ...overrides });

test("AuthCenter covers Rust managed accounts with an explicit pre-existing Trae gap", () => {
  assert.deepEqual(audit(), { errors: [], entryCount: 15, knownEntryGaps: ["trae_solo"] });
});

for (const product of ["codebuddy_oauth", "workbuddy_oauth"]) {
  test(`AuthCenter rejects a missing ${product} entry`, () => {
    const result = audit({ registry: registry.replace(`providerType: "${product}"`, "removed: true") });
    assert.ok(result.errors.includes(`AuthCenter is missing managed-account entry ${product}`));
  });
}

test("a newly declared managed OAuth provider requires an AuthCenter decision", () => {
  const expanded = accountManagers.replace(
    "=> AccountCredentialOwnership::ManagedAccount,",
    "| ProviderType::FixtureOAuth => AccountCredentialOwnership::ManagedAccount,",
  );
  assert.ok(audit({ accountManagers: expanded,
    variantToId: new Map([...variantToId, ["FixtureOAuth", "fixture_oauth"]]),
  }).errors.includes("AuthCenter is missing managed-account entry fixture_oauth"));
});

test("AuthCenter rejects duplicates and Provider-owned static credentials", () => {
  const result = audit({ registry: `${registry}\n{providerType: "workbuddy_oauth"}\n{providerType: "aws_bedrock"}` });
  assert.ok(result.errors.includes("AuthCenter has duplicate entry workbuddy_oauth"));
  assert.ok(result.errors.includes("AuthCenter entry aws_bedrock is not a managed-account provider"));
});

test("known gaps need a reason and must be removed when implemented", () => {
  assert.ok(audit({ knownEntryGaps: [{ providerType: "trae_solo", status: "known_gap", reason: "" }] })
    .errors.includes("AuthCenter has invalid known entry gap trae_solo"));
  assert.ok(audit({ registry: `${registry}\n{providerType: "trae_solo"}` })
    .errors.includes("AuthCenter known entry gap trae_solo is already implemented"));
});
