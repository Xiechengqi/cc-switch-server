#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const readJson = (relativePath) =>
  JSON.parse(fs.readFileSync(path.join(repoRoot, relativePath), "utf8"));
const fail = (message) => {
  throw new Error(message);
};
const assert = (condition, message) => {
  if (!condition) fail(message);
};

const evidence = readJson("assets/contract/provider-conformance-evidence.json");
const registry = readJson("assets/contract/provider-registry.json");
const operations = ["forward", "test", "discovery"];
const levels = ["unsupported", "planned", "implemented", "fixture_verified", "live_verified"];
const liveStates = new Set(["not_applicable", "live_pending", "live_verified"]);

assert(evidence.schemaVersion === 1, "provider conformance evidence schema changed");
assert(
  evidence.authority === "cc-switch-server-provider-conformance-evidence",
  "provider conformance evidence authority changed",
);
assert(
  JSON.stringify(evidence.policy?.verificationOrder) === JSON.stringify(levels),
  "provider conformance verification order changed",
);
assert(
  evidence.policy?.livePendingIsVerificationLevel === false,
  "live_pending must remain a gate rather than a verification level",
);
assert(
  evidence.policy?.liveVerifiedRequiresReceipt === true,
  "live_verified must require an independent receipt",
);

const driverSpecs = new Map(registry.drivers.map((driver) => [driver.driverId, driver]));
const registryConformance = new Map(
  registry.conformance.map((driver) => [driver.driverId, driver]),
);
const seen = new Set();
for (const driver of evidence.drivers ?? []) {
  assert(!seen.has(driver.driverId), `duplicate conformance evidence for ${driver.driverId}`);
  seen.add(driver.driverId);
  const spec = driverSpecs.get(driver.driverId);
  const conformance = registryConformance.get(driver.driverId);
  assert(spec && conformance, `unknown governed driver ${driver.driverId}`);
  for (const operation of operations) {
    const entry = driver.operations?.[operation];
    assert(entry, `${driver.driverId}/${operation} evidence is missing`);
    assert(levels.includes(entry.verificationLevel), `${driver.driverId}/${operation} has invalid verification level`);
    assert(liveStates.has(entry.liveState), `${driver.driverId}/${operation} has invalid live state`);
    assert(Array.isArray(entry.receipts), `${driver.driverId}/${operation} receipts must be an array`);
    assert(
      conformance[operation] === entry.verificationLevel,
      `${driver.driverId}/${operation} registry verification level drifted`,
    );
    const support = spec.operations?.[operation];
    if (support === "unsupported") {
      assert(entry.verificationLevel === "unsupported", `${driver.driverId}/${operation} unsupported level drifted`);
      assert(entry.liveState === "not_applicable", `${driver.driverId}/${operation} unsupported live gate drifted`);
      assert(entry.receipts.length === 0, `${driver.driverId}/${operation} unsupported operation has receipts`);
      continue;
    }
    assert(entry.verificationLevel !== "unsupported", `${driver.driverId}/${operation} supported operation is unsupported`);
    if (entry.verificationLevel === "live_verified" || entry.liveState === "live_verified") {
      assert(entry.verificationLevel === "live_verified" && entry.liveState === "live_verified", `${driver.driverId}/${operation} has split live truth`);
      assert(entry.receipts.length > 0, `${driver.driverId}/${operation} claims live_verified without a receipt`);
    } else {
      assert(entry.liveState === "live_pending", `${driver.driverId}/${operation} must remain live_pending`);
      assert(entry.receipts.length === 0, `${driver.driverId}/${operation} pending gate must not cite a receipt`);
    }
  }
}

for (const required of [
  "special.antigravity",
  "oauth.claude_messages",
  "oauth.openai_codex",
  "special.cursor",
  "oauth.grok_responses",
  "special.kiro",
  "special.qoder_cosy",
  "special.codebuddy_oauth",
]) {
  assert(seen.has(required), `missing requested Provider conformance evidence: ${required}`);
}

const serialized = JSON.stringify(evidence);
for (const forbidden of ["authorization", "cookie", "accessToken", "refreshToken", "prompt", "email", "opaqueReasoning"]) {
  assert(!serialized.toLowerCase().includes(forbidden.toLowerCase()), `conformance evidence contains forbidden field ${forbidden}`);
}

console.log(`Provider conformance evidence ok (${seen.size} governed drivers, all live gates pending)`);
