#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { assertRequiredProviderCoverage } from "./provider-profile-coverage.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

function readContract(name) {
  return JSON.parse(
    fs.readFileSync(path.join(repoRoot, "assets/contract", name), "utf8"),
  );
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function rejectConflictMarkers(relativePath, source) {
  if (
    source.includes("<<<<<<< ") ||
    source.includes("\n=======\n") ||
    source.includes("\n>>>>>>> ")
  ) {
    throw new Error(`source contains a conflict marker: ${relativePath}`);
  }
}

function extractProviderTypesSource(relativePath, source) {
  rejectConflictMarkers(relativePath, source);
  const enumMatch = /pub enum ProviderType\s*\{([\s\S]*?)\n\}/.exec(source);
  const enumBody = enumMatch?.[1];
  const afterEnum = enumMatch
    ? source.slice(enumMatch.index + enumMatch[0].length)
    : "";
  const asStrBody = afterEnum.match(
    /pub fn as_str\((?:&)?self\)\s*->\s*&'static str\s*\{([\s\S]*?)\n\s*\}/,
  )?.[1];
  if (!enumBody || !asStrBody) {
    throw new Error(`unable to parse ProviderType contract from ${relativePath}`);
  }
  const unparsedEnumBody = enumBody
    .replace(/^\s*\/\/\/?.*$/gm, "")
    .replace(/^\s*#\[[^\]]+\]\s*$/gm, "")
    .replace(/^\s*[A-Z][A-Za-z0-9]*\s*,\s*$/gm, "")
    .trim();
  if (unparsedEnumBody) {
    throw new Error(
      `unsupported ProviderType enum syntax in ${relativePath}: ${unparsedEnumBody.split("\n")[0]}`,
    );
  }
  const variants = [
    ...enumBody.matchAll(/^\s*([A-Z][A-Za-z0-9]*)\s*,/gm),
  ].map((match) => match[1]);
  if (variants.length === 0 || new Set(variants).size !== variants.length) {
    throw new Error(`ProviderType variants are empty or duplicated in ${relativePath}`);
  }
  const ids = new Map(
    [
      ...asStrBody.matchAll(
        /(?:ProviderType|Self)::([A-Za-z0-9]+)\s*=>\s*"([^"]+)"/g,
      ),
    ].map((match) => [match[1], match[2]]),
  );
  const missing = variants.filter((variant) => !ids.has(variant));
  if (missing.length > 0) {
    throw new Error(`ProviderType variants without as_str: ${missing.join(", ")}`);
  }
  const extra = [...ids.keys()].filter((variant) => !variants.includes(variant));
  if (extra.length > 0) {
    throw new Error(`ProviderType as_str arms without variants: ${extra.join(", ")}`);
  }
  const values = variants.map((variant) => ids.get(variant));
  if (new Set(values).size !== values.length) {
    throw new Error(`ProviderType as_str ids are duplicated in ${relativePath}`);
  }
  return variants.map((variant) => ({ variant, id: ids.get(variant) }));
}

export function extractServerProviderTypesSource(relativePath, source) {
  const providerTypes = extractProviderTypesSource(relativePath, source);
  const enumBody = source.match(/pub enum ProviderType\s*\{([\s\S]*?)\n\}/)?.[1] ?? "";
  const serdeIds = new Map(
    [
      ...enumBody.matchAll(
        /#\[serde\(rename\s*=\s*"([^"]+)"\)\]\s*([A-Z][A-Za-z0-9]*)\s*,/g,
      ),
    ].map((match) => [match[2], match[1]]),
  );
  if (serdeIds.size !== providerTypes.length) {
    throw new Error(`Server ProviderType serde mappings are incomplete in ${relativePath}`);
  }
  for (const providerType of providerTypes) {
    if (serdeIds.get(providerType.variant) !== providerType.id) {
      throw new Error(
        `Server ProviderType serde/as_str mismatch for ${providerType.variant} in ${relativePath}`,
      );
    }
  }
  return providerTypes;
}

function assertUnique(items, key, label) {
  const values = items.map((item) => item[key]);
  if (values.some((value) => typeof value !== "string" || value.length === 0)) {
    throw new Error(`${label} contains an empty ${key}`);
  }
  if (new Set(values).size !== values.length) {
    throw new Error(`${label} contains a duplicate ${key}`);
  }
}

function assertPresetInventory(compatibility) {
  for (const app of ["claude", "codex", "gemini"]) {
    const presets = compatibility.presets?.[app];
    if (!Array.isArray(presets) || presets.length === 0) {
      throw new Error(`legacy compatibility has no ${app} presets`);
    }
    if (compatibility.counts?.[app] !== presets.length) {
      throw new Error(`legacy compatibility count drift for ${app}`);
    }
    assertUnique(presets, "name", `${app} legacy presets`);
    presets.forEach((preset, index) => {
      if (preset.sourceIndex !== index) {
        throw new Error(`${app} legacy preset sourceIndex drift at ${index}`);
      }
    });
  }
}

export function validateServerProviderContracts(
  requirements,
  compatibility,
  compatibilityWindow,
  registry,
) {
  if (
    requirements.schemaVersion !== 1 ||
    requirements.authority !== "cc-switch-server-product-requirements"
  ) {
    throw new Error("invalid Server Provider requirements contract");
  }
  const providerTypes = requirements.providerTypes;
  if (!Array.isArray(providerTypes) || providerTypes.length !== 25) {
    throw new Error(`Server Provider requirements must contain 25 types`);
  }
  assertUnique(providerTypes, "variant", "Server Provider requirements");
  assertUnique(providerTypes, "id", "Server Provider requirements");
  for (const entry of providerTypes) {
    if (
      !["core", "server_extension"].includes(entry.coverageClass) ||
      !Array.isArray(entry.apps) ||
      entry.apps.length === 0 ||
      entry.apps.some((app) => !["claude", "codex", "gemini"].includes(app))
    ) {
      throw new Error(`invalid Server Provider requirement: ${entry.id}`);
    }
  }

  const sourcePath = requirements.providerTypeSource?.path;
  const source = fs.readFileSync(path.join(repoRoot, sourcePath), "utf8");
  if (sha256(source) !== requirements.providerTypeSource?.sha256) {
    throw new Error(`ProviderType source hash drift: ${sourcePath}`);
  }
  const extracted = extractServerProviderTypesSource(sourcePath, source);
  const expected = providerTypes.map(({ variant, id }) => ({ variant, id }));
  if (JSON.stringify(extracted) !== JSON.stringify(expected)) {
    throw new Error("ProviderType source does not match Server product requirements");
  }

  if (
    compatibility.schemaVersion !== 1 ||
    compatibility.authority !== "server-owned-legacy-compatibility"
  ) {
    throw new Error("invalid legacy Provider compatibility contract");
  }
  assertPresetInventory(compatibility);
  const windowEligible = compatibilityWindow.policy?.removalEligible === true;
  if (compatibility.providerApi?.removalEligible !== windowEligible) {
    throw new Error("legacy Provider API removal gate disagrees with compatibility window");
  }
  const endpointEntries = new Set(
    compatibilityWindow.entries
      .filter((entry) => entry.kind === "endpoint")
      .map((entry) => entry.id),
  );
  for (const endpoint of [
    "provider-presets-endpoint",
    "provider-matrix-endpoint",
    "provider-type-endpoint",
  ]) {
    if (!endpointEntries.has(endpoint)) {
      throw new Error(`compatibility window is missing ${endpoint}`);
    }
  }
  for (const profileId of compatibility.firstClassProfileAdditions ?? []) {
    if (!registry.profiles.some((profile) => profile.profileId === profileId)) {
      throw new Error(`legacy bridge references a missing Profile: ${profileId}`);
    }
  }
  for (const recipeId of compatibility.customRecipeAdditions ?? []) {
    if (!registry.customRecipes.some((recipe) => recipe.recipeId === recipeId)) {
      throw new Error(`legacy bridge references a missing recipe: ${recipeId}`);
    }
  }
  assertRequiredProviderCoverage(registry);
}

function main() {
  const requirements = readContract("server-provider-requirements.json");
  const compatibility = readContract("provider-legacy-compatibility.json");
  const compatibilityWindow = readContract("provider-compatibility-window.json");
  const registry = readContract("provider-registry.json");
  validateServerProviderContracts(
    requirements,
    compatibility,
    compatibilityWindow,
    registry,
  );
  console.log(
    `Server Provider contracts ok: ${requirements.providerTypes.length} types, ${Object.values(compatibility.counts).join("/")} legacy fixtures`,
  );
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main();
}
