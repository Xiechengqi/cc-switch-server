#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { execFileSync } from "node:child_process";

const root = path.resolve(new URL("../..", import.meta.url).pathname);
const baseline = JSON.parse(
  fs.readFileSync(path.join(root, "assets/contract/antigravity-reference-delta.json"), "utf8"),
);
const checkSources = process.argv.includes("--check-sources");
const assert = (condition, message) => {
  if (!condition) throw new Error(message);
};
const sha256 = (value) => crypto.createHash("sha256").update(value).digest("hex");

assert(
  baseline.format === "cc-switch-antigravity-reference-delta" &&
    baseline.schemaVersion === 1,
  "Antigravity reference delta format changed",
);
assert(
  baseline.policy?.externalSources ===
    "read_only_optional_audit_input_never_runtime_dependency" &&
    baseline.policy?.scope?.includes("no cross-account fallback") &&
    baseline.policy?.scope?.includes("no cross-Provider fallback") &&
    baseline.policy?.recoveryBoundary?.includes("pre-commit"),
  "Antigravity fixed-binding or external-source boundary changed",
);

const sourceDeltaIds = new Set();
for (const source of baseline.sources ?? []) {
  assert(source.id && source.rootEnv && source.defaultRelativeRoot, "Antigravity source metadata is incomplete");
  const sourceRoot = path.resolve(root, process.env[source.rootEnv] || source.defaultRelativeRoot);
  for (const delta of source.deltas ?? []) {
    assert(!sourceDeltaIds.has(delta.id), `duplicate Antigravity delta ${delta.id}`);
    sourceDeltaIds.add(delta.id);
    assert(/^[a-f0-9]{40}$/.test(delta.commit), `${delta.id} has an invalid commit`);
    assert(Array.isArray(delta.files) && delta.files.length > 0, `${delta.id} has no evidence files`);
    for (const file of delta.files) {
      assert(
        file.path && !path.isAbsolute(file.path) && !file.path.split("/").includes(".."),
        `${delta.id} has an unsafe path`,
      );
      assert(/^[a-f0-9]{64}$/.test(file.sha256), `${delta.id}:${file.path} has an invalid SHA-256`);
      if (checkSources) {
        assert(fs.existsSync(sourceRoot), `${source.id} source root is unavailable`);
        const content = execFileSync("git", ["-C", sourceRoot, "show", `${delta.commit}:${file.path}`], {
          encoding: null,
          maxBuffer: 64 * 1024 * 1024,
        });
        assert(sha256(content) === file.sha256, `${source.id}:${delta.id}:${file.path} drifted`);
      }
    }
  }
}

const expectedCapabilities = new Map([
  ["AG-01", "fixture_verified"],
  ["AG-02", "fixture_verified"],
  ["AG-03", "fixture_verified"],
  ["AG-04", "live_pending"],
  ["AG-N1", "fixture_verified"],
  ["AG-N2", "fixture_verified"],
  ["AG-N3", "fixture_verified"],
  ["AG-N4", "fixture_verified"],
  ["AG-N5", "fixture_verified"],
  ["AG-N6", "live_pending"],
]);
for (const capability of baseline.capabilities ?? []) {
  assert(
    expectedCapabilities.get(capability.id) === capability.status,
    `${capability.id} has an unsupported evidence state`,
  );
  expectedCapabilities.delete(capability.id);
  for (const contract of capability.contracts ?? []) {
    const localPath = path.resolve(root, contract.path);
    assert(localPath.startsWith(`${root}${path.sep}`) && fs.existsSync(localPath), `${capability.id} path is unavailable`);
    const source = fs.readFileSync(localPath, "utf8");
    for (const anchor of contract.anchors ?? []) {
      assert(source.includes(anchor), `${capability.id} is missing local anchor ${anchor}`);
    }
  }
  assert((capability.contracts ?? []).length > 0, `${capability.id} has no local contracts`);
}
assert(expectedCapabilities.size === 0, "Antigravity capability coverage is incomplete");
const requestTypeGate = (baseline.capabilities ?? []).find((capability) => capability.id === "AG-N6");
assert(
  requestTypeGate?.runtimeChange === false,
  "Antigravity requestType behavior changed without a real receipt",
);
const rails = baseline.realAcceptance?.rails ?? [];
assert(
  JSON.stringify(rails.map((rail) => rail.rail).sort()) ===
    JSON.stringify(["agy_oauth", "antigravity_oauth"]) &&
    rails.every((rail) => rail.status === "live_pending" && rail.receipt === null),
  "Antigravity real rail gates changed without receipts",
);
assert(
  baseline.realAcceptance?.compaction?.status === "live_pending" &&
    baseline.realAcceptance?.compaction?.runtimeEnabled === false &&
    baseline.realAcceptance?.compaction?.receipt === null,
  "Antigravity compaction gate opened without a receipt",
);

console.log(
  `antigravity reference delta audit ok (${sourceDeltaIds.size} deltas${
    checkSources ? ", external objects verified" : ", external check optional"
  })`,
);
