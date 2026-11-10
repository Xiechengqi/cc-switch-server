#!/usr/bin/env node
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const fixturePath = fileURLToPath(new URL("../../assets/contract/kiro-image-budget.json", import.meta.url));
const reviewedDigest = "5d4826d031da86672a2909c0779fbaef47e074446fc7971d7d0548281e7611bb";
function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") return Object.fromEntries(
    Object.keys(value).sort().map((key) => [key, canonical(value[key])]),
  );
  return value;
}
export function loadKiroImageBudget() {
  const bytes = fs.readFileSync(fixturePath);
  assert(bytes.length < 32 * 1024, "Kiro image fixture exceeds 32 KiB");
  return JSON.parse(bytes.toString("utf8"));
}
export function auditKiroImageBudget(document) {
  assert.equal(document.schemaVersion, 1);
  assert.equal(document.kind, "synthetic_server_contract");
  assert.equal(document.liveState, "live_pending");
  assert.equal(document.scope.realReceipt, false);
  assert.equal(document.reference.officialVendorLimitProven, false);
  assert.deepEqual(document.scope.providerTypes, ["kiro_oauth", "amazon_q_oauth"]);
  const digest = crypto.createHash("sha256").update(JSON.stringify(canonical(document))).digest("hex");
  assert.equal(digest, reviewedDigest, "Kiro image evidence or local policy drifted from reviewed digest");
  return { cases: document.cases.length, liveState: "live_pending" };
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  console.log("Kiro image budget ok:", auditKiroImageBudget(loadKiroImageBudget()));
}
