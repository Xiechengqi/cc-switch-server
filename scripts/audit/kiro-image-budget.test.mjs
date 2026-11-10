import assert from "node:assert/strict";
import test from "node:test";
import { auditKiroImageBudget, loadKiroImageBudget } from "./audit-kiro-image-budget.mjs";

test("Kiro image evidence is local, independently frozen and live_pending", () => {
  assert.deepEqual(auditKiroImageBudget(loadKiroImageBudget()), { cases: 7, liveState: "live_pending" });
});
test("Kiro image evidence cannot change units, scope or claim vendor acceptance", () => {
  for (const mutate of [
    (doc) => { doc.localPolicy.unit = "decoded bytes"; },
    (doc) => { doc.localPolicy.maxWireBase64CharactersPerImage = 500000; },
    (doc) => { doc.reference.officialVendorLimitProven = true; },
    (doc) => { doc.scope.providerTypes.pop(); },
    (doc) => { doc.liveState = "live_verified"; },
    (doc) => { doc.scope.realReceipt = true; },
  ]) {
    const document = loadKiroImageBudget();
    mutate(document);
    assert.throws(() => auditKiroImageBudget(document));
  }
});
test("Kiro image source, coverage and replacement call chain require review", () => {
  for (const mutate of [
    (doc) => { doc.reference.commit = "0".repeat(40); },
    (doc) => { doc.cases.pop(); },
    (doc) => { doc.localAnchorMigration.currentEvidence[0].anchors = []; },
  ]) {
    const document = loadKiroImageBudget();
    mutate(document);
    assert.throws(() => auditKiroImageBudget(document), /drifted/);
  }
});
