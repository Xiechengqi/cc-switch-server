import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  extractServerProviderTypesSource,
  validateServerProviderContracts,
} from "./audit-server-provider-contract.mjs";
import {
  assertRequiredProviderCoverage,
  requiredProviderProfilePairs,
} from "./provider-profile-coverage.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

function contract(name) {
  return JSON.parse(
    fs.readFileSync(path.join(repoRoot, "assets/contract", name), "utf8"),
  );
}

function contracts() {
  return {
    requirements: contract("server-provider-requirements.json"),
    compatibility: contract("provider-legacy-compatibility.json"),
    window: contract("provider-compatibility-window.json"),
    registry: contract("provider-registry.json"),
  };
}

function validate(input = contracts()) {
  return validateServerProviderContracts(
    input.requirements,
    input.compatibility,
    input.window,
    input.registry,
  );
}

test("checked-in Server Provider contracts satisfy product requirements", () => {
  assert.doesNotThrow(() => validate());
});

test("Provider registry matches the shared runtime inventory expectations", () => {
  const registry = contract("provider-registry.json");
  const expected = contract("provider-registry-expectations.json");
  assert.equal(registry.families.length, expected.counts.families);
  assert.equal(registry.profiles.length, expected.counts.profiles);
  assert.equal(
    registry.legacyPresetMappings.length,
    expected.counts.legacyPresetMappings,
  );
  assert.equal(registry.drivers.length, expected.counts.drivers);
  for (const [app, count] of Object.entries(expected.firstClassProfiles)) {
    assert.equal(
      registry.profiles.filter(
        (profile) =>
          profile.app === app &&
          profile.formComposition !== "custom" &&
          profile.formComposition !== "legacy",
      ).length,
      count,
      app,
    );
  }
  const ids = (items, key) => new Set(items.map((item) => item[key]));
  const profileIds = ids(registry.profiles, "profileId");
  const driverIds = ids(registry.drivers, "driverId");
  const familyIds = ids(registry.families, "familyId");
  for (const id of expected.requiredProfileIds) assert.equal(profileIds.has(id), true, id);
  for (const id of expected.requiredDriverIds) assert.equal(driverIds.has(id), true, id);
  for (const id of expected.requiredFamilyIds) assert.equal(familyIds.has(id), true, id);
});

test("first-class Server Profiles are committed additions, not candidates", () => {
  const compatibility = contract("provider-legacy-compatibility.json");
  const registry = contract("provider-registry.json");
  assert.equal(compatibility.firstClassProfileAdditions.length, 41);
  assert.deepEqual(compatibility.customRecipeAdditions, [
    "claude.anthropic_bearer_relay",
  ]);

  const mappedProfileIds = new Set(
    registry.legacyPresetMappings.map((mapping) => mapping.profileId),
  );
  const unmappedFirstClassProfiles = registry.profiles
    .filter(
      (profile) =>
        profile.visibility === "visible" &&
        profile.formComposition !== "custom" &&
        profile.formComposition !== "legacy" &&
        !mappedProfileIds.has(profile.profileId),
    )
    .map((profile) => profile.profileId)
    .sort();
  assert.deepEqual(
    unmappedFirstClassProfiles,
    [...compatibility.firstClassProfileAdditions].sort(),
  );
});

test("every required Provider type/app pair has a creatable Profile or recipe", () => {
  const registry = contract("provider-registry.json");
  assert.equal(requiredProviderProfilePairs().length, 52);
  assert.doesNotThrow(() => assertRequiredProviderCoverage(registry));

  const missing = structuredClone(registry);
  missing.profiles = missing.profiles.filter(
    (profile) => profile.profileId !== "claude.google_oauth",
  );
  assert.throws(
    () => assertRequiredProviderCoverage(missing),
    /Missing visible create_allowed Provider Profile or Custom HTTP recipe for claude:gemini_cli/,
  );

  const missingRecipe = structuredClone(registry);
  missingRecipe.customRecipes = missingRecipe.customRecipes.filter(
    (recipe) => recipe.recipeId !== "claude.anthropic_bearer_relay",
  );
  assert.throws(
    () => assertRequiredProviderCoverage(missingRecipe),
    /Missing visible create_allowed Provider Profile or Custom HTTP recipe for claude:claude_auth/,
  );
});

test("requirements reject omitted and duplicate Provider types", () => {
  const omitted = contracts();
  omitted.requirements.providerTypes.pop();
  assert.throws(() => validate(omitted), /must contain 25 types/);

  const duplicate = contracts();
  duplicate.requirements.providerTypes[1].id =
    duplicate.requirements.providerTypes[0].id;
  assert.throws(() => validate(duplicate), /duplicate id/);
});

test("legacy fixtures reject count, order, and removal-gate drift", () => {
  const countDrift = contracts();
  countDrift.compatibility.counts.claude -= 1;
  assert.throws(() => validate(countDrift), /count drift for claude/);

  const orderDrift = contracts();
  orderDrift.compatibility.presets.codex[0].sourceIndex = 2;
  assert.throws(() => validate(orderDrift), /sourceIndex drift/);

  const gateDrift = contracts();
  gateDrift.compatibility.providerApi.removalEligible = true;
  assert.throws(() => validate(gateDrift), /removal gate disagrees/);
});

test("Server ProviderType extraction requires serde and as_str to agree", () => {
  const valid = `
pub enum ProviderType {
    #[serde(rename = "claude")]
    Claude,
    #[serde(rename = "codex")]
    Codex,
}

impl ProviderType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}
`;
  assert.deepEqual(extractServerProviderTypesSource("fixture.rs", valid), [
    { variant: "Claude", id: "claude" },
    { variant: "Codex", id: "codex" },
  ]);

  assert.throws(
    () =>
      extractServerProviderTypesSource(
        "fixture.rs",
        valid.replace(
          '#[serde(rename = "codex")]',
          '#[serde(rename = "openai")]',
        ),
      ),
    /serde\/as_str mismatch/,
  );
  assert.throws(
    () => extractServerProviderTypesSource("fixture.rs", `<<<<<<< ours\n${valid}`),
    /conflict marker/,
  );
});
