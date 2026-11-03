import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import test from "node:test";

test("provider presentation and all Registry surface fixtures are reproducible locally", () => {
  execFileSync(process.execPath, ["scripts/generate-provider-presentation.mjs", "--check"], { cwd: new URL("../../", import.meta.url) });
  const read = (file) => JSON.parse(readFileSync(new URL(`../../${file}`, import.meta.url), "utf8"));
  const registry = read("assets/contract/provider-registry.json");
  const fixtures = read("assets/generated/provider-identity-fixtures.json");
  assert.equal(fixtures.length, registry.families.length);
  for (const family of registry.families) {
    const fixture = fixtures.find((f) => f.name === family.familyId);
    assert.deepEqual(Object.keys(fixture.bindings).sort(), family.surfaces.map((s) => s.app).sort());
  }
});
