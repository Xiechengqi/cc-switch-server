import assert from "node:assert/strict";
import test from "node:test";
import {
  auditQoderCapabilityExtension,
  loadQoderCapabilityExtension,
} from "./audit-qoder-capability-extension.mjs";

test("Qoder additive official capability evidence stays self-contained and live_pending", () => {
  assert.deepEqual(auditQoderCapabilityExtension(loadQoderCapabilityExtension()), {
    packages: 2, observations: 3, revision: 3, liveState: "live_pending",
  });
});

test("Qoder extension rejects coherent input/output drift", () => {
  const document = loadQoderCapabilityExtension();
  document.observations.image.input.content[1].source.url = "https://changed.invalid/a.png";
  document.observations.image.output[0].content[1].image_url.url = "https://changed.invalid/a.png";
  document.observations.image.output[0].contents[1].image_url.url = "https://changed.invalid/a.png";
  assert.throws(() => auditQoderCapabilityExtension(document), /observations drifted/);
});

test("Qoder extension rejects changed official source identity or function extraction", () => {
  for (const change of [
    (doc) => { doc.officialPackages[1].bundleSha256 = doc.officialPackages[0].bundleSha256; },
    (doc) => { doc.officialPackages[0].functions[0].sha256 = "0".repeat(64); },
    (doc) => { doc.officialPackages[0].functions.pop(); },
  ]) {
    const document = loadQoderCapabilityExtension();
    change(document);
    assert.throws(() => auditQoderCapabilityExtension(document), /evidence drifted/);
  }
});

test("Qoder extension cannot promote live state, add a rail or depend on external paths", () => {
  for (const change of [
    (doc) => { doc.liveState = "live_verified"; },
    (doc) => { doc.externalPath = "/external/official-cli"; },
    (doc) => { doc.rails = ["cn_pat"]; },
  ]) {
    const document = loadQoderCapabilityExtension();
    change(document);
    assert.throws(() => auditQoderCapabilityExtension(document));
  }
});
