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
    Object.entries(object).filter(([, value]) => value !== undefined && value !== "")
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
    console.error(`refusing to write evidence; secret-like pattern matched: ${match}`);
    process.exit(3);
  }
}

const output = argValue("--out", env("EVIDENCE_FILE"));
if (!output) {
  console.error("--out or EVIDENCE_FILE is required");
  process.exit(2);
}

const evidence = {
  date: new Date().toISOString(),
  stage: env("EVIDENCE_STAGE", env("STAGE", "unknown")),
  status: env("EVIDENCE_STATUS", "unknown"),
  serverCommit: env("SERVER_COMMIT", gitCommit()),
  target: env("EVIDENCE_TARGET"),
  source: env("EVIDENCE_SOURCE"),
  app: env("EVIDENCE_APP"),
  provider: env("EVIDENCE_PROVIDER"),
  providerType: env("EVIDENCE_PROVIDER_TYPE"),
  blockerGroup: env("BLOCKER_GROUP"),
  failureClass: env("FAILURE_CLASS"),
  deploymentNotTested: env("DEPLOYMENT_NOT_TESTED"),
  serverUrl: env("SERVER_URL"),
  routerBaseUrl: env("ROUTER_BASE_URL"),
  marketUrl: env("MARKET_URL"),
  shareMarketUrl: env("SHARE_MARKET_URL"),
  directShareUrl: env("DIRECT_SHARE_URL"),
  marketApiUrl: env("MARKET_API_URL"),
  shareId: env("SHARE_ID"),
  requestId: env("REQUEST_ID"),
  buyerEmail: redactEmail(env("SHARE_MARKET_BUYER_EMAIL")),
  oauthAccounts: nonEmptyObject({
    codex: redactEmail(env("CODEX_OAUTH_TEST_ACCOUNT")),
    claude: redactEmail(env("CLAUDE_OAUTH_TEST_ACCOUNT")),
    gemini: redactEmail(env("GEMINI_OAUTH_TEST_ACCOUNT")),
    cursor: redactEmail(env("CURSOR_OAUTH_TEST_ACCOUNT")),
    antigravity: redactEmail(env("ANTIGRAVITY_OAUTH_TEST_ACCOUNT")),
    githubCopilot: redactEmail(env("GITHUB_COPILOT_TEST_ACCOUNT")),
    kiro: redactEmail(env("KIRO_TEST_ACCOUNT")),
  }),
  listingId: env("SHARE_MARKET_LISTING_ID"),
  orderId: env("SHARE_MARKET_ORDER_ID"),
  streamProbe: env("STREAM_PROBE"),
  probeModel: env("PROBE_MODEL"),
  routerTokenPresent: Boolean(env("ROUTER_API_TOKEN")),
  marketTokenPresent: Boolean(env("MARKET_API_TOKEN")),
  providerTokensPresent: {
    claude: Boolean(env("CLAUDE_PROVIDER_TOKEN")),
    codex: Boolean(env("CODEX_PROVIDER_TOKEN")),
    gemini: Boolean(env("GEMINI_PROVIDER_TOKEN")),
  },
  oauthFixturesPresent: {
    codex: Boolean(env("CODEX_OAUTH_REFRESH_TOKEN_FIXTURE") || env("CODEX_OAUTH_REFRESH_TOKEN")),
    claude: Boolean(env("CLAUDE_OAUTH_REFRESH_TOKEN_FIXTURE") || env("CLAUDE_OAUTH_REFRESH_TOKEN")),
    gemini: Boolean(
      env("GEMINI_OAUTH_REFRESH_TOKEN_FIXTURE") ||
        env("GEMINI_OAUTH_REFRESH_TOKEN") ||
        env("GEMINI_CLI_CREDENTIALS_FIXTURE")
    ),
    cursor: Boolean(env("CURSOR_OAUTH_REFRESH_TOKEN_FIXTURE") || env("CURSOR_API_KEY_FIXTURE")),
    antigravity: Boolean(env("ANTIGRAVITY_OAUTH_REFRESH_TOKEN_FIXTURE")),
    githubCopilot: Boolean(env("GITHUB_COPILOT_TOKEN_FIXTURE")),
    kiro: Boolean(env("KIRO_REFRESH_TOKEN_FIXTURE")),
  },
  longTailInputsPresent: {
    cursorOAuthAccount: Boolean(env("CURSOR_OAUTH_TEST_ACCOUNT")),
    cursorCallbackUrl: Boolean(env("CURSOR_OAUTH_CALLBACK_URL")),
    githubCopilotAccount: Boolean(env("GITHUB_COPILOT_TEST_ACCOUNT")),
    githubCopilotDomain: env("GITHUB_COPILOT_GITHUB_DOMAIN"),
    kiroAccount: Boolean(env("KIRO_TEST_ACCOUNT")),
    kiroRegion: env("KIRO_REGION"),
    kiroStartUrl: env("KIRO_START_URL"),
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
    directNoAuthStatus: env("DIRECT_NOAUTH_STATUS"),
    directPublicStatus: env("DIRECT_PUBLIC_STATUS"),
    directPublicStreamStatus: env("DIRECT_PUBLIC_STREAM_STATUS"),
    directClaudeStatus: env("DIRECT_CLAUDE_STATUS"),
    directCodexStatus: env("DIRECT_CODEX_STATUS"),
    directGeminiStatus: env("DIRECT_GEMINI_STATUS"),
    directClaudeStreamStatus: env("DIRECT_CLAUDE_STREAM_STATUS"),
    directCodexStreamStatus: env("DIRECT_CODEX_STREAM_STATUS"),
    directGeminiStreamStatus: env("DIRECT_GEMINI_STREAM_STATUS"),
    localShareStatus: env("LOCAL_SHARE_STATUS"),
    marketApiStatus: env("MARKET_API_STATUS"),
    marketApiStreamStatus: env("MARKET_API_STREAM_STATUS"),
    marketClaudeStatus: env("MARKET_CLAUDE_STATUS"),
    marketCodexStatus: env("MARKET_CODEX_STATUS"),
    marketGeminiStatus: env("MARKET_GEMINI_STATUS"),
    marketClaudeStreamStatus: env("MARKET_CLAUDE_STREAM_STATUS"),
    marketCodexStreamStatus: env("MARKET_CODEX_STREAM_STATUS"),
    marketGeminiStreamStatus: env("MARKET_GEMINI_STREAM_STATUS"),
    marketHealthStatus: env("MARKET_HEALTH_STATUS"),
    shareMarketAddStatus: env("SHARE_MARKET_ADD_STATUS"),
    shareMarketRevokeStatus: env("SHARE_MARKET_REVOKE_STATUS"),
    shareMarketAddEditId: env("SHARE_MARKET_ADD_EDIT_ID"),
    shareMarketRevokeEditId: env("SHARE_MARKET_REVOKE_EDIT_ID"),
    serverHealthStatus: env("SERVER_HEALTH_STATUS"),
    routerStatusStatus: env("ROUTER_STATUS_STATUS"),
    routerDiagnosticsStatus: env("ROUTER_DIAGNOSTICS_STATUS"),
    routerTunnelsStatus: env("ROUTER_TUNNELS_STATUS"),
    sharesStatus: env("SHARES_STATUS"),
    usageLogsStatus: env("USAGE_LOGS_STATUS"),
    providerHealthStatus: env("PROVIDER_HEALTH_STATUS"),
    diagnosticsClassification: env("DIAGNOSTICS_CLASSIFICATION"),
    matrixTotal: env("MATRIX_TOTAL"),
    matrixRunnable: env("MATRIX_RUNNABLE"),
    matrixSkipped: env("MATRIX_SKIPPED"),
    matrixSkeleton: env("MATRIX_SKELETON"),
    oauthNativeReady: env("OAUTH_NATIVE_READY"),
    oauthGateStatus: env("OAUTH_GATE_STATUS"),
    cursorGateStatus: env("CURSOR_GATE_STATUS"),
    copilotGateStatus: env("COPILOT_GATE_STATUS"),
    kiroGateStatus: env("KIRO_GATE_STATUS"),
    bedrockGateStatus: env("BEDROCK_GATE_STATUS"),
    skeletonTotal: env("SKELETON_TOTAL"),
    skeletonBatch: env("SKELETON_BATCH"),
    releaseDecision: env("RELEASE_DECISION"),
    deploymentNotTested: env("DEPLOYMENT_NOT_TESTED"),
    requestLogStatus: env("REQUEST_LOG_STATUS"),
    directLogDuplicateStatus: env("DIRECT_LOG_DUPLICATE_STATUS"),
    marketLogDuplicateStatus: env("MARKET_LOG_DUPLICATE_STATUS"),
    marketPermissionsStatus: env("MARKET_PERMISSIONS_STATUS"),
  }),
  notes: env("EVIDENCE_NOTES"),
};

const clean = nonEmptyObject(evidence);
const serialized = `${JSON.stringify(clean, null, 2)}\n`;
assertNoSecrets(serialized);

fs.mkdirSync(path.dirname(output), { recursive: true });
fs.writeFileSync(output, serialized, { mode: 0o600 });
console.log(`wrote redacted evidence: ${output}`);
