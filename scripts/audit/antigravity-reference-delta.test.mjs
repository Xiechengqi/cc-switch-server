import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { execFileSync, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const auditPath = "scripts/audit/audit-antigravity-reference-delta.mjs";
const evidencePath = "assets/contract/antigravity-reference-delta.json";
const evidence = JSON.parse(fs.readFileSync(path.join(repoRoot, evidencePath), "utf8"));
const gitDir = execFileSync("git", ["-C", repoRoot, "rev-parse", "--path-format=absolute", "--git-common-dir"], { encoding: "utf8" }).trim();

function auditFixture(edit = () => {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "cc-switch-antigravity-history-"));
  try {
    // Read existing objects through alternates; create no commits and never
    // mutate the real repository or any external reference repository.
    execFileSync("git", ["init", "--quiet", root], { stdio: "pipe" });
    fs.writeFileSync(path.join(root, ".git/objects/info/alternates"), `${path.join(gitDir, "objects")}\n`);
    const files = new Set([
      auditPath,
      evidencePath,
      "assets/contract/antigravity-r4-wire-delta.json",
      "src/proxy/antigravity_replay/resident_tests.rs",
      "src/clients/oauth/antigravity_models/resident_tests.rs",
      ...evidence.capabilities.flatMap((entry) => entry.contracts.map((contract) => contract.path)),
      ...evidence.r4Reviews.flatMap((entry) => entry.target.contracts.map((contract) => contract.path)),
    ]);
    for (const file of files) {
      const target = path.join(root, file);
      fs.mkdirSync(path.dirname(target), { recursive: true });
      fs.copyFileSync(path.join(repoRoot, file), target);
    }
    edit(root);
    const env = { ...process.env };
    for (const source of evidence.sources) env[source.rootEnv] = path.join(root, "absent-external-source");
    return spawnSync(process.execPath, [path.join(root, auditPath)], { cwd: root, env, encoding: "utf8", maxBuffer: 4 * 1024 * 1024 });
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
}

function editEvidence(root, edit) {
  const file = path.join(root, evidencePath);
  const value = JSON.parse(fs.readFileSync(file, "utf8"));
  edit(value);
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

test("Antigravity immutable evidence survives replay admission relocation without external sources", () => {
  const source = fs.readFileSync(path.join(repoRoot, "src/proxy/providers/antigravity/mod.rs"), "utf8");
  assert.equal(source.includes("snapshot.retained_bytes()"), false);
  const result = auditFixture();
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /22 immutable observations/);
});

test("Antigravity immutable evidence rejects a mismatched committed target tree", () => {
  const result = auditFixture((root) => editEvidence(root, (value) => {
    value.observations[0].target.implementationTree = "0".repeat(40);
  }));
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /committed target tree drifted/);
});

test("Antigravity historical proof cannot satisfy the current accounted replay contract", () => {
  const result = auditFixture((root) => {
    const file = path.join(root, "src/proxy/providers/antigravity/mod.rs");
    fs.writeFileSync(file, fs.readFileSync(file, "utf8").replaceAll("get_accounted", "unaccounted_lookup"));
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /resident review current contract is unavailable/);
});

test("Antigravity historical observations remain immutable after target lookup correction", () => {
  const result = auditFixture((root) => editEvidence(root, (value) => {
    value.observations[0].reason += " rewritten";
  }));
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /AG-OBS-0001 changed after it was recorded/);
});
