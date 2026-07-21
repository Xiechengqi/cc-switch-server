import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  extractPresetSource,
  extractProviderTypesSource,
  extractServerProviderTypesSource,
  rejectConflictMarkers,
  validateBaselineContracts,
} from "./audit-upstream-provider-baseline.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

function contract(name) {
  return JSON.parse(fs.readFileSync(path.join(repoRoot, "assets/contract", name), "utf8"));
}

function providerTypesSource(variants, arms) {
  return `
pub enum ProviderType {
${variants}
}

impl ProviderType {
    pub fn as_str(&self) -> &'static str {
        match self {
${arms}
        }
    }
}
`;
}

test("checked-in provider inventories satisfy the reviewed contract", () => {
  assert.doesNotThrow(() =>
    validateBaselineContracts(
      contract("upstream-provider-source-baseline.json"),
      contract("server-provider-legacy-inventory.json"),
    ),
  );
});

test("direct official API Profiles are committed additions, not candidates", () => {
  const mappings = contract("server-provider-legacy-inventory.json").coverageMappings;
  assert.deepEqual(mappings.firstClassProfileAdditions, [
    "claude.anthropic_api_key",
    "codex.openai_api_key",
    "gemini.google_api_key",
  ]);
  assert.equal("directApiCandidates" in mappings, false);
});

test("inventory validation rejects omitted and duplicate presets before write", () => {
  const baseline = contract("upstream-provider-source-baseline.json");
  const server = contract("server-provider-legacy-inventory.json");
  baseline.appPresets.claude.pop();
  assert.throws(
    () => validateBaselineContracts(baseline, server),
    /requires reviewed count 15/,
  );

  const duplicateBaseline = contract("upstream-provider-source-baseline.json");
  duplicateBaseline.appPresets.claude[1].name = duplicateBaseline.appPresets.claude[0].name;
  assert.throws(
    () => validateBaselineContracts(duplicateBaseline, server),
    /duplicate preset name/,
  );
});

test("static TypeScript extraction rejects malformed and executable input", () => {
  assert.throws(
    () =>
      extractPresetSource(
        "fixture.ts",
        Buffer.from("export const providerPresets = [{ name: 'broken' }"),
        "providerPresets",
        "claude",
      ),
    /TypeScript parse failed/,
  );
  assert.throws(
    () =>
      extractPresetSource(
        "fixture.ts",
        Buffer.from("export const providerPresets = buildPresets();"),
        "providerPresets",
        "claude",
      ),
    /unsupported call buildPresets/,
  );
});

test("conflict markers are rejected", () => {
  assert.throws(
    () => rejectConflictMarkers("fixture.ts", "<<<<<<< ours\n=======\n>>>>>>> theirs\n"),
    /conflict marker/,
  );
});

test("ProviderType extraction rejects unsupported variants and incomplete mappings", () => {
  const valid = extractProviderTypesSource(
    "fixture.rs",
    providerTypesSource(
      "    Claude,\n    Codex,",
      '            ProviderType::Claude => "claude",\n            ProviderType::Codex => "codex",',
    ),
  );
  assert.deepEqual(valid, [
    { variant: "Claude", id: "claude" },
    { variant: "Codex", id: "codex" },
  ]);

  assert.throws(
    () =>
      extractProviderTypesSource(
        "fixture.rs",
        providerTypesSource(
          "    Claude(String),\n    Codex,",
          '            ProviderType::Claude => "claude",\n            ProviderType::Codex => "codex",',
        ),
      ),
    /unsupported ProviderType enum syntax/,
  );
  assert.throws(
    () =>
      extractProviderTypesSource(
        "fixture.rs",
        providerTypesSource(
          "    Claude,\n    Codex,",
          '            ProviderType::Claude => "claude",',
        ),
      ),
    /variants without as_str: Codex/,
  );
  assert.throws(
    () =>
      extractProviderTypesSource(
        "fixture.rs",
        providerTypesSource(
          "    Claude,\n    Codex,",
          '            ProviderType::Claude => "same",\n            ProviderType::Codex => "same",',
        ),
      ),
    /ids are duplicated/,
  );
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
        valid.replace('#[serde(rename = "codex")]', '#[serde(rename = "openai")]'),
      ),
    /serde\/as_str mismatch/,
  );
  assert.throws(
    () =>
      extractServerProviderTypesSource(
        "fixture.rs",
        valid.replace('    #[serde(rename = "codex")]\n', ""),
      ),
    /serde mappings are incomplete/,
  );
});
