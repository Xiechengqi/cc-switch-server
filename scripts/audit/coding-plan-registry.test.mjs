import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const manifestPath = path.join(
  repoRoot,
  "assets/contract/coding-plan-registry-manifest.json",
);

test("coding-plan registry manifest is generated from the current typed contracts", () => {
  const result = spawnSync(
    process.execPath,
    ["scripts/audit/audit-coding-plan-registry.mjs", "--check"],
    { cwd: repoRoot, encoding: "utf8" },
  );
  assert.equal(result.status, 0, result.stderr || result.stdout);

  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  assert.equal(manifest.summary.typedFamilies, 10);
  assert.equal(manifest.summary.typedProfiles, 20);
  assert.deepEqual(manifest.summary.surfaces, ["claude", "codex"]);
  assert.equal(manifest.summary.fixtureState, "fixture_verified");
  assert.equal(manifest.summary.liveState, "live_pending");
  assert.deepEqual(Object.keys(manifest.generatedFrom.sourceCommits).sort(), [
    "9router",
    "omniroute",
  ]);
  assert.deepEqual(manifest.invariants, {
    credentialOwnership: "provider_owned",
    accountPool: false,
    crossAccountFallback: false,
    crossProviderFallback: false,
    crossCredentialRailFallback: false,
    quotaSelection: false,
    consoleCookieScraping: false,
    liveWithoutReceipt: false,
  });

  const regionSurfaces = new Set();
  for (const family of manifest.families) {
    assert.deepEqual(
      family.surfaces.map((surface) => surface.app),
      ["claude", "codex"],
      family.familyId,
    );
    assert.ok(family.evidenceFiles.length > 0, family.familyId);
    assert.ok(family.planIds.length > 0, family.familyId);
    assert.ok(
      family.evidenceFiles.every((evidence) =>
        ["9router", "omniroute"].includes(evidence.sourceId),
      ),
      family.familyId,
    );
    for (const surface of family.surfaces) {
      assert.equal(surface.region, family.region);
      assert.equal(surface.providerOwnedCredential, true);
      assert.equal(surface.accountBindingSupported, false);
      assert.equal(surface.fixtureState, "fixture_verified");
      assert.equal(surface.liveState, "live_pending");
      assert.match(surface.inference.fixedOrigin, /^https:\/\//);
      assert.ok(surface.catalog.modelCount > 0);
      assert.ok(surface.catalog.inputModalities.includes("text"));
      assert.equal(
        surface.catalog.tools,
        "not_inferred_without_explicit_model_evidence",
      );
      if (surface.quota.adapter === "unavailable") {
        assert.equal(surface.quota.endpoint, null);
        assert.deepEqual(surface.quota.credentialSlots, []);
        assert.equal(
          surface.quota.provenance,
          "explicit_unavailable_no_console_cookie",
        );
      } else {
        assert.match(surface.quota.endpoint, /^https:\/\//);
        assert.ok(surface.quota.credentialSlots.length > 0);
        assert.equal(surface.quota.provenance, "reviewed_plan_api");
      }
      regionSurfaces.add(`${family.familyId}:${family.region}:${surface.app}`);
    }
  }
  assert.equal(regionSurfaces.size, 20);
});

test("Ollama Cloud stays Provider-owned, display-only, and generation fenced", () => {
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  const ollama = manifest.ollamaCloud;
  assert.equal(ollama.credentialOwnership, "provider_owned");
  assert.equal(ollama.inferenceAccountRows, false);
  assert.equal(ollama.cookieOrHtmlCredential, false);
  assert.deepEqual(
    ollama.profiles.map((profile) => profile.app),
    ["claude", "codex"],
  );
  assert.equal(ollama.accountProjection.concurrentPartialSections, true);
  assert.equal(ollama.accountProjection.redirects, "disabled");
  assert.equal(ollama.accountProjection.maxResponseBytes, 512 * 1024);
  assert.deepEqual(ollama.accountProjection.cacheScope, [
    "credential_source_key",
    "credential_generation",
  ]);
  assert.deepEqual(ollama.accountProjection.staleOnlyFor, [
    "rate_limited",
    "transient",
  ]);
  assert.equal(ollama.accountProjection.authenticationFailureClearsCache, true);
  assert.equal(ollama.accountProjection.inferenceSchedulingEffect, "none_display_only");
  assert.equal(ollama.fixtureState, "fixture_verified");
  assert.equal(ollama.liveState, "live_pending");
});
