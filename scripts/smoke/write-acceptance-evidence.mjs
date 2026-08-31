#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";

function argValue(name, fallback = "") {
  const index = process.argv.indexOf(name);
  if (index >= 0 && index + 1 < process.argv.length) {
    return process.argv[index + 1];
  }
  return fallback;
}

function env(name, fallback = "") {
  return process.env[name] || fallback;
}

function gitCommit() {
  try {
    return execFileSync("git", ["rev-parse", "--short", "HEAD"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    return "unknown";
  }
}

function redactEmail(value) {
  if (!value || !value.includes("@")) {
    return value || "";
  }
  const [name, domain] = value.split("@");
  const head = name.slice(0, 2);
  return `${head}${"*".repeat(Math.max(1, name.length - 2))}@${domain}`;
}

function nonEmptyObject(object) {
  return Object.fromEntries(
    Object.entries(object).filter(
      ([, value]) => value !== undefined && value !== "",
    ),
  );
}

function assertNoSecrets(serialized) {
  const patterns = [
    /Bearer\s+[A-Za-z0-9._~+/=-]{10,}/,
    /sk-[A-Za-z0-9._-]{10,}/,
    /ya29\.[A-Za-z0-9._-]+/,
    /[A-Za-z0-9_-]*refresh[_-]?token[A-Za-z0-9_-]*["']?\s*[:=]\s*["'][^"']{6,}/i,
    /[A-Za-z0-9_-]*access[_-]?token[A-Za-z0-9_-]*["']?\s*[:=]\s*["'][^"']{6,}/i,
  ];
  const match = patterns.find((pattern) => pattern.test(serialized));
  if (match) {
    console.error(
      `refusing to write evidence; secret-like pattern matched: ${match}`,
    );
    process.exit(3);
  }
}

const output = argValue("--out", env("EVIDENCE_FILE"));
if (!output) {
  console.error("--out or EVIDENCE_FILE is required");
  process.exit(2);
}

const verificationState = env("EVIDENCE_VERIFICATION_STATE", "unknown");
const evidenceTarget = env("EVIDENCE_TARGET");
const verificationStates = new Set([
  "unknown",
  "contract_verified",
  "live_verified",
  "blocked_inputs",
  "failed",
]);
if (!verificationStates.has(verificationState)) {
  console.error(
    `unsupported evidence verification state: ${verificationState}`,
  );
  process.exit(2);
}
if (verificationState === "live_verified" && env("RUN_REAL") !== "1") {
  console.error("refusing to write live_verified evidence without RUN_REAL=1");
  process.exit(2);
}
if (
  verificationState === "live_verified" &&
  evidenceTarget === "code-agent-matrix"
) {
  const requiredGates = {
    FAILURES: "0",
    CONTRACT_FAILURES: "0",
    RUN_CONTRACT_TESTS: "1",
    CONTRACT_TESTS_PASSED: "1",
    STREAM_PROBE: "1",
    REQUIRE_STREAM_USAGE: "1",
    MATRIX_FIXTURE_EVIDENCE_COMPLETE: "true",
    MATRIX_FIXTURE_EVIDENCE_MISSING: "0",
    MATRIX_SKIPPED: "0",
    SKIPPED: "0",
    LIVE_VERIFICATION_COMPLETE: "1",
  };
  const incomplete = Object.entries(requiredGates)
    .filter(([name, expected]) => env(name) !== expected)
    .map(([name]) => name);
  const matrixTotalRaw = env("MATRIX_TOTAL", "0");
  const matrixRunnableRaw = env("MATRIX_RUNNABLE", "0");
  const matrixTotal = Number.parseInt(matrixTotalRaw, 10);
  const matrixRunnable = Number.parseInt(matrixRunnableRaw, 10);
  if (
    !/^\d+$/.test(matrixTotalRaw) ||
    !/^\d+$/.test(matrixRunnableRaw) ||
    !Number.isSafeInteger(matrixTotal) ||
    matrixTotal <= 0 ||
    matrixRunnable !== matrixTotal
  ) {
    incomplete.push("MATRIX_RUNNABLE");
  }
  if (incomplete.length > 0) {
    console.error(
      `refusing to write code-agent live_verified; incomplete gates: ${incomplete.join(",")}`,
    );
    process.exit(2);
  }
}

const evidence = {
  date: new Date().toISOString(),
  stage: env("EVIDENCE_STAGE", env("STAGE", "unknown")),
  status: env("EVIDENCE_STATUS", "unknown"),
  verificationState,
  verificationScope: env("EVIDENCE_VERIFICATION_SCOPE"),
  serverCommit: env("SERVER_COMMIT", gitCommit()),
  target: evidenceTarget,
  source: env("EVIDENCE_SOURCE"),
  app: env("EVIDENCE_APP"),
  provider: env("EVIDENCE_PROVIDER"),
  providerType: env("EVIDENCE_PROVIDER_TYPE"),
  blockerGroup: env("BLOCKER_GROUP"),
  failureClass: env("FAILURE_CLASS"),
  deploymentNotTested: env("DEPLOYMENT_NOT_TESTED"),
  serverUrl: env("SERVER_URL"),
  routerBaseUrl: env("ROUTER_BASE_URL"),
  routerShareUrl: env("CC_SWITCH_SHARE_URL"),
  shareId: env("SHARE_ID"),
  requestId: env("REQUEST_ID"),
  oauthAccounts: nonEmptyObject({
    codex: redactEmail(env("CODEX_OAUTH_TEST_ACCOUNT")),
    claude: redactEmail(env("CLAUDE_OAUTH_TEST_ACCOUNT")),
    claudeMax5x: redactEmail(env("CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT")),
    claudeMax20x: redactEmail(env("CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT")),
    gemini: redactEmail(env("GEMINI_OAUTH_TEST_ACCOUNT")),
    grok: redactEmail(env("GROK_OAUTH_TEST_ACCOUNT")),
    cursor: redactEmail(env("CURSOR_OAUTH_TEST_ACCOUNT")),
    antigravity: redactEmail(env("ANTIGRAVITY_OAUTH_TEST_ACCOUNT")),
    githubCopilot: redactEmail(env("GITHUB_COPILOT_TEST_ACCOUNT")),
    kiro: redactEmail(env("KIRO_TEST_ACCOUNT")),
    amazonQ: redactEmail(env("AMAZON_Q_TEST_ACCOUNT")),
  }),
  streamProbe: env("STREAM_PROBE"),
  probeModel: env("PROBE_MODEL"),
  routerTokenPresent: Boolean(env("ROUTER_API_TOKEN")),
  providerTokensPresent: {
    claude: Boolean(env("CLAUDE_PROVIDER_TOKEN")),
    codex: Boolean(env("CODEX_PROVIDER_TOKEN")),
    gemini: Boolean(env("GEMINI_PROVIDER_TOKEN")),
  },
  oauthFixturesPresent: {
    codex: Boolean(
      env("CODEX_OAUTH_REFRESH_TOKEN_FIXTURE") ||
      env("CODEX_OAUTH_REFRESH_TOKEN"),
    ),
    claude: Boolean(
      env("CLAUDE_OAUTH_REFRESH_TOKEN_FIXTURE") ||
      env("CLAUDE_OAUTH_REFRESH_TOKEN"),
    ),
    gemini: Boolean(
      env("GEMINI_OAUTH_REFRESH_TOKEN_FIXTURE") ||
      env("GEMINI_OAUTH_REFRESH_TOKEN") ||
      env("GEMINI_CLI_CREDENTIALS_FIXTURE"),
    ),
    grok: Boolean(
      env("GROK_OAUTH_REFRESH_TOKEN_FIXTURE") ||
      env("GROK_OAUTH_AUTH_JSON_FIXTURE"),
    ),
    cursor: Boolean(
      env("CURSOR_OAUTH_REFRESH_TOKEN_FIXTURE") ||
      env("CURSOR_API_KEY_FIXTURE"),
    ),
    antigravity: Boolean(env("ANTIGRAVITY_OAUTH_REFRESH_TOKEN_FIXTURE")),
    githubCopilot: Boolean(env("GITHUB_COPILOT_TOKEN_FIXTURE")),
    kiro: Boolean(env("KIRO_REFRESH_TOKEN_FIXTURE")),
    amazonQ: Boolean(env("AMAZON_Q_REFRESH_TOKEN_FIXTURE")),
  },
  longTailInputsPresent: {
    cursorOAuthAccount: Boolean(env("CURSOR_OAUTH_TEST_ACCOUNT")),
    cursorCallbackUrl: Boolean(env("CURSOR_OAUTH_CALLBACK_URL")),
    githubCopilotAccount: Boolean(env("GITHUB_COPILOT_TEST_ACCOUNT")),
    githubCopilotDomain: env("GITHUB_COPILOT_GITHUB_DOMAIN"),
    githubCopilotClaudeProviderId: Boolean(env("CC_SWITCH_COPILOT_CLAUDE_PROVIDER_ID")),
    githubCopilotCodexProviderId: Boolean(env("CC_SWITCH_COPILOT_CODEX_PROVIDER_ID")),
    githubCopilotGeminiProviderId: Boolean(env("CC_SWITCH_COPILOT_GEMINI_PROVIDER_ID")),
    kiroAccount: Boolean(env("KIRO_TEST_ACCOUNT")),
    kiroRegion: env("KIRO_REGION"),
    kiroStartUrl: env("KIRO_START_URL"),
    amazonQAccount: Boolean(env("AMAZON_Q_TEST_ACCOUNT")),
    amazonQClaudeProviderId: Boolean(env("CC_SWITCH_AMAZON_Q_CLAUDE_PROVIDER_ID")),
    amazonQCodexProviderId: Boolean(env("CC_SWITCH_AMAZON_Q_CODEX_PROVIDER_ID")),
    amazonQModel: env("CC_SWITCH_AMAZON_Q_MODEL"),
    amazonQRuntimeRegion: env("CC_SWITCH_AMAZON_Q_RUNTIME_REGION"),
    amazonQProfileArnPresent: Boolean(env("CC_SWITCH_AMAZON_Q_PROFILE_ARN")),
    bedrockRegion: env("AWS_REGION"),
    bedrockAccessKeyPresent: Boolean(env("AWS_ACCESS_KEY_ID")),
    bedrockSecretKeyPresent: Boolean(env("AWS_SECRET_ACCESS_KEY")),
    bedrockSessionTokenPresent: Boolean(env("AWS_SESSION_TOKEN")),
    bedrockModelId: env("BEDROCK_MODEL_ID"),
  },
  checks: nonEmptyObject({
    failures: env("FAILURES"),
    warnings: env("WARNINGS"),
    blockedGroups: env("BLOCKED_GROUPS"),
    externalBlockedGroups: env("EXTERNAL_BLOCKED_GROUPS"),
    shareNoAuthStatus: env("SHARE_NOAUTH_STATUS"),
    sharePublicStatus: env("SHARE_PUBLIC_STATUS"),
    sharePublicStreamStatus: env("SHARE_PUBLIC_STREAM_STATUS"),
    shareClaudeStatus: env("SHARE_CLAUDE_STATUS"),
    shareCodexStatus: env("SHARE_CODEX_STATUS"),
    shareGeminiStatus: env("SHARE_GEMINI_STATUS"),
    shareClaudeStreamStatus: env("SHARE_CLAUDE_STREAM_STATUS"),
    shareCodexStreamStatus: env("SHARE_CODEX_STREAM_STATUS"),
    shareGeminiStreamStatus: env("SHARE_GEMINI_STREAM_STATUS"),
    serverHealthStatus: env("SERVER_HEALTH_STATUS"),
    routerStatusStatus: env("ROUTER_STATUS_STATUS"),
    routerDiagnosticsStatus: env("ROUTER_DIAGNOSTICS_STATUS"),
    routerTunnelsStatus: env("ROUTER_TUNNELS_STATUS"),
    sharesStatus: env("SHARES_STATUS"),
    usageRequestsStatus: env("USAGE_REQUESTS_STATUS"),
    providerHealthStatus: env("PROVIDER_HEALTH_STATUS"),
    diagnosticsClassification: env("DIAGNOSTICS_CLASSIFICATION"),
    matrixTotal: env("MATRIX_TOTAL"),
    matrixRunnable: env("MATRIX_RUNNABLE"),
    matrixSkipped: env("MATRIX_SKIPPED"),
    matrixSkeleton: env("MATRIX_SKELETON"),
    matrixFixtureEvidenceComplete: env("MATRIX_FIXTURE_EVIDENCE_COMPLETE"),
    matrixFixtureEvidenceMissing: env("MATRIX_FIXTURE_EVIDENCE_MISSING"),
    runContractTests: env("RUN_CONTRACT_TESTS"),
    contractTestsPassed: env("CONTRACT_TESTS_PASSED"),
    contractFailures: env("CONTRACT_FAILURES"),
    requireStreamUsage: env("REQUIRE_STREAM_USAGE"),
    skipped: env("SKIPPED"),
    liveVerificationComplete: env("LIVE_VERIFICATION_COMPLETE"),
    oauthNativeReady: env("OAUTH_NATIVE_READY"),
    oauthGateStatus: env("OAUTH_GATE_STATUS"),
    claudeMax5xGateStatus: env("CLAUDE_MAX_5X_GATE_STATUS"),
    claudeMax20xGateStatus: env("CLAUDE_MAX_20X_GATE_STATUS"),
    codexImagesGateStatus: env("CODEX_IMAGES_GATE_STATUS"),
    grokGateStatus: env("GROK_GATE_STATUS"),
    grokReadyStatus: env("GROK_READY_STATUS"),
    grokModelsStatus: env("GROK_MODELS_STATUS"),
    grokJsonStatus: env("GROK_JSON_STATUS"),
    grokStreamStatus: env("GROK_STREAM_STATUS"),
    grokMediaStatus: env("GROK_MEDIA_STATUS"),
    cursorGateStatus: env("CURSOR_GATE_STATUS"),
    copilotGateStatus: env("COPILOT_GATE_STATUS"),
    copilotBindingsStatus: env("COPILOT_BINDINGS_STATUS"),
    copilotModelsStatus: env("COPILOT_MODELS_STATUS"),
    copilotQuotaStatus: env("COPILOT_QUOTA_STATUS"),
    copilotClaudeStatus: env("COPILOT_CLAUDE_STATUS"),
    copilotCodexStatus: env("COPILOT_CODEX_STATUS"),
    copilotGeminiStatus: env("COPILOT_GEMINI_STATUS"),
    amazonQGateStatus: env("AMAZON_Q_GATE_STATUS"),
    kiroGateStatus: env("KIRO_GATE_STATUS"),
    bedrockGateStatus: env("BEDROCK_GATE_STATUS"),
    skeletonTotal: env("SKELETON_TOTAL"),
    skeletonBatch: env("SKELETON_BATCH"),
    releaseDecision: env("RELEASE_DECISION"),
    deploymentNotTested: env("DEPLOYMENT_NOT_TESTED"),
    requestLogStatus: env("REQUEST_LOG_STATUS"),
    directLogDuplicateStatus: env("DIRECT_LOG_DUPLICATE_STATUS"),
  }),
  notes: env("EVIDENCE_NOTES"),
};

const clean = nonEmptyObject(evidence);
const serialized = `${JSON.stringify(clean, null, 2)}\n`;
assertNoSecrets(serialized);

fs.mkdirSync(path.dirname(output), { recursive: true });
fs.writeFileSync(output, serialized, { mode: 0o600 });
console.log(`wrote redacted evidence: ${output}`);
