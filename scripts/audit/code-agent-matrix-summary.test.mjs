import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const script = path.join(
  repoRoot,
  "scripts/smoke/code-agent-matrix-summary.mjs",
);

function runSummary(matrixPath, evidencePath, overrides = {}) {
  const environment = { ...process.env, ...overrides };
  for (const [name, value] of Object.entries(environment)) {
    if (value === undefined) delete environment[name];
  }
  const args = [script, matrixPath];
  if (evidencePath) args.push(evidencePath);
  return JSON.parse(
    execFileSync(process.execPath, args, {
      cwd: repoRoot,
      env: environment,
      encoding: "utf8",
    }),
  );
}

function writeJson(directory, name, value) {
  const file = path.join(directory, name);
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
  return file;
}

test("share matrix cases require the Router Share URL", () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "cc-switch-matrix-"));
  const matrixPath = writeJson(directory, "matrix.json", {
    schemaVersion: 2,
    requiredFixtureFields: ["non_stream", "stream"],
    cases: [
      {
        id: "claude-share",
        app: "claude",
        source: "router_share",
        entryPath: "/v1/messages",
        urlEnv: "CC_SWITCH_SHARE_URL",
        requiresRouterToken: true,
      },
    ],
  });

  const summary = runSummary(matrixPath, "", {
    ROUTER_API_TOKEN: "router-token",
    CC_SWITCH_SHARE_URL: undefined,
  });

  assert.equal(summary.runnable, 0);
  assert.deepEqual(summary.cases[0].missing, ["CC_SWITCH_SHARE_URL"]);
  assert.equal(summary.matrixInputComplete, false);
  assert.equal(summary.cases[0].blockerGroup, "missing-matrix-input");
  assert.equal(summary.cases[0].liveBlockerGroup, "missing-matrix-input");
});

test("live fixture evidence is complete only when every required field passed", () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "cc-switch-matrix-"));
  const matrixPath = writeJson(directory, "matrix.json", {
    schemaVersion: 2,
    requiredFixtureFields: ["non_stream", "stream"],
    cases: [
      {
        id: "codex-direct",
        app: "codex",
        source: "direct",
        entryPath: "/v1/responses",
        urlEnv: "CC_SWITCH_SHARE_URL",
        requiresRouterToken: true,
      },
    ],
  });
  const incompleteEvidence = writeJson(directory, "incomplete.json", {
    schemaVersion: 1,
    cases: {
      "codex-direct": {
        evidencePath: "/private/codex-direct.json",
        checks: { non_stream: "passed" },
      },
    },
  });
  const environment = {
    ROUTER_API_TOKEN: "router-token",
    CC_SWITCH_SHARE_URL: "https://share.example",
  };

  const incomplete = runSummary(matrixPath, incompleteEvidence, environment);
  assert.equal(incomplete.fixtureEvidenceComplete, false);
  assert.deepEqual(incomplete.cases[0].missingFixtureFields, ["stream"]);
  assert.equal(
    incomplete.cases[0].liveBlockerGroup,
    "missing-live-fixture-evidence",
  );

  const completeEvidence = writeJson(directory, "complete.json", {
    schemaVersion: 1,
    cases: {
      "codex-direct": {
        evidencePath: "/private/codex-direct.json",
        checks: { non_stream: "passed", stream: "passed" },
      },
    },
  });
  const complete = runSummary(matrixPath, completeEvidence, environment);
  assert.equal(complete.fixtureEvidenceComplete, true);
  assert.equal(complete.cases[0].liveBlockerGroup, "");
});
