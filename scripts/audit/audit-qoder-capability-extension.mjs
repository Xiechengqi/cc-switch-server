#!/usr/bin/env node

import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { auditQoderCliOracle, loadQoderCliOracle } from "./audit-qoder-cli-oracle.mjs";

export const extensionPath = fileURLToPath(
  new URL("../../assets/contract/qoder-capability-extension.json", import.meta.url),
);

// Independent mutation tripwires. Extending capability evidence does not
// rewrite the old complete-wire oracle or its historical verification counts.
const frozenPackages = "58bf5ee680a3a83373158856a91ce51c73539be05ac5b2297f3bcae41345d813";
const frozenObservations = "7b29308b9f6ac7132f96401d2c06545d756b27c14288acd51099d0c5722d3b9b";

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, item]) => [key, canonical(item)]));
  }
  return value;
}

function digest(value) {
  return crypto.createHash("sha256").update(JSON.stringify(canonical(value))).digest("hex");
}

export function loadQoderCapabilityExtension(file = extensionPath) {
  const bytes = fs.readFileSync(file);
  assert(bytes.length <= 32 * 1024, "Qoder capability fixture exceeds 32 KiB");
  return JSON.parse(bytes.toString("utf8"));
}

export function auditQoderCapabilityExtension(document) {
  assert.equal(document.format, "cc-switch-qoder-capability-extension");
  assert.equal(document.schemaVersion, 1);
  assert.equal(document.driverContractRevision, 3);
  assert.equal(document.liveState, "live_pending", "capability evidence must remain live_pending");
  assert.deepEqual(Object.keys(document).sort(), [
    "capturedAt", "driverContractRevision", "format", "liveState", "observations",
    "officialPackages", "schemaVersion", "sourcePolicy",
  ].sort(), "capability evidence cannot gain external inputs or unreviewed metadata");
  assert.equal(document.sourcePolicy,
    "pinned official archives inspected read-only; no CLI bootstrap, installation or external runtime/test dependency");
  assert.equal(digest(document.officialPackages), frozenPackages,
    "official package/function evidence drifted from independent digest");
  assert.equal(digest(document.observations), frozenObservations,
    "official observations drifted from independent digest");
  const oracle = loadQoderCliOracle();
  auditQoderCliOracle(oracle);
  for (const pkg of document.officialPackages) {
    const old = oracle.source.packages.find((candidate) => candidate.site === pkg.site);
    assert.equal(pkg.version, old?.version);
    assert.equal(pkg.bundleSha256, old?.bundleSha256);
  }
  return { packages: 2, observations: 3, revision: 3, liveState: "live_pending" };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  console.log("Qoder capability extension ok:", auditQoderCapabilityExtension(loadQoderCapabilityExtension()));
}
