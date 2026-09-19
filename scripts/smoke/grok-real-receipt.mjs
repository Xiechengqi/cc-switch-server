#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

// Grok normal-path probing and acceptance are intentionally separate. This
// validator consumes one operation-scoped private receipt; fixture mode proves
// only that the validator fails closed and can never promote live state.

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const contract = JSON.parse(
  fs.readFileSync(
    path.join(repoRoot, "assets/contract/grok-reference-delta.json"),
    "utf8",
  ),
);
const HARNESS_REVISION = 1;
const RECEIPT_MAX_AGE_MS = 24 * 60 * 60 * 1_000;

const operationSpecs = Object.freeze({
  inference: Object.freeze({
    providerEnv: "CC_SWITCH_GROK_INFERENCE_PROVIDER_ID",
    shareEnv: "CC_SWITCH_GROK_INFERENCE_SHARE_ID",
    modelEnv: "CC_SWITCH_GROK_INFERENCE_MODEL",
    receiptEnv: "GROK_INFERENCE_REAL_RECEIPT_FILE",
  }),
  media: Object.freeze({
    providerEnv: "CC_SWITCH_GROK_MEDIA_PROVIDER_ID",
    shareEnv: "CC_SWITCH_GROK_MEDIA_SHARE_ID",
    modelEnv: "CC_SWITCH_GROK_MEDIA_MODEL",
    receiptEnv: "GROK_MEDIA_REAL_RECEIPT_FILE",
  }),
  remote_compaction: Object.freeze({
    providerEnv: "CC_SWITCH_GROK_COMPACTION_PROVIDER_ID",
    shareEnv: "CC_SWITCH_GROK_COMPACTION_SHARE_ID",
    modelEnv: "CC_SWITCH_GROK_COMPACTION_MODEL",
    receiptEnv: "GROK_REMOTE_COMPACTION_REAL_RECEIPT_FILE",
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

function sameCanonical(left, right) {
  return JSON.stringify(canonical(left)) === JSON.stringify(canonical(right));
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

const operation = argValue("--operation", env("CC_SWITCH_GROK_REAL_OPERATION"));
const spec = operationSpecs[operation];
if (!spec) {
  console.log(
    JSON.stringify({
      verificationState: "blocked_inputs",
      liveState: "live_pending",
      missingInputs: ["--operation inference|media|remote_compaction"],
    }),
  );
  process.exit(0);
}

const fixtureMode = env("CC_SWITCH_GROK_HARNESS_MODE") === "fixture";
const serverUrl = env("SERVER_URL").replace(/\/+$/, "");
const shareUrl = env("CC_SWITCH_SHARE_URL").replace(/\/+$/, "");
const serverToken = env("CC_SWITCH_SERVER_TOKEN");
const routerToken = env("ROUTER_API_TOKEN");
const routerTokenHeader = env("ROUTER_API_TOKEN_HEADER", "Authorization");
const accountSelector = env("GROK_OAUTH_TEST_ACCOUNT");
const providerId = env(spec.providerEnv);
const shareId = env(spec.shareEnv);
const model = env(spec.modelEnv);
const signedUser = env("CC_SWITCH_GROK_SIGNED_USER");
const sessionId = env("CC_SWITCH_GROK_SESSION_ID");
const turnIndex = env("CC_SWITCH_GROK_TURN_INDEX");
const receiptFile = env(spec.receiptEnv, env("GROK_REAL_RECEIPT_FILE"));
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
  ["GROK_OAUTH_TEST_ACCOUNT", accountSelector],
  [spec.providerEnv, providerId],
  [spec.shareEnv, shareId],
  [spec.modelEnv, model],
  ["CC_SWITCH_GROK_SIGNED_USER", signedUser],
  ["CC_SWITCH_GROK_SESSION_ID", sessionId],
  ["CC_SWITCH_GROK_TURN_INDEX", turnIndex],
  [spec.receiptEnv, receiptFile],
];
const missingInputs = requiredInputs
  .filter(([, value]) => !usable(value))
  .map(([name]) => name);
if (missingInputs.length > 0) {
  console.log(
    JSON.stringify({
      operation,
      verificationState: "blocked_inputs",
      liveState: "live_pending",
      missingInputs,
    }),
  );
  process.exit(0);
}
if (!/^[A-Za-z0-9][A-Za-z0-9._:/-]{0,255}$/.test(model)) {
  fail(`${spec.modelEnv} must be one exact bounded model id`);
}
if (!/^(0|[1-9][0-9]{0,19})$/.test(turnIndex)) {
  fail("CC_SWITCH_GROK_TURN_INDEX must be one canonical unsigned decimal");
}
if (BigInt(turnIndex) > 18_446_744_073_709_551_615n) {
  fail("CC_SWITCH_GROK_TURN_INDEX exceeds u64");
}

function safeOrigin(value, label, { share = false } = {}) {
  let parsed;
  try {
    parsed = new URL(value);
  } catch {
    fail(`${label} is not a valid URL`);
  }
  if (
    parsed.username ||
    parsed.password ||
    parsed.search ||
    parsed.hash ||
    parsed.pathname !== "/"
  ) {
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
  fail("Grok receipt file must be an absolute path");
}
const requestedReceiptPath = path.resolve(receiptFile);
const requestedRelative = path.relative(repoRoot, requestedReceiptPath);
if (!requestedRelative.startsWith("..") && !path.isAbsolute(requestedRelative)) {
  fail("Grok receipt file must stay outside the repository");
}
if (
  !fs.existsSync(requestedReceiptPath) ||
  !fs.statSync(requestedReceiptPath).isFile()
) {
  fail("Grok receipt file is unavailable");
}
const receiptPath = fs.realpathSync(requestedReceiptPath);
const receiptRelative = path.relative(repoRoot, receiptPath);
if (!receiptRelative.startsWith("..") && !path.isAbsolute(receiptRelative)) {
  fail("Grok receipt file must stay outside the repository");
}
const receiptStat = fs.statSync(receiptPath);
if (receiptStat.size <= 0 || receiptStat.size > 1024 * 1024) {
  fail("Grok receipt file size is invalid");
}
if (!fixtureMode && (receiptStat.mode & 0o777) !== 0o600) {
  fail("Grok receipt file must have mode 0600");
}

const secrets = [serverToken, routerToken].filter(usable);
function containsSecret(value) {
  const text = String(value || "");
  return (
    secrets.some((secret) => text.includes(secret)) ||
    /Bearer\s+[A-Za-z0-9._~+/-]{10,}/i.test(text) ||
    /\b(?:sk|xai|key|jwt)-[A-Za-z0-9_-]{8,}\b/i.test(text) ||
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
    "Grok Account list",
  );
  if (response?.ok !== true || !Array.isArray(response.accounts)) {
    fail("Grok Account list violated the control-plane contract");
  }
  const selector = accountSelector.toLowerCase();
  const account = exactOne(
    response.accounts.filter(
      (candidate) =>
        candidate?.providerType === "grok_oauth" &&
        (candidate.id === accountSelector ||
          candidate.email?.trim().toLowerCase() === selector),
    ),
    "Grok Account selector",
  );
  if (
    !Number.isSafeInteger(account.authIdentityGeneration) ||
    account.authIdentityGeneration < 1 ||
    !Number.isSafeInteger(account.tokenRefreshGeneration) ||
    account.tokenRefreshGeneration < 0 ||
    account.hasAccessToken !== true ||
    account.hasRefreshToken !== true ||
    account.hasApiKey === true ||
    account.needsRelogin === true
  ) {
    fail("Grok Account is not one live refreshable OAuth identity generation");
  }
  return account;
}

async function validateBinding(account) {
  const providerList = await requestJson(
    serverUrl,
    "/api/providers",
    adminHeaders(),
    "Grok Provider list",
  );
  if (providerList?.ok !== true || !Array.isArray(providerList.providers)) {
    fail("Grok Provider list violated the control-plane contract");
  }
  const view = exactOne(
    providerList.providers.filter(
      (candidate) => candidate?.app === "codex" && candidate?.provider?.id === providerId,
    ),
    "Grok Provider binding",
  );
  const authRef = view.runtime?.authRef;
  if (
    view.providerType !== "grok_oauth" ||
    view.providerTypeId !== "grok_oauth" ||
    view.runtime?.driverId !== "oauth.grok_responses" ||
    view.runtime?.configurationState !== "ready" ||
    authRef?.kind !== "managed_account" ||
    authRef.expectedProviderType !== "grok_oauth" ||
    authRef.accountId !== account.id ||
    authRef.authIdentityGeneration !== account.authIdentityGeneration
  ) {
    fail("Grok Provider is not fixed to the selected Account generation");
  }

  const shareList = await requestJson(
    serverUrl,
    "/api/shares",
    adminHeaders(),
    "Grok Share list",
  );
  if (shareList?.ok !== true || !Array.isArray(shareList.shares)) {
    fail("Grok Share list violated the control-plane contract");
  }
  const share = exactOne(
    shareList.shares.filter((candidate) => candidate?.id === shareId),
    "Grok Share",
  );
  const bindings = [
    { app: share.app, providerId: share.providerId, providerType: share.providerType },
    ...(Array.isArray(share.bindings) ? share.bindings : []),
  ];
  if (
    !bindings.some(
      (binding) =>
        binding?.app === "codex" &&
        binding?.providerId === providerId &&
        binding?.providerType === "grok_oauth",
    )
  ) {
    fail("Grok Share is not fixed to the selected Provider");
  }
  const providerRevision = view.providerRevision ?? view.provider?.revision ?? 0;
  const runtimeFingerprint = String(view.runtime.runtimeFingerprint || "");
  const shareRevision = share.configRevision ?? 0;
  if (
    !Number.isSafeInteger(providerRevision) ||
    providerRevision < 0 ||
    !runtimeFingerprint ||
    !Number.isSafeInteger(shareRevision) ||
    shareRevision < 0
  ) {
    fail("Grok Provider or Share revision scope is incomplete");
  }
  return { providerRevision, runtimeFingerprint, shareRevision };
}

async function validateModel() {
  const query = new URLSearchParams({ app: "codex", providerId });
  const catalog = await requestJson(
    shareUrl,
    `/v1/models?${query}`,
    shareHeaders(),
    "Grok model catalog",
  );
  if (!Array.isArray(catalog?.data)) {
    fail("Grok model catalog violated the public contract");
  }
  if (catalog.data.filter((entry) => entry?.id === model).length !== 1) {
    fail("Grok model catalog did not preserve the exact selected model");
  }
  if (
    typeof catalog.source !== "string" ||
    !/^[a-z0-9][a-z0-9_.:-]{0,127}$/i.test(catalog.source) ||
    catalog.stale !== false ||
    !Number.isSafeInteger(catalog.fetchedAtMs) ||
    catalog.fetchedAtMs <= 0
  ) {
    fail("Grok model catalog is not one fresh authoritative snapshot");
  }
  return {
    source: catalog.source,
    fetchedAtMs: catalog.fetchedAtMs,
    stale: false,
    digest: digest("cc-switch-server:grok-catalog:v1", catalog.data),
  };
}

function expectedDecisions() {
  const decisions = {
    crossAccountFallback: "disabled",
    crossProviderFallback: "disabled",
    crossShareFallback: "disabled",
    crossRailFallback: "disabled",
    postCommitReplay: "disabled",
    webCookie: "disabled",
    poolRouting: "disabled",
  };
  if (operation === "inference") {
    Object.assign(decisions, {
      sameAccount401: "replayed_once",
      second401: "terminal",
      reasoningRejection: "replayed_once_precommit",
      rateLimitScope: "bound_account",
      catalogStale: "same_identity_only",
      sessionTurnDrift: "terminal",
    });
  } else if (operation === "media") {
    Object.assign(decisions, {
      sameAccount401: "replayed_once",
      second401: "terminal",
      taskBinding: "sticky",
      rateLimitScope: "bound_account",
      invalidInput: "zero_upstream",
    });
  } else {
    Object.assign(decisions, {
      runtimeEnabled: "disabled",
      receiptAutoEnablesRuntime: "false",
      designReview: "required",
      accountCache: "isolated",
    });
  }
  return decisions;
}

function validateReceipt(receipt, scopeDigest, targetCommit, account, binding, catalog) {
  const fields = [
    "schemaVersion",
    "providerFamily",
    "operation",
    "verificationState",
    "liveState",
    "targetCommit",
    "harnessRevision",
    "recordedAt",
    "scopeDigest",
    "userNamespaceDigest",
    "sessionDigest",
    "turnIndex",
    "model",
    "catalog",
    "checks",
    "bodyHashes",
    "measurements",
    "generations",
    "decisions",
    "decoyRequests",
    "sensitiveScan",
  ];
  if (!receipt || typeof receipt !== "object" || Array.isArray(receipt)) {
    fail("Grok receipt must be an object");
  }
  if (!sameCanonical(Object.keys(receipt).sort(), [...fields].sort())) {
    fail("Grok receipt fields do not match schema version 1");
  }
  if (containsSecret(JSON.stringify(receipt))) {
    fail("Grok receipt contained secret-like material");
  }
  const operationContract = contract.realAcceptance.operations.find(
    (candidate) => candidate.operation === operation,
  );
  const userNamespaceDigest = digest(
    "cc-switch-server:grok-signed-user:v1",
    signedUser.toLowerCase(),
  );
  const sessionDigest = digest("cc-switch-server:grok-session:v1", sessionId);
  if (
    !operationContract ||
    receipt.schemaVersion !== contract.realAcceptance.receiptSchemaVersion ||
    receipt.providerFamily !== "grok" ||
    receipt.operation !== operation ||
    receipt.targetCommit !== targetCommit ||
    receipt.harnessRevision !== HARNESS_REVISION ||
    receipt.harnessRevision !== contract.realAcceptance.harnessRevision ||
    receipt.model !== model ||
    receipt.userNamespaceDigest !== userNamespaceDigest ||
    receipt.sessionDigest !== sessionDigest ||
    receipt.turnIndex !== turnIndex ||
    receipt.scopeDigest !== scopeDigest ||
    !/^[a-f0-9]{64}$/.test(receipt.scopeDigest)
  ) {
    fail("Grok receipt identity or scope does not match the selected operation");
  }
  const expectedVerification = fixtureMode ? "contract_verified" : "live_verified";
  const expectedLive = fixtureMode ? "live_pending" : "live_verified";
  if (
    receipt.verificationState !== expectedVerification ||
    receipt.liveState !== expectedLive
  ) {
    fail("Grok receipt evidence state is not valid for this harness mode");
  }
  const recordedAt = Date.parse(receipt.recordedAt);
  const now = Date.now();
  if (
    !Number.isFinite(recordedAt) ||
    recordedAt < now - RECEIPT_MAX_AGE_MS ||
    recordedAt > now + 5 * 60_000
  ) {
    fail("Grok receipt timestamp is outside the acceptance window");
  }
  if (!sameCanonical(receipt.catalog, catalog)) {
    fail("Grok receipt catalog does not match the fresh bound snapshot");
  }
  const checkKeys = Object.keys(receipt.checks || {}).sort();
  const requiredChecks = [...operationContract.requiredChecks].sort();
  if (!sameCanonical(checkKeys, requiredChecks)) {
    fail("Grok receipt check set is incomplete");
  }
  for (const check of requiredChecks) {
    if (receipt.checks[check] !== "pass") {
      fail(`Grok receipt check ${check} did not pass`);
    }
  }
  const hashKeys = Object.keys(receipt.bodyHashes || {}).sort();
  const requiredHashes = [...operationContract.requiredBodyHashes].sort();
  if (!sameCanonical(hashKeys, requiredHashes)) {
    fail("Grok receipt body hash set is incomplete");
  }
  for (const name of requiredHashes) {
    if (!/^[a-f0-9]{64}$/.test(receipt.bodyHashes[name])) {
      fail(`Grok receipt body hash ${name} is invalid`);
    }
  }
  const measurementKeys = Object.keys(receipt.measurements || {}).sort();
  const requiredMeasurements = [...operationContract.requiredMeasurements].sort();
  if (!sameCanonical(measurementKeys, requiredMeasurements)) {
    fail("Grok receipt measurement set is incomplete");
  }
  for (const name of requiredMeasurements) {
    if (
      !Number.isSafeInteger(receipt.measurements[name]) ||
      receipt.measurements[name] < 1
    ) {
      fail(`Grok receipt measurement ${name} is invalid`);
    }
  }
  const expectedGenerations = {
    authIdentityGeneration: account.authIdentityGeneration,
    tokenRefreshGeneration: account.tokenRefreshGeneration,
    providerRevision: binding.providerRevision,
    shareRevision: binding.shareRevision,
    credentialIdentityDigest: digest("cc-switch-server:grok-credential:v1", {
      accountId: account.id,
      authIdentityGeneration: account.authIdentityGeneration,
      tokenRefreshGeneration: account.tokenRefreshGeneration,
    }),
    providerBindingDigest: digest("cc-switch-server:grok-provider-binding:v1", {
      providerId,
      runtimeFingerprint: binding.runtimeFingerprint,
      accountId: account.id,
      authIdentityGeneration: account.authIdentityGeneration,
    }),
    shareBindingDigest: digest("cc-switch-server:grok-share-binding:v1", {
      shareId,
      shareRevision: binding.shareRevision,
      providerId,
      userNamespaceDigest,
    }),
  };
  if (!sameCanonical(receipt.generations, expectedGenerations)) {
    fail("Grok receipt generations do not match the selected binding");
  }
  if (!sameCanonical(receipt.decisions, expectedDecisions())) {
    fail("Grok receipt recovery decisions are incomplete");
  }
  const expectedDecoys = {
    otherAccount: 0,
    otherProvider: 0,
    otherShare: 0,
    otherRail: 0,
    otherSession: 0,
    webCookie: 0,
    poolRouter: 0,
  };
  if (
    !sameCanonical(receipt.decoyRequests, expectedDecoys) ||
    receipt.sensitiveScan?.status !== "pass" ||
    receipt.sensitiveScan?.matches !== 0 ||
    Object.keys(receipt.sensitiveScan || {}).length !== 2
  ) {
    fail("Grok receipt did not prove isolation and secret safety");
  }
}

async function main() {
  const targetCommit = gitCommit();
  const account = await validateAccount();
  const binding = await validateBinding(account);
  const catalog = await validateModel();
  const userNamespaceDigest = digest(
    "cc-switch-server:grok-signed-user:v1",
    signedUser.toLowerCase(),
  );
  const sessionDigest = digest("cc-switch-server:grok-session:v1", sessionId);
  const scopeDigest = digest("cc-switch-server:grok-real-scope:v1", {
    operation,
    targetCommit,
    accountId: account.id,
    authIdentityGeneration: account.authIdentityGeneration,
    tokenRefreshGeneration: account.tokenRefreshGeneration,
    providerId,
    providerRevision: binding.providerRevision,
    runtimeFingerprint: binding.runtimeFingerprint,
    shareId,
    shareRevision: binding.shareRevision,
    userNamespaceDigest,
    sessionDigest,
    turnIndex,
    model,
    catalog,
    harnessRevision: HARNESS_REVISION,
  });
  let receipt;
  try {
    receipt = JSON.parse(fs.readFileSync(receiptPath, "utf8"));
  } catch {
    fail("Grok receipt file is not valid JSON");
  }
  validateReceipt(receipt, scopeDigest, targetCommit, account, binding, catalog);
  const verificationState = fixtureMode ? "contract_verified" : "live_verified";
  const liveState = fixtureMode ? "live_pending" : "live_verified";
  console.log(
    `[PASS] Grok ${operation} private receipt accepted (verificationState=${verificationState}, liveState=${liveState})`,
  );
}

main().catch(() => {
  console.error("[FAIL] Grok acceptance failed (details redacted)");
  process.exitCode = 1;
});
