import assert from "node:assert/strict";
import test from "node:test";
import { auditQoderCatalogScenes, loadQoderCatalogScenes } from "./audit-qoder-catalog-scenes.mjs";

test("Qoder scene evidence stays local, frozen and live_pending", () => {
  assert.deepEqual(auditQoderCatalogScenes(loadQoderCatalogScenes()), { cases: 12, liveState: "live_pending" });
});
test("Qoder scene source and coherent input/output mutations require independent review", () => {
  for (const mutate of [
    (doc) => { doc.source.commit = "0".repeat(40); },
    (doc) => { doc.cases[0].input.chat[0].source = "changed"; doc.cases[0].selectedConfigs.shared.source = "changed"; doc.cases[0].catalogEntries[0].source = "changed"; },
    (doc) => { doc.cases.pop(); },
  ]) {
    const doc = loadQoderCatalogScenes();
    mutate(doc);
    assert.throws(() => auditQoderCatalogScenes(doc), /drifted/);
  }
});
test("Qoder discovery evidence cannot grant app inference, new rails or live success", () => {
  for (const mutate of [
    (doc) => { doc.policy.appOnlyExecutable = true; },
    (doc) => { doc.scope.credentialRails.push("cn_pat"); },
    (doc) => { doc.source.liveState = "live_verified"; },
  ]) {
    const doc = loadQoderCatalogScenes();
    mutate(doc);
    assert.throws(() => auditQoderCatalogScenes(doc));
  }
});
