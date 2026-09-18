#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

// This validator deliberately keeps both Google OAuth rails independent. A
// successful fixture run proves only the harness contract; only a private,
// current-commit receipt produced with RUN_REAL=1 can prove one live rail.

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const contract = JSON.parse(
  fs.readFileSync(
    path.join(repoRoot, "assets/contract/antigravity-reference-delta.json"),
    "utf8",
  ),
);

const HARNESS_REVISION = 1;
const railSpecs = Object.freeze({
  antigravity_oauth: Object.freeze({
    providerType: "antigravity_oauth",
    driverId: "special.antigravity",
    accountEnv: "ANTIGRAVITY_OAUTH_TEST_ACCOUNT",
    shareEnv: "CC_SWITCH_ANTIGRAVITY_OAUTH_SHARE_ID",
    receiptEnv: "ANTIGRAVITY_OAUTH_REAL_RECEIPT_FILE",
    providerEnvs: Object.freeze({
      claude: "CC_SWITCH_ANTIGRAVITY_OAUTH_CLAUDE_PROVIDER_ID",
      gemini: "CC_SWITCH_ANTIGRAVITY_OAUTH_GEMINI_PROVIDER_ID",
    }),
    modelEnvs: Object.freeze({
      claude: "CC_SWITCH_ANTIGRAVITY_OAUTH_CLAUDE_MODEL",
      gemini: "CC_SWITCH_ANTIGRAVITY_OAUTH_GEMINI_MODEL",
    }),
  }),
  agy_oauth: Object.freeze({
    providerType: "agy_oauth",
    driverId: "special.agy",
    accountEnv: "AGY_OAUTH_TEST_ACCOUNT",
    shareEnv: "CC_SWITCH_AGY_OAUTH_SHARE_ID",
    receiptEnv: "AGY_OAUTH_REAL_RECEIPT_FILE",
    providerEnvs: Object.freeze({
      claude: "CC_SWITCH_AGY_OAUTH_CLAUDE_PROVIDER_ID",
      gemini: "CC_SWITCH_AGY_OAUTH_GEMINI_PROVIDER_ID",
    }),
    modelEnvs: Object.freeze({
      claude: "CC_SWITCH_AGY_OAUTH_CLAUDE_MODEL",
      gemini: "CC_SWITCH_AGY_OAUTH_GEMINI_MODEL",
    }),
  }),
});

function env(name, fallback = "") {
  return String(process.env[name] || fallback).trim();
}

function argValue(name, fallback = "") {
  const index = process.argv.indexOf(name);
  return index >= 0 && index + 1 < process.argv.length
    ? process.argv[index + 1]
    : fallback;
}

function usable(value) {
  const text = String(value || "").trim();
  return Boolean(text) && !text.includes("<") && !text.includes(">");
}

function fail(message) {
  throw new Error(message);
}

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, nested]) => [key, canonical(nested)]),
    );
  }
  return value;
}

function digest(domain, value) {
  return crypto
    .createHash("sha256")
    .update(`${domain}\0`)
    .update(JSON.stringify(canonical(value)))
    .digest("hex");
}

function gitCommit() {
  try {
    return execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: repoRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    fail("current target commit is unavailable");
  }
}

const rail = argValue("--rail", env("CC_SWITCH_ANTIGRAVITY_REAL_RAIL"));
const spec = railSpecs[rail];
if (!spec) {
  console.log(
    JSON.stringify({
      verificationState: "blocked_inputs",
      liveState: "live_pending",
      missingInputs: ["--rail antigravity_oauth|agy_oauth"],
    }),
  );
  process.exit(0);
}

const fixtureMode = env("CC_SWITCH_ANTIGRAVITY_HARNESS_MODE") === "fixture";
const serverUrl = env("SERVER_URL").replace(/\/+$/, "");
const shareUrl = env("CC_SWITCH_SHARE_URL").replace(/\/+$/, "");
const serverToken = env("CC_SWITCH_SERVER_TOKEN");
const routerToken = env("ROUTER_API_TOKEN");
const routerTokenHeader = env("ROUTER_API_TOKEN_HEADER", "Authorization");
const accountSelector = env(spec.accountEnv);
const shareId = env(spec.shareEnv);
const providerIds = Object.fromEntries(
  Object.entries(spec.providerEnvs).map(([app, name]) => [app, env(name)]),
);
const models = Object.fromEntries(
  Object.entries(spec.modelEnvs).map(([app, name]) => [app, env(name)]),
);
const receiptFile = env(spec.receiptEnv, env("ANTIGRAVITY_REAL_RECEIPT_FILE"));
const configuredTimeoutMs = Number(env("CC_SWITCH_REAL_TIMEOUT_MS", "30000"));
const timeoutMs = Number.isFinite(configuredTimeoutMs)
  ? Math.max(1_000, Math.min(120_000, Math.trunc(configuredTimeoutMs)))
  : 30_000;

const requiredInputs = [
  ["RUN_REAL=1", process.env.RUN_REAL === "1" ? "1" : ""],
  ["SERVER_URL", serverUrl],
  ["CC_SWITCH_SERVER_TOKEN", serverToken],
  ["CC_SWITCH_SHARE_URL", shareUrl],
  ["ROUTER_API_TOKEN", routerToken],
  [spec.accountEnv, accountSelector],
  [spec.shareEnv, shareId],
  ...Object.entries(spec.providerEnvs).map(([app, name]) => [name, providerIds[app]]),
  ...Object.entries(spec.modelEnvs).map(([app, name]) => [name, models[app]]),
  [spec.receiptEnv, receiptFile],
];
const missingInputs = requiredInputs
  .filter(([, value]) => !usable(value))
  .map(([name]) => name);
if (missingInputs.length > 0) {
  console.log(
    JSON.stringify({
      rail,
      verificationState: "blocked_inputs",
      liveState: "live_pending",
      missingInputs,
    }),
  );
  process.exit(0);
}

function safeOrigin(value, label, { share = false } = {}) {
  let parsed;
  try {
    parsed = new URL(value);
  } catch {
    fail(`${label} is not a valid URL`);
  }
  if (parsed.username || parsed.password || parsed.search || parsed.hash) {
    fail(`${label} must be a credential-free origin`);
  }
  const loopback = ["127.0.0.1", "::1", "localhost"].includes(parsed.hostname);
  if (parsed.protocol !== "https:" && !(loopback && (!share || fixtureMode))) {
    fail(`${label} must use HTTPS${share ? " outside fixture mode" : " or loopback HTTP"}`);
  }
  return parsed.origin;
}

safeOrigin(serverUrl, "SERVER_URL");
safeOrigin(shareUrl, "CC_SWITCH_SHARE_URL", { share: true });
if (!/^(authorization|x-api-key|x-goog-api-key)$/i.test(routerTokenHeader)) {
  fail("ROUTER_API_TOKEN_HEADER is unsupported");
}
if (!path.isAbsolute(receiptFile)) {
  fail("ANTIGRAVITY_REAL_RECEIPT_FILE must be an absolute path");
}
const receiptPath = path.resolve(receiptFile);
const receiptRelative = path.relative(repoRoot, receiptPath);
if (!receiptRelative.startsWith("..") && !path.isAbsolute(receiptRelative)) {
  fail("ANTIGRAVITY_REAL_RECEIPT_FILE must stay outside the repository");
}
if (!fs.existsSync(receiptPath) || !fs.statSync(receiptPath).isFile()) {
  fail("ANTIGRAVITY_REAL_RECEIPT_FILE is unavailable");
}
if (!fixtureMode && (fs.statSync(receiptPath).mode & 0o077) !== 0) {
  fail("ANTIGRAVITY_REAL_RECEIPT_FILE must not be group/world accessible");
}

const secrets = [serverToken, routerToken].filter(usable);
function containsSecret(value) {
  const text = String(value || "");
  return (
    secrets.some((secret) => text.includes(secret)) ||
    /Bearer\s+[A-Za-z0-9._~+/-]{10,}/i.test(text) ||
    /\bya29\.[A-Za-z0-9._-]+\b/.test(text) ||
    /\beyJ[A-Za-z0-9_-]{12,}\.[A-Za-z0-9_-]{12,}\.[A-Za-z0-9_-]{8,}\b/.test(text) ||
    /"(?:access|refresh|id)_token"\s*:\s*"[^"<][^"]+"/i.test(text)
  );
}

function adminHeaders() {
  return { accept: "application/json", authorization: `Bearer ${serverToken}` };
}

function shareHeaders() {
  const headers = { accept: "application/json" };
  headers[routerTokenHeader] = /^authorization$/i.test(routerTokenHeader)
    ? `Bearer ${routerToken}`
    : routerToken;
  return headers;
}

async function requestJson(base, requestPath, headers, label) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(`${base}${requestPath}`, {
      method: "GET",
      headers,
      signal: controller.signal,
    });
    const bytes = new Uint8Array(await response.arrayBuffer());
    if (bytes.byteLength > 4 * 1024 * 1024) {
      fail(`${label} exceeded the response-size bound`);
    }
    const text = new TextDecoder().decode(bytes);
    if (containsSecret(text)) fail(`${label} contained secret-like material`);
    if (!response.ok) fail(`${label} returned HTTP ${response.status}`);
    try {
      return JSON.parse(text);
    } catch {
      fail(`${label} returned invalid JSON`);
    }
  } catch (error) {
    if (error instanceof Error && error.message.startsWith(label)) throw error;
    fail(`${label} request failed`);
  } finally {
    clearTimeout(timer);
  }
}

function exactOne(values, label) {
  if (values.length !== 1) fail(`${label} is not unique`);
  return values[0];
}

async function validateAccount() {
  const response = await requestJson(
    serverUrl,
    "/api/accounts",
    adminHeaders(),
    "Antigravity Account list",
  );
  if (response?.ok !== true || !Array.isArray(response.accounts)) {
    fail("Antigravity Account list violated the control-plane contract");
  }
  const selector = accountSelector.toLowerCase();
  const account = exactOne(
    response.accounts.filter(
      (candidate) =>
        candidate?.providerType === spec.providerType &&
        (candidate.id === accountSelector ||
          candidate.email?.trim().toLowerCase() === selector),
    ),
    "Antigravity Account selector",
  );
  if (
    !Number.isSafeInteger(account.authIdentityGeneration) ||
    account.authIdentityGeneration < 1 ||
    !Number.isSafeInteger(account.tokenRefreshGeneration) ||
    account.tokenRefreshGeneration < 0 ||
    account.hasAccessToken !== true ||
    account.hasApiKey === true ||
    account.needsRelogin === true
  ) {
    fail("Antigravity Account is not a live OAuth identity generation");
  }
  return account;
}

async function validateBindings(account) {
  const providerList = await requestJson(
    serverUrl,
    "/api/providers",
    adminHeaders(),
    "Antigravity Provider list",
  );
  if (providerList?.ok !== true || !Array.isArray(providerList.providers)) {
    fail("Antigravity Provider list violated the control-plane contract");
  }
  const bindings = [];
  for (const [app, providerId] of Object.entries(providerIds)) {
    const view = exactOne(
      providerList.providers.filter(
        (candidate) => candidate?.app === app && candidate?.provider?.id === providerId,
      ),
      `${app} Antigravity Provider binding`,
    );
    const authRef = view.runtime?.authRef;
    if (
      view.providerType !== spec.providerType ||
      view.providerTypeId !== spec.providerType ||
      view.runtime?.driverId !== spec.driverId ||
      view.runtime?.configurationState !== "ready" ||
      authRef?.kind !== "managed_account" ||
      authRef.expectedProviderType !== spec.providerType ||
      authRef.accountId !== account.id ||
      authRef.authIdentityGeneration !== account.authIdentityGeneration
    ) {
      fail(`${app} Antigravity Provider is not fixed to the selected rail generation`);
    }
    bindings.push({
      app,
      providerId,
      providerRevision: view.providerRevision ?? view.provider?.revision ?? 0,
      runtimeFingerprint: String(view.runtime.runtimeFingerprint || ""),
      authIdentityGeneration: authRef.authIdentityGeneration,
    });
  }

  const shareList = await requestJson(
    serverUrl,
    "/api/shares",
    adminHeaders(),
    "Antigravity Share list",
  );
  if (shareList?.ok !== true || !Array.isArray(shareList.shares)) {
    fail("Antigravity Share list violated the control-plane contract");
  }
  const share = exactOne(
    shareList.shares.filter((candidate) => candidate?.id === shareId),
    "Antigravity Share",
  );
  const shareBindings = [
    { app: share.app, providerId: share.providerId, providerType: share.providerType },
    ...(Array.isArray(share.bindings) ? share.bindings : []),
  ];
  for (const [app, providerId] of Object.entries(providerIds)) {
    if (
      !shareBindings.some(
        (binding) =>
          binding?.app === app &&
          binding?.providerId === providerId &&
          binding?.providerType === spec.providerType,
      )
    ) {
      fail(`Antigravity Share is not fixed to the ${app} Provider rail`);
    }
  }
  return { bindings, shareRevision: share.configRevision ?? 0 };
}

async function validateCatalogs() {
  const snapshots = [];
  for (const [app, providerId] of Object.entries(providerIds)) {
    const query = new URLSearchParams({ app, providerId });
    const catalog = await requestJson(
      shareUrl,
      `/v1/models?${query}`,
      shareHeaders(),
      `${app} Antigravity model catalog`,
    );
    if (
      !Array.isArray(catalog?.data) ||
      catalog.source !== "authenticated_fetch_available_models" ||
      catalog.stale !== false ||
      !Number.isSafeInteger(catalog.fetchedAtMs) ||
      catalog.fetchedAtMs <= 0
    ) {
      fail(`${app} Antigravity model catalog is not fresh bound-account evidence`);
    }
    const matches = catalog.data.filter((entry) => entry?.id === models[app]);
    if (matches.length !== 1) {
      fail(`${app} Antigravity model catalog does not contain the exact selected model`);
    }
    snapshots.push({ app, model: models[app], source: catalog.source, stale: catalog.stale });
  }
  return snapshots;
}

function validateReceipt(receipt, expectedScopeDigest, targetCommit, account, bindingState) {
  const allowed = new Set([
    "schemaVersion",
    "providerFamily",
    "rail",
    "verificationState",
    "liveState",
    "targetCommit",
    "harnessRevision",
    "recordedAt",
    "scopeDigest",
    "models",
    "requestTypeDecision",
    "checks",
    "bodyHashes",
    "decisions",
    "generations",
    "surfaceMatrix",
    "decoyRequests",
    "sensitiveScan",
  ]);
  if (!receipt || typeof receipt !== "object" || Array.isArray(receipt)) {
    fail("Antigravity receipt must be an object");
  }
  for (const key of Object.keys(receipt)) {
    if (!allowed.has(key)) fail(`Antigravity receipt contains forbidden field ${key}`);
  }
  if (containsSecret(JSON.stringify(receipt))) {
    fail("Antigravity receipt contained secret-like material");
  }
  if (
    receipt.schemaVersion !== contract.realAcceptance.receiptSchemaVersion ||
    receipt.providerFamily !== "antigravity" ||
    receipt.rail !== rail ||
    receipt.targetCommit !== targetCommit ||
    receipt.harnessRevision !== HARNESS_REVISION ||
    receipt.scopeDigest !== expectedScopeDigest ||
    !/^[a-f0-9]{64}$/.test(receipt.scopeDigest)
  ) {
    fail("Antigravity receipt identity does not match the selected target scope");
  }
  if (!Number.isFinite(Date.parse(receipt.recordedAt))) {
    fail("Antigravity receipt recordedAt is invalid");
  }
  const expectedVerification = fixtureMode ? "contract_verified" : "live_verified";
  const expectedLive = fixtureMode ? "live_pending" : "live_verified";
  if (
    receipt.verificationState !== expectedVerification ||
    receipt.liveState !== expectedLive
  ) {
    fail("Antigravity receipt evidence state is invalid for this harness mode");
  }
  if (JSON.stringify(receipt.models) !== JSON.stringify(models)) {
    fail("Antigravity receipt model scope changed");
  }
  if (
    receipt.requestTypeDecision?.ordinary !== "agent" ||
    receipt.requestTypeDecision?.webSearch !== "web_search" ||
    receipt.requestTypeDecision?.mixedToolsPreserved !== true
  ) {
    fail("Antigravity requestType receipt does not prove the current wire decision");
  }
  const requiredChecks = [...contract.realAcceptance.requiredChecks].sort();
  if (
    !receipt.checks ||
    typeof receipt.checks !== "object" ||
    Array.isArray(receipt.checks) ||
    JSON.stringify(Object.keys(receipt.checks).sort()) !== JSON.stringify(requiredChecks) ||
    requiredChecks.some((check) => receipt.checks[check] !== "pass")
  ) {
    fail("Antigravity receipt check set is incomplete");
  }
  const hashKeys = [...contract.realAcceptance.requiredBodyHashes].sort();
  if (
    !receipt.bodyHashes ||
    JSON.stringify(Object.keys(receipt.bodyHashes).sort()) !== JSON.stringify(hashKeys) ||
    hashKeys.some((key) => !/^[a-f0-9]{64}$/.test(receipt.bodyHashes[key]))
  ) {
    fail("Antigravity receipt body hashes are incomplete");
  }
  if (
    receipt.decisions?.sameAccount401 !== "replayed_once" ||
    receipt.decisions?.second401 !== "terminal" ||
    receipt.decisions?.structured429 !== "bounded_exact_scope" ||
    receipt.decisions?.compaction !== "runtime_disabled"
  ) {
    fail("Antigravity receipt recovery decisions are incomplete");
  }
  if (
    receipt.generations?.authIdentityGeneration !== account.authIdentityGeneration ||
    receipt.generations?.tokenRefreshGeneration !== account.tokenRefreshGeneration ||
    receipt.generations?.providerBindingDigest !==
      digest("cc-switch-server:antigravity-provider-bindings:v1", bindingState.bindings) ||
    receipt.generations?.shareRevision !== bindingState.shareRevision
  ) {
    fail("Antigravity receipt generations do not match the selected binding");
  }
  for (const app of ["claude", "gemini"]) {
    const surface = receipt.surfaceMatrix?.[app];
    if (
      surface?.nonstream !== "pass" ||
      surface?.stream !== "pass" ||
      surface?.tool !== "pass" ||
      surface?.usage !== "pass" ||
      surface?.terminal !== "pass"
    ) {
      fail(`Antigravity ${app} surface receipt is incomplete`);
    }
  }
  if (
    receipt.decoyRequests?.otherAccount !== 0 ||
    receipt.decoyRequests?.otherProvider !== 0 ||
    receipt.decoyRequests?.otherRail !== 0 ||
    receipt.sensitiveScan?.status !== "pass" ||
    receipt.sensitiveScan?.matches !== 0
  ) {
    fail("Antigravity receipt did not prove isolation and secret safety");
  }
}

async function main() {
  const targetCommit = gitCommit();
  const account = await validateAccount();
  const bindingState = await validateBindings(account);
  const catalogs = await validateCatalogs();
  const scopeDigest = digest("cc-switch-server:antigravity-real-scope:v1", {
    rail,
    accountId: account.id,
    authIdentityGeneration: account.authIdentityGeneration,
    tokenRefreshGeneration: account.tokenRefreshGeneration,
    providerIds,
    shareId,
    models,
    bindings: bindingState.bindings,
    shareRevision: bindingState.shareRevision,
    catalogs,
    targetCommit,
    harnessRevision: HARNESS_REVISION,
  });
  let receipt;
  try {
    receipt = JSON.parse(fs.readFileSync(receiptPath, "utf8"));
  } catch {
    fail("ANTIGRAVITY_REAL_RECEIPT_FILE is not valid JSON");
  }
  validateReceipt(receipt, scopeDigest, targetCommit, account, bindingState);
  const verificationState = fixtureMode ? "contract_verified" : "live_verified";
  const liveState = fixtureMode ? "live_pending" : "live_verified";
  console.log(
    `[PASS] Antigravity ${rail} private receipt accepted (verificationState=${verificationState}, liveState=${liveState})`,
  );
}

main().catch(() => {
  console.error("[FAIL] Antigravity acceptance failed (details redacted)");
  process.exitCode = 1;
});
