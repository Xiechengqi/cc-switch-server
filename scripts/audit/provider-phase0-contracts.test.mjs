import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  classifyPresetPointer,
  validatePhase0Contracts,
} from "./audit-provider-phase0-contracts.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

function contract(name) {
  return JSON.parse(fs.readFileSync(path.join(repoRoot, "assets/contract", name), "utf8"));
}

function contracts() {
  return {
    fields: contract("provider-field-consumption.json"),
    behavior: contract("provider-legacy-behavior.json"),
    writers: contract("provider-writer-inventory.json"),
    compatibility: contract("provider-compatibility-window.json"),
    router: contract("router-provider-channel-baseline.json"),
  };
}

test("checked-in Phase 0 Provider contracts satisfy the reviewed shape", () => {
  assert.doesNotThrow(() => validatePhase0Contracts(contracts()));
});

test("preset pointer classification fails closed for a new field", () => {
  assert.throws(
    () => classifyPresetPointer("/newExecutableTemplate"),
    /unclassified preset pointer/,
  );
});

test("field ledger rejects partial runtime and secret classifications", () => {
  const missingOwner = contracts();
  const runtime = missingOwner.fields.persistedFields.find(
    (entry) => entry.classification === "runtime",
  );
  delete runtime.targetOwner;
  assert.throws(() => validatePhase0Contracts(missingOwner), /unclassified Provider field/);

  const missingReader = contracts();
  const secret = missingReader.fields.persistedFields.find((entry) => entry.secret);
  secret.reader = [];
  assert.throws(
    () => validatePhase0Contracts(missingReader),
    /runtime\/secret field lacks reader evidence/,
  );
});

test("writer and Router inventories reject omitted or optimistic facts", () => {
  const missingWriter = contracts();
  missingWriter.writers.entries = missingWriter.writers.entries.filter(
    (entry) => entry.id !== "backup-restore",
  );
  assert.throws(
    () => validatePhase0Contracts(missingWriter),
    /missing Provider writer inventory entry: backup-restore/,
  );

  const optimisticRouter = contracts();
  optimisticRouter.router.facts.appAvailabilityPersistedByRuntimeSnapshot = true;
  assert.throws(
    () => validatePhase0Contracts(optimisticRouter),
    /Router baseline facts are incomplete/,
  );
});

test("writer inventory is bound to reviewed behavior and current source", () => {
  const staleBehavior = contracts();
  staleBehavior.writers.entries.find(
    (entry) => entry.id === "rest-create",
  ).currentBehavior = "raw Provider upsert; mutates live before save";
  assert.throws(
    () => validatePhase0Contracts(staleBehavior),
    /stale Provider writer inventory/,
  );

  const staleSource = contracts();
  staleSource.writers.currentSourceEvidence.find(
    (entry) => entry.path === "src/api/providers.rs",
  ).sha256 = "0".repeat(64);
  assert.throws(
    () => validatePhase0Contracts(staleSource),
    /stale Provider writer source evidence/,
  );

  const optimisticClosure = contracts();
  optimisticClosure.writers.entries.find(
    (entry) => entry.id === "delete-cascade",
  ).closureStatus = "done";
  assert.throws(
    () => validatePhase0Contracts(optimisticClosure),
    /invalid Provider writer closure status/,
  );
});

test("legacy readers remain held until the full release window is evidenced", () => {
  const earlyRemoval = contracts();
  earlyRemoval.compatibility.policy.removalEligible = true;
  assert.throws(
    () => validatePhase0Contracts(earlyRemoval),
    /cannot be removed without a completed release window/,
  );

  const missingReader = contracts();
  missingReader.compatibility.entries = missingReader.compatibility.entries.filter(
    (entry) => entry.id !== "legacy-provider-classifier",
  );
  assert.throws(
    () => validatePhase0Contracts(missingReader),
    /stale Provider compatibility inventory/,
  );
});
