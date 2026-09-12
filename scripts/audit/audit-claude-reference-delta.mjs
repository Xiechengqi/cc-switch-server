#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { execFileSync } from "node:child_process";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const baselinePath = path.join(
  repoRoot,
  "assets/contract/claude-reference-delta.json",
);
const checkSources = process.argv.includes("--check-sources");

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

const baseline = JSON.parse(fs.readFileSync(baselinePath, "utf8"));
assert(
  baseline.format === "cc-switch-claude-reference-delta" &&
    baseline.schemaVersion === 1,
  "Claude reference delta baseline format changed",
);
assert(
  baseline.policy?.externalSources ===
    "read_only_optional_audit_input_never_runtime_dependency",
  "Claude external-source boundary changed",
);
assert(
  baseline.policy?.scope?.includes("no pool") &&
    baseline.policy.scope.includes("no cross-account fallback") &&
    baseline.policy.scope.includes("no cross-Provider fallback"),
  "Claude fixed-binding scope changed",
);

const sourceDeltaIds = new Set();
for (const source of baseline.sources ?? []) {
  assert(source.id && source.rootEnv, "Claude audit source metadata is incomplete");
  assert(
    typeof source.defaultRelativeRoot === "string" &&
      source.defaultRelativeRoot.length > 0,
    `${source.id} has no default relative root`,
  );
  const sourceRoot = path.resolve(
    repoRoot,
    process.env[source.rootEnv] || source.defaultRelativeRoot,
  );
  for (const delta of source.deltas ?? []) {
    assert(!sourceDeltaIds.has(delta.id), `duplicate Claude delta ${delta.id}`);
    sourceDeltaIds.add(delta.id);
    assert(/^[a-f0-9]{40}$/.test(delta.commit), `${delta.id} has an invalid commit`);
    assert(
      Array.isArray(delta.files) && delta.files.length >= 2,
      `${delta.id} must pin implementation and fixture evidence`,
    );
    for (const file of delta.files) {
      assert(
        typeof file.path === "string" &&
          file.path.length > 0 &&
          !path.isAbsolute(file.path) &&
          !file.path.split("/").includes(".."),
        `${delta.id} has an unsafe evidence path`,
      );
      assert(
        /^[a-f0-9]{64}$/.test(file.sha256),
        `${delta.id}:${file.path} has an invalid SHA-256`,
      );
      if (checkSources) {
        assert(fs.existsSync(sourceRoot), `${source.id} source root is unavailable`);
        const content = execFileSync(
          "git",
          ["-C", sourceRoot, "show", `${delta.commit}:${file.path}`],
          { encoding: null, maxBuffer: 64 * 1024 * 1024 },
        );
        assert(
          sha256(content) === file.sha256,
          `${source.id}:${delta.id}:${file.path} drifted from the reviewed object`,
        );
      }
    }
  }
}

const localIds = new Set();
for (const contract of baseline.localContracts ?? []) {
  assert(!localIds.has(contract.id), `duplicate local Claude contract ${contract.id}`);
  localIds.add(contract.id);
  assert(
    contract.status === "fixture_verified",
    `${contract.id} claims an unsupported evidence state`,
  );
  const localPath = path.resolve(repoRoot, contract.path);
  assert(
    localPath.startsWith(`${repoRoot}${path.sep}`) && fs.existsSync(localPath),
    `${contract.id} local fixture path is unavailable`,
  );
  const source = fs.readFileSync(localPath, "utf8");
  assert(
    Array.isArray(contract.anchors) && contract.anchors.length >= 1,
    `${contract.id} has no local fixture anchors`,
  );
  for (const anchor of contract.anchors) {
    assert(source.includes(anchor), `${contract.id} is missing local anchor ${anchor}`);
  }
}

assert(
  JSON.stringify([...sourceDeltaIds].sort()) ===
    JSON.stringify([...localIds].sort()),
  "Claude reference deltas and local fixture contracts do not match",
);

console.log(
  `claude reference delta audit ok (${localIds.size} contracts${
    checkSources ? ", external objects verified" : ", external check optional"
  })`,
);
