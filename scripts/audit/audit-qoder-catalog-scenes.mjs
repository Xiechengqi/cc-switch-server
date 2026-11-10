#!/usr/bin/env node
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const fixturePath = fileURLToPath(new URL("../../assets/contract/qoder-catalog-scenes.json", import.meta.url));
// Reviewed synthetic observations and local policy; no external test dependency.
const frozen = "bb9cf4d59947de5665879c52511ffd1515c5d7a0e38d84e1ac5c36d1483de6d7";
function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") return Object.fromEntries(
    Object.keys(value).sort().map((key) => [key, canonical(value[key])]),
  );
  return value;
}
export function loadQoderCatalogScenes() {
  const bytes = fs.readFileSync(fixturePath);
  assert(bytes.length <= 32 * 1024, "scene fixture exceeds 32 KiB");
  return JSON.parse(bytes.toString("utf8"));
}
export function auditQoderCatalogScenes(document) {
  assert.equal(document.schemaVersion, 1);
  assert.equal(document.kind, "synthetic_server_contract");
  assert.equal(document.source.liveState, "live_pending");
  assert.equal(document.policy.liveState, "live_pending");
  assert.equal(document.policy.appOnlyExecutable, false);
  assert.deepEqual(document.scope.credentialRails, ["global_oauth", "cn_oauth", "pat_job_token"]);
  const digest = crypto.createHash("sha256").update(JSON.stringify(canonical(document))).digest("hex");
  assert.equal(digest, frozen, "scene evidence or local policy drifted from reviewed digest");
  return { cases: document.cases.length, liveState: "live_pending" };
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  console.log("Qoder catalog scenes ok:", auditQoderCatalogScenes(loadQoderCatalogScenes()));
}
