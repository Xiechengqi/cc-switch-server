import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const envCheckScript = path.join(
  repoRoot,
  "scripts/smoke/real-acceptance-env-check.sh",
);

function runEnvCheck(overrides) {
  const directory = fs.mkdtempSync(
    path.join(os.tmpdir(), "cc-switch-real-env-check-"),
  );
  const evidenceFile = path.join(directory, "evidence.json");
  const result = spawnSync("bash", [envCheckScript], {
    cwd: repoRoot,
    env: {
      ...process.env,
      STAGE: "AB6",
      STRICT: "0",
      EVIDENCE_FILE: evidenceFile,
      GROK_OAUTH_TEST_ACCOUNT: "",
      CC_SWITCH_BASE_URL: "",
      CC_SWITCH_INFERENCE_TOKEN: "",
      CC_SWITCH_GROK_ROUTE_KEY: "",
      CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT: "",
      CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT: "",
      CC_SWITCH_CODEX_IMAGES_SMOKE: "0",
      CC_SWITCH_CODEX_ROUTE_KEY: "",
      ...overrides,
    },
    encoding: "utf8",
  });

  try {
    assert.equal(result.status, 0, result.stderr);
    return JSON.parse(fs.readFileSync(evidenceFile, "utf8"));
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
}

test("Claude Max and Grok external input gates remain isolated", () => {
  const grokReady = runEnvCheck({
    GROK_OAUTH_TEST_ACCOUNT: "grok-test-account",
    CC_SWITCH_BASE_URL: "http://127.0.0.1:15721",
    CC_SWITCH_INFERENCE_TOKEN: "test-inference-token",
    CC_SWITCH_GROK_ROUTE_KEY: "grok-route",
  });
  assert.equal(grokReady.checks.grokGateStatus, "inputs-ready");
  assert.equal(grokReady.checks.claudeMax5xGateStatus, "blocked-inputs");
  assert.equal(grokReady.checks.claudeMax20xGateStatus, "blocked-inputs");

  const maxReady = runEnvCheck({
    CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT: "max-5x-test-account",
    CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT: "max-20x-test-account",
  });
  assert.equal(maxReady.checks.grokGateStatus, "blocked-inputs");
  assert.equal(maxReady.checks.claudeMax5xGateStatus, "inputs-ready");
  assert.equal(maxReady.checks.claudeMax20xGateStatus, "inputs-ready");
});

test("Codex Images gate distinguishes disabled, blocked, and input-ready states", () => {
  const disabled = runEnvCheck({ STAGE: "AB5" });
  assert.equal(disabled.checks.codexImagesGateStatus, "disabled");

  const blocked = runEnvCheck({
    STAGE: "AB5",
    CC_SWITCH_CODEX_IMAGES_SMOKE: "1",
  });
  assert.equal(blocked.checks.codexImagesGateStatus, "blocked-inputs");

  const ready = runEnvCheck({
    STAGE: "AB5",
    CC_SWITCH_CODEX_IMAGES_SMOKE: "1",
    CC_SWITCH_BASE_URL: "https://images.example.test",
    CC_SWITCH_INFERENCE_TOKEN: "test-inference-token",
    CC_SWITCH_CODEX_ROUTE_KEY: "codex-images-route",
  });
  assert.equal(ready.checks.codexImagesGateStatus, "inputs-ready");
});
