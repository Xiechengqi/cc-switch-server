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
      CC_SWITCH_SHARE_URL: "",
      ROUTER_API_TOKEN: "",
      CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT: "",
      CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT: "",
      CC_SWITCH_CODEX_IMAGES_SMOKE: "0",
      GITHUB_COPILOT_TEST_ACCOUNT: "",
      CC_SWITCH_COPILOT_CLAUDE_PROVIDER_ID: "",
      CC_SWITCH_COPILOT_CODEX_PROVIDER_ID: "",
      CC_SWITCH_COPILOT_GEMINI_PROVIDER_ID: "",
      QODER_GLOBAL_OAUTH_TEST_ACCOUNT: "",
      CC_SWITCH_QODER_GLOBAL_OAUTH_CLAUDE_PROVIDER_ID: "",
      CC_SWITCH_QODER_GLOBAL_OAUTH_CODEX_PROVIDER_ID: "",
      CC_SWITCH_QODER_GLOBAL_OAUTH_GEMINI_PROVIDER_ID: "",
      QODER_GLOBAL_PAT_TEST_ACCOUNT: "",
      CC_SWITCH_QODER_GLOBAL_PAT_CLAUDE_PROVIDER_ID: "",
      CC_SWITCH_QODER_GLOBAL_PAT_CODEX_PROVIDER_ID: "",
      CC_SWITCH_QODER_GLOBAL_PAT_GEMINI_PROVIDER_ID: "",
      QODER_CN_OAUTH_TEST_ACCOUNT: "",
      CC_SWITCH_QODER_CN_OAUTH_CLAUDE_PROVIDER_ID: "",
      CC_SWITCH_QODER_CN_OAUTH_CODEX_PROVIDER_ID: "",
      CC_SWITCH_QODER_CN_OAUTH_GEMINI_PROVIDER_ID: "",
      AMAZON_Q_TEST_ACCOUNT: "",
      CC_SWITCH_AMAZON_Q_CLAUDE_PROVIDER_ID: "",
      CC_SWITCH_AMAZON_Q_CODEX_PROVIDER_ID: "",
      CC_SWITCH_SERVER_TOKEN: "",
      SERVER_URL: "",
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
    CC_SWITCH_SHARE_URL: "https://grok-share.example.test",
    ROUTER_API_TOKEN: "router-token",
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
    CC_SWITCH_SHARE_URL: "https://images-share.example.test",
    ROUTER_API_TOKEN: "router-token",
  });
  assert.equal(ready.checks.codexImagesGateStatus, "inputs-ready");
});

test("Copilot external gate requires one account, control plane, Share, and three Provider IDs", () => {
  const blocked = runEnvCheck({
    STAGE: "AB7",
    GITHUB_COPILOT_TEST_ACCOUNT: "copilot-test-account",
    CC_SWITCH_SHARE_URL: "https://copilot-share.example.test",
    ROUTER_API_TOKEN: "router-token",
  });
  assert.equal(blocked.checks.copilotGateStatus, "blocked-inputs");

  const ready = runEnvCheck({
    STAGE: "AB7",
    GITHUB_COPILOT_TEST_ACCOUNT: "copilot-test-account",
    SERVER_URL: "https://server.example.test",
    CC_SWITCH_SERVER_TOKEN: "server-token",
    CC_SWITCH_SHARE_URL: "https://copilot-share.example.test",
    ROUTER_API_TOKEN: "router-token",
    CC_SWITCH_COPILOT_CLAUDE_PROVIDER_ID: "provider-claude",
    CC_SWITCH_COPILOT_CODEX_PROVIDER_ID: "provider-codex",
    CC_SWITCH_COPILOT_GEMINI_PROVIDER_ID: "provider-gemini",
  });
  assert.equal(ready.checks.copilotGateStatus, "inputs-ready");
  assert.equal(ready.longTailInputsPresent.githubCopilotClaudeProviderId, true);
  assert.equal(ready.longTailInputsPresent.githubCopilotCodexProviderId, true);
  assert.equal(ready.longTailInputsPresent.githubCopilotGeminiProviderId, true);
});

test("Qoder external gates keep the three credential rails independent", () => {
  const common = {
    STAGE: "AB7",
    SERVER_URL: "https://server.example.test",
    CC_SWITCH_SERVER_TOKEN: "server-token",
    CC_SWITCH_SHARE_URL: "https://qoder-share.example.test",
    ROUTER_API_TOKEN: "router-token",
  };
  const globalOauth = runEnvCheck({
    ...common,
    QODER_GLOBAL_OAUTH_TEST_ACCOUNT: "qoder-global-oauth-account",
    CC_SWITCH_QODER_GLOBAL_OAUTH_CLAUDE_PROVIDER_ID: "qoder-global-oauth-claude",
    CC_SWITCH_QODER_GLOBAL_OAUTH_CODEX_PROVIDER_ID: "qoder-global-oauth-codex",
    CC_SWITCH_QODER_GLOBAL_OAUTH_GEMINI_PROVIDER_ID: "qoder-global-oauth-gemini",
  });
  assert.equal(globalOauth.checks.qoderGlobalOauthGateStatus, "inputs-ready");
  assert.equal(globalOauth.checks.qoderGlobalPatGateStatus, "blocked-inputs");
  assert.equal(globalOauth.checks.qoderCnOauthGateStatus, "blocked-inputs");

  const globalPatAndCn = runEnvCheck({
    ...common,
    QODER_GLOBAL_PAT_TEST_ACCOUNT: "qoder-global-pat-account",
    CC_SWITCH_QODER_GLOBAL_PAT_CLAUDE_PROVIDER_ID: "qoder-global-pat-claude",
    CC_SWITCH_QODER_GLOBAL_PAT_CODEX_PROVIDER_ID: "qoder-global-pat-codex",
    CC_SWITCH_QODER_GLOBAL_PAT_GEMINI_PROVIDER_ID: "qoder-global-pat-gemini",
    QODER_CN_OAUTH_TEST_ACCOUNT: "qoder-cn-oauth-account",
    CC_SWITCH_QODER_CN_OAUTH_CLAUDE_PROVIDER_ID: "qoder-cn-oauth-claude",
    CC_SWITCH_QODER_CN_OAUTH_CODEX_PROVIDER_ID: "qoder-cn-oauth-codex",
    CC_SWITCH_QODER_CN_OAUTH_GEMINI_PROVIDER_ID: "qoder-cn-oauth-gemini",
  });
  assert.equal(globalPatAndCn.checks.qoderGlobalOauthGateStatus, "blocked-inputs");
  assert.equal(globalPatAndCn.checks.qoderGlobalPatGateStatus, "inputs-ready");
  assert.equal(globalPatAndCn.checks.qoderCnOauthGateStatus, "inputs-ready");
});

test("Amazon Q external gate is independent from Kiro and requires two explicit Provider IDs", () => {
  const blocked = runEnvCheck({
    STAGE: "AB7",
    AMAZON_Q_TEST_ACCOUNT: "amazon-q-test-account",
    SERVER_URL: "https://server.example.test",
    CC_SWITCH_SERVER_TOKEN: "server-token",
    CC_SWITCH_SHARE_URL: "https://amazon-q-share.example.test",
    ROUTER_API_TOKEN: "router-token",
    KIRO_TEST_ACCOUNT: "kiro-decoy-account",
  });
  assert.equal(blocked.checks.amazonQGateStatus, "blocked-inputs");

  const ready = runEnvCheck({
    STAGE: "AB7",
    AMAZON_Q_TEST_ACCOUNT: "amazon-q-test-account",
    SERVER_URL: "https://server.example.test",
    CC_SWITCH_SERVER_TOKEN: "server-token",
    CC_SWITCH_SHARE_URL: "https://amazon-q-share.example.test",
    ROUTER_API_TOKEN: "router-token",
    CC_SWITCH_AMAZON_Q_CLAUDE_PROVIDER_ID: "amazon-q-claude",
    CC_SWITCH_AMAZON_Q_CODEX_PROVIDER_ID: "amazon-q-codex",
  });
  assert.equal(ready.checks.amazonQGateStatus, "inputs-ready");
  assert.equal(ready.longTailInputsPresent.amazonQClaudeProviderId, true);
  assert.equal(ready.longTailInputsPresent.amazonQCodexProviderId, true);
});
