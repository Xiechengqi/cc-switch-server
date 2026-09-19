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
      CLAUDE_OAUTH_TEST_ACCOUNT: "",
      CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT: "",
      CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT: "",
      CC_SWITCH_CLAUDE_OAUTH_PROVIDER_ID: "",
      CC_SWITCH_CLAUDE_OAUTH_SHARE_ID: "",
      CC_SWITCH_CLAUDE_OAUTH_MODEL: "",
      CLAUDE_OAUTH_INFERENCE_REAL_RECEIPT_FILE: "",
      CC_SWITCH_CLAUDE_MAX_5X_PROVIDER_ID: "",
      CC_SWITCH_CLAUDE_MAX_5X_SHARE_ID: "",
      CC_SWITCH_CLAUDE_MAX_5X_MODEL: "",
      CLAUDE_MAX_5X_REAL_RECEIPT_FILE: "",
      CC_SWITCH_CLAUDE_MAX_20X_PROVIDER_ID: "",
      CC_SWITCH_CLAUDE_MAX_20X_SHARE_ID: "",
      CC_SWITCH_CLAUDE_MAX_20X_MODEL: "",
      CLAUDE_MAX_20X_REAL_RECEIPT_FILE: "",
      CC_SWITCH_CLAUDE_FABLE_5_1_PROVIDER_ID: "",
      CC_SWITCH_CLAUDE_FABLE_5_1_SHARE_ID: "",
      CC_SWITCH_CLAUDE_FABLE_5_1_MODEL: "",
      CLAUDE_FABLE_5_1_REAL_RECEIPT_FILE: "",
      ANTIGRAVITY_OAUTH_TEST_ACCOUNT: "",
      CC_SWITCH_ANTIGRAVITY_OAUTH_SHARE_ID: "",
      CC_SWITCH_ANTIGRAVITY_OAUTH_CLAUDE_PROVIDER_ID: "",
      CC_SWITCH_ANTIGRAVITY_OAUTH_GEMINI_PROVIDER_ID: "",
      CC_SWITCH_ANTIGRAVITY_OAUTH_CLAUDE_MODEL: "",
      CC_SWITCH_ANTIGRAVITY_OAUTH_GEMINI_MODEL: "",
      ANTIGRAVITY_OAUTH_REAL_RECEIPT_FILE: "",
      AGY_OAUTH_TEST_ACCOUNT: "",
      CC_SWITCH_AGY_OAUTH_SHARE_ID: "",
      CC_SWITCH_AGY_OAUTH_CLAUDE_PROVIDER_ID: "",
      CC_SWITCH_AGY_OAUTH_GEMINI_PROVIDER_ID: "",
      CC_SWITCH_AGY_OAUTH_CLAUDE_MODEL: "",
      CC_SWITCH_AGY_OAUTH_GEMINI_MODEL: "",
      AGY_OAUTH_REAL_RECEIPT_FILE: "",
      CC_SWITCH_CODEX_IMAGES_SMOKE: "0",
      CODEX_OAUTH_TEST_ACCOUNT: "",
      CC_SWITCH_CODEX_GPT_IMAGE_2_5_PROVIDER_ID: "",
      CC_SWITCH_CODEX_GPT_IMAGE_2_5_SHARE_ID: "",
      CC_SWITCH_CODEX_GPT_IMAGE_2_5_MODEL: "",
      CODEX_GPT_IMAGE_2_5_REAL_RECEIPT_FILE: "",
      CC_SWITCH_CODEX_GPT_IMAGE_2_5_FLARE_PROVIDER_ID: "",
      CC_SWITCH_CODEX_GPT_IMAGE_2_5_FLARE_SHARE_ID: "",
      CC_SWITCH_CODEX_GPT_IMAGE_2_5_FLARE_MODEL: "",
      CODEX_GPT_IMAGE_2_5_FLARE_REAL_RECEIPT_FILE: "",
      CC_SWITCH_CODEX_GPT_IMAGE_2_5_SUNBURST_PROVIDER_ID: "",
      CC_SWITCH_CODEX_GPT_IMAGE_2_5_SUNBURST_SHARE_ID: "",
      CC_SWITCH_CODEX_GPT_IMAGE_2_5_SUNBURST_MODEL: "",
      CODEX_GPT_IMAGE_2_5_SUNBURST_REAL_RECEIPT_FILE: "",
      CC_SWITCH_CODEX_WS_PREWARM_PROVIDER_ID: "",
      CC_SWITCH_CODEX_WS_PREWARM_SHARE_ID: "",
      CC_SWITCH_CODEX_WS_PREWARM_MODEL: "",
      CODEX_WS_PREWARM_REAL_RECEIPT_FILE: "",
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

test("Claude operation receipts and Grok external input gates remain isolated", () => {
  const grokReady = runEnvCheck({
    GROK_OAUTH_TEST_ACCOUNT: "grok-test-account",
    CC_SWITCH_SHARE_URL: "https://grok-share.example.test",
    ROUTER_API_TOKEN: "router-token",
  });
  assert.equal(grokReady.checks.grokGateStatus, "inputs-ready");
  assert.equal(grokReady.checks.claudeOauthInferenceGateStatus, "blocked-inputs");
  assert.equal(grokReady.checks.claudeMax5xGateStatus, "blocked-inputs");
  assert.equal(grokReady.checks.claudeMax20xGateStatus, "blocked-inputs");
  assert.equal(grokReady.checks.claudeFable51GateStatus, "blocked-inputs");

  const maxReady = runEnvCheck({
    SERVER_URL: "https://server.example.test",
    CC_SWITCH_SERVER_TOKEN: "server-token",
    CC_SWITCH_SHARE_URL: "https://share.example.test",
    ROUTER_API_TOKEN: "router-token",
    CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT: "max-5x-test-account",
    CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT: "max-20x-test-account",
    CC_SWITCH_CLAUDE_MAX_5X_PROVIDER_ID: "max-5x-provider",
    CC_SWITCH_CLAUDE_MAX_5X_SHARE_ID: "max-5x-share",
    CC_SWITCH_CLAUDE_MAX_5X_MODEL: "claude-sonnet-4-6",
    CLAUDE_MAX_5X_REAL_RECEIPT_FILE: "/tmp/claude-max-5x.json",
    CC_SWITCH_CLAUDE_MAX_20X_PROVIDER_ID: "max-20x-provider",
    CC_SWITCH_CLAUDE_MAX_20X_SHARE_ID: "max-20x-share",
    CC_SWITCH_CLAUDE_MAX_20X_MODEL: "claude-sonnet-4-6",
    CLAUDE_MAX_20X_REAL_RECEIPT_FILE: "/tmp/claude-max-20x.json",
  });
  assert.equal(maxReady.checks.grokGateStatus, "blocked-inputs");
  assert.equal(maxReady.checks.claudeOauthInferenceGateStatus, "blocked-inputs");
  assert.equal(maxReady.checks.claudeMax5xGateStatus, "inputs-ready");
  assert.equal(maxReady.checks.claudeMax20xGateStatus, "inputs-ready");
  assert.equal(maxReady.checks.claudeFable51GateStatus, "blocked-inputs");
  assert.equal(maxReady.longTailInputsPresent.claudeMax5xProviderId, true);
  assert.equal(maxReady.longTailInputsPresent.claudeMax20xReceiptFile, true);
});

test("Antigravity and Agy external gates require independent bindings and receipts", () => {
  const common = {
    STAGE: "AB6",
    SERVER_URL: "https://server.example.test",
    CC_SWITCH_SERVER_TOKEN: "server-token",
    CC_SWITCH_SHARE_URL: "https://share.example.test",
    ROUTER_API_TOKEN: "router-token",
  };
  const antigravity = runEnvCheck({
    ...common,
    ANTIGRAVITY_OAUTH_TEST_ACCOUNT: "antigravity-account",
    CC_SWITCH_ANTIGRAVITY_OAUTH_SHARE_ID: "antigravity-share",
    CC_SWITCH_ANTIGRAVITY_OAUTH_CLAUDE_PROVIDER_ID: "antigravity-claude",
    CC_SWITCH_ANTIGRAVITY_OAUTH_GEMINI_PROVIDER_ID: "antigravity-gemini",
    CC_SWITCH_ANTIGRAVITY_OAUTH_CLAUDE_MODEL: "claude-sonnet-4-6",
    CC_SWITCH_ANTIGRAVITY_OAUTH_GEMINI_MODEL: "gemini-3.5-flash-medium",
    ANTIGRAVITY_OAUTH_REAL_RECEIPT_FILE: "/tmp/antigravity-receipt.json",
  });
  assert.equal(antigravity.checks.antigravityOauthGateStatus, "inputs-ready");
  assert.equal(antigravity.checks.agyOauthGateStatus, "blocked-inputs");

  const agy = runEnvCheck({
    ...common,
    AGY_OAUTH_TEST_ACCOUNT: "agy-account",
    CC_SWITCH_AGY_OAUTH_SHARE_ID: "agy-share",
    CC_SWITCH_AGY_OAUTH_CLAUDE_PROVIDER_ID: "agy-claude",
    CC_SWITCH_AGY_OAUTH_GEMINI_PROVIDER_ID: "agy-gemini",
    CC_SWITCH_AGY_OAUTH_CLAUDE_MODEL: "claude-sonnet-4-6",
    CC_SWITCH_AGY_OAUTH_GEMINI_MODEL: "gemini-3.5-flash-medium",
    AGY_OAUTH_REAL_RECEIPT_FILE: "/tmp/agy-receipt.json",
  });
  assert.equal(agy.checks.antigravityOauthGateStatus, "blocked-inputs");
  assert.equal(agy.checks.agyOauthGateStatus, "inputs-ready");
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

test("Codex operation receipts remain independently gated", () => {
  const common = {
    STAGE: "AB5",
    CODEX_OAUTH_TEST_ACCOUNT: "codex-test-account",
    SERVER_URL: "https://server.example.test",
    CC_SWITCH_SERVER_TOKEN: "server-token",
    CC_SWITCH_SHARE_URL: "https://share.example.test",
    ROUTER_API_TOKEN: "router-token",
  };
  const image = runEnvCheck({
    ...common,
    CC_SWITCH_CODEX_GPT_IMAGE_2_5_PROVIDER_ID: "image-provider",
    CC_SWITCH_CODEX_GPT_IMAGE_2_5_SHARE_ID: "image-share",
    CC_SWITCH_CODEX_GPT_IMAGE_2_5_MODEL: "gpt-image-2.5",
    CODEX_GPT_IMAGE_2_5_REAL_RECEIPT_FILE: "/tmp/codex-image-2-5.json",
  });
  assert.equal(image.checks.codexGptImage25GateStatus, "inputs-ready");
  assert.equal(image.checks.codexGptImage25FlareGateStatus, "blocked-inputs");
  assert.equal(image.checks.codexGptImage25SunburstGateStatus, "blocked-inputs");
  assert.equal(image.checks.codexWsPrewarmGateStatus, "blocked-inputs");
  assert.equal(image.longTailInputsPresent.codexGptImage25ReceiptFile, true);

  const websocket = runEnvCheck({
    ...common,
    CC_SWITCH_CODEX_WS_PREWARM_PROVIDER_ID: "ws-provider",
    CC_SWITCH_CODEX_WS_PREWARM_SHARE_ID: "ws-share",
    CC_SWITCH_CODEX_WS_PREWARM_MODEL: "gpt-5.4-mini",
    CODEX_WS_PREWARM_REAL_RECEIPT_FILE: "/tmp/codex-ws-prewarm.json",
  });
  assert.equal(websocket.checks.codexGptImage25GateStatus, "blocked-inputs");
  assert.equal(websocket.checks.codexWsPrewarmGateStatus, "inputs-ready");
  assert.equal(websocket.longTailInputsPresent.codexWsPrewarmReceiptFile, true);
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
