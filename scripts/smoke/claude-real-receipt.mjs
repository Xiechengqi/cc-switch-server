#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

// Each operation has its own private receipt. Fixture mode proves only that the
// validator is fail-closed; it can never promote an Anthropic operation to live.

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const contract = JSON.parse(
  fs.readFileSync(
    path.join(repoRoot, "assets/contract/claude-reference-delta.json"),
    "utf8",
  ),
);
const HARNESS_REVISION = 1;
const RECEIPT_MAX_AGE_MS = 24 * 60 * 60 * 1_000;
const PLAN_MAX_AGE_MS = 15 * 60 * 1_000;

const operationSpecs = Object.freeze({
  oauth_inference: Object.freeze({
    accountEnv: "CLAUDE_OAUTH_TEST_ACCOUNT",
    providerEnv: "CC_SWITCH_CLAUDE_OAUTH_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CLAUDE_OAUTH_SHARE_ID",
    modelEnv: "CC_SWITCH_CLAUDE_OAUTH_MODEL",
    receiptEnv: "CLAUDE_OAUTH_INFERENCE_REAL_RECEIPT_FILE",
    expectedPlan: null,
    exactModel: null,
  }),
  max_5x_plan: Object.freeze({
    accountEnv: "CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT",
    providerEnv: "CC_SWITCH_CLAUDE_MAX_5X_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CLAUDE_MAX_5X_SHARE_ID",
    modelEnv: "CC_SWITCH_CLAUDE_MAX_5X_MODEL",
    receiptEnv: "CLAUDE_MAX_5X_REAL_RECEIPT_FILE",
    expectedPlan: Object.freeze({
      planType: "claude_max_5x",
      planLabel: "Claude Max 5x",
    }),
    exactModel: null,
  }),
  max_20x_plan: Object.freeze({
    accountEnv: "CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT",
    providerEnv: "CC_SWITCH_CLAUDE_MAX_20X_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CLAUDE_MAX_20X_SHARE_ID",
    modelEnv: "CC_SWITCH_CLAUDE_MAX_20X_MODEL",
    receiptEnv: "CLAUDE_MAX_20X_REAL_RECEIPT_FILE",
    expectedPlan: Object.freeze({
      planType: "claude_max_20x",
      planLabel: "Claude Max 20x",
    }),
    exactModel: null,
  }),
  fable_5_1: Object.freeze({
    accountEnv: "CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT",
    providerEnv: "CC_SWITCH_CLAUDE_FABLE_5_1_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CLAUDE_FABLE_5_1_SHARE_ID",
    modelEnv: "CC_SWITCH_CLAUDE_FABLE_5_1_MODEL",
    receiptEnv: "CLAUDE_FABLE_5_1_REAL_RECEIPT_FILE",
    expectedPlan: Object.freeze({
      planType: "claude_max_20x",
      planLabel: "Claude Max 20x",
    }),
    exactModel: "claude-fable-5-1",
  }),
  opus_5_5: Object.freeze({
    accountEnv: "CLAUDE_OAUTH_TEST_ACCOUNT",
    providerEnv: "CC_SWITCH_CLAUDE_OPUS_5_5_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CLAUDE_OPUS_5_5_SHARE_ID",
    modelEnv: "CC_SWITCH_CLAUDE_OPUS_5_5_MODEL",
    receiptEnv: "CLAUDE_OPUS_5_5_REAL_RECEIPT_FILE",
    expectedPlan: null,
    exactModel: "claude-opus-5-5",
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

const operation = argValue("--operation", env("CC_SWITCH_CLAUDE_REAL_OPERATION"));
const spec = operationSpecs[operation];
if (!spec) {
  console.log(
    JSON.stringify({
      verificationState: "blocked_inputs",
      liveState: "live_pending",
      missingInputs: [
        "--operation oauth_inference|max_5x_plan|max_20x_plan|fable_5_1|opus_5_5",
      ],
    }),
  );
  process.exit(0);
}

const fixtureMode = env("CC_SWITCH_CLAUDE_HARNESS_MODE") === "fixture";
const serverUrl = env("SERVER_URL").replace(/\/+$/, "");
const shareUrl = env("CC_SWITCH_SHARE_URL").replace(/\/+$/, "");
const serverToken = env("CC_SWITCH_SERVER_TOKEN");
const routerToken = env("ROUTER_API_TOKEN");
const routerTokenHeader = env("ROUTER_API_TOKEN_HEADER", "Authorization");
const accountSelector = env(spec.accountEnv);
const providerId = env(spec.providerEnv);
const shareId = env(spec.shareEnv);
const model = env(spec.modelEnv);
const receiptFile = env(spec.receiptEnv, env("CLAUDE_REAL_RECEIPT_FILE"));
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
  [spec.providerEnv, providerId],
  [spec.shareEnv, shareId],
  [spec.modelEnv, model],
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
if (spec.exactModel && model !== spec.exactModel) {
  fail("Claude operation requires its exact canonical model");
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
  fail("Claude receipt file must be an absolute path");
}
const receiptPath = path.resolve(receiptFile);
const receiptRelative = path.relative(repoRoot, receiptPath);
if (!receiptRelative.startsWith("..") && !path.isAbsolute(receiptRelative)) {
  fail("Claude receipt file must stay outside the repository");
}
if (!fs.existsSync(receiptPath) || !fs.statSync(receiptPath).isFile()) {
  fail("Claude receipt file is unavailable");
}
if (!fixtureMode && (fs.statSync(receiptPath).mode & 0o777) !== 0o600) {
  fail("Claude receipt file must have mode 0600");
}

const secrets = [serverToken, routerToken].filter(usable);
function containsSecret(value) {
  const text = String(value || "");
  return (
    secrets.some((secret) => text.includes(secret)) ||
    /Bearer\s+[A-Za-z0-9._~+/-]{10,}/i.test(text) ||
    /\bsk-ant-[A-Za-z0-9_-]{8,}\b/i.test(text) ||
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
    "Claude Account list",
  );
  if (response?.ok !== true || !Array.isArray(response.accounts)) {
    fail("Claude Account list violated the control-plane contract");
  }
  const selector = accountSelector.toLowerCase();
  const account = exactOne(
    response.accounts.filter(
      (candidate) =>
        candidate?.providerType === "claude_oauth" &&
        (candidate.id === accountSelector ||
          candidate.email?.trim().toLowerCase() === selector),
    ),
    "Claude Account selector",
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
    fail("Claude Account is not a live OAuth identity generation");
  }
  return account;
}

async function validateBinding(account) {
  const providerList = await requestJson(
    serverUrl,
    "/api/providers",
    adminHeaders(),
    "Claude Provider list",
  );
  if (providerList?.ok !== true || !Array.isArray(providerList.providers)) {
    fail("Claude Provider list violated the control-plane contract");
  }
  const view = exactOne(
    providerList.providers.filter(
      (candidate) => candidate?.app === "claude" && candidate?.provider?.id === providerId,
    ),
    "Claude Provider binding",
  );
  const authRef = view.runtime?.authRef;
  if (
    view.providerType !== "claude_oauth" ||
    view.providerTypeId !== "claude_oauth" ||
    view.runtime?.driverId !== "oauth.claude_messages" ||
    view.runtime?.configurationState !== "ready" ||
    authRef?.kind !== "managed_account" ||
    authRef.expectedProviderType !== "claude_oauth" ||
    authRef.accountId !== account.id ||
    authRef.authIdentityGeneration !== account.authIdentityGeneration
  ) {
    fail("Claude Provider is not fixed to the selected Account generation");
  }

  const shareList = await requestJson(
    serverUrl,
    "/api/shares",
    adminHeaders(),
    "Claude Share list",
  );
  if (shareList?.ok !== true || !Array.isArray(shareList.shares)) {
    fail("Claude Share list violated the control-plane contract");
  }
  const share = exactOne(
    shareList.shares.filter((candidate) => candidate?.id === shareId),
    "Claude Share",
  );
  const bindings = [
    { app: share.app, providerId: share.providerId, providerType: share.providerType },
    ...(Array.isArray(share.bindings) ? share.bindings : []),
  ];
  if (
    !bindings.some(
      (binding) =>
        binding?.app === "claude" &&
        binding?.providerId === providerId &&
        binding?.providerType === "claude_oauth",
    )
  ) {
    fail("Claude Share is not fixed to the selected Provider");
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
    fail("Claude Provider or Share revision scope is incomplete");
  }
  return { providerRevision, runtimeFingerprint, shareRevision };
}

async function validateModel() {
  const query = new URLSearchParams({ app: "claude", providerId });
  const catalog = await requestJson(
    shareUrl,
    `/v1/models?${query}`,
    shareHeaders(),
    "Claude model catalog",
  );
  if (!Array.isArray(catalog?.data)) {
    fail("Claude model catalog violated the public contract");
  }
  if (catalog.data.filter((entry) => entry?.id === model).length !== 1) {
    fail("Claude model catalog did not preserve the exact selected model");
  }
}

async function validatePlan(account) {
  if (!spec.expectedPlan) return null;
  const payload = await requestJson(
    serverUrl,
    `/api/accounts/${encodeURIComponent(account.id)}/quota?refresh=true&force=true`,
    adminHeaders(),
    "Claude quota refresh",
  );
  const quota = payload?.quota;
  const subscription = quota?.extraUsage?.subscription;
  const evidence = quota?.extraUsage?.subscriptionEvidence;
  if (
    payload?.ok !== true ||
    payload.refreshed !== true ||
    quota?.success !== true ||
    payload.account?.id !== account.id ||
    payload.account?.providerType !== "claude_oauth" ||
    payload.account?.authIdentityGeneration !== account.authIdentityGeneration
  ) {
    fail("Claude quota refresh did not produce a fresh bound projection");
  }
  if (
    payload.account.subscriptionLevel !== spec.expectedPlan.planLabel ||
    quota.credentialMessage !== spec.expectedPlan.planLabel ||
    subscription?.planType !== spec.expectedPlan.planType ||
    subscription?.planLabel !== spec.expectedPlan.planLabel
  ) {
    fail("Claude quota refresh did not produce the expected canonical plan");
  }
  if (
    typeof subscription.planSource !== "string" ||
    !subscription.planSource ||
    subscription.planStale !== false ||
    !Number.isFinite(subscription.planObservedAt) ||
    evidence?.source !== subscription.planSource ||
    evidence?.stale !== false ||
    evidence?.observedAt !== subscription.planObservedAt ||
    evidence?.conflict !== false ||
    !/^[a-z0-9][a-z0-9_.:-]{0,127}$/i.test(subscription.planSource)
  ) {
    fail("Claude canonical plan evidence is stale, conflicting, or incomplete");
  }
  const now = Date.now();
  if (
    subscription.planObservedAt < now - PLAN_MAX_AGE_MS ||
    subscription.planObservedAt > now + 60_000
  ) {
    fail("Claude canonical plan evidence is outside the freshness window");
  }
  return {
    planType: subscription.planType,
    planLabel: subscription.planLabel,
    source: subscription.planSource,
    observedAt: subscription.planObservedAt,
    stale: false,
    conflict: false,
  };
}

function expectedDecisions() {
  const decisions = {
    crossAccountFallback: "disabled",
    crossProviderFallback: "disabled",
    crossShareFallback: "disabled",
  };
  if (operation === "oauth_inference") {
    Object.assign(decisions, {
      sameAccount401: "replayed_once",
      second401: "terminal",
      rateLimitScope: "bounded_exact_scope",
      terminalLateDisconnect: "complete",
    });
  } else if (operation === "fable_5_1") {
    Object.assign(decisions, {
      quotaRefresh: "forced",
      rateLimitScope: "fable_pool",
      modelFallback: "disabled",
    });
  } else if (operation === "opus_5_5") {
    Object.assign(decisions, {
      modelFallback: "disabled",
      context1mProbe: "bounded_small_request",
    });
  } else {
    decisions.quotaRefresh = "forced";
  }
  return decisions;
}

function validateReceipt(receipt, scopeDigest, targetCommit, account, binding, plan) {
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
    "model",
    "checks",
    "bodyHashes",
    "generations",
    "plan",
    "decisions",
    "decoyRequests",
    "sensitiveScan",
  ];
  if (!receipt || typeof receipt !== "object" || Array.isArray(receipt)) {
    fail("Claude receipt must be an object");
  }
  if (!sameCanonical(Object.keys(receipt).sort(), [...fields].sort())) {
    fail("Claude receipt fields do not match schema version 1");
  }
  if (containsSecret(JSON.stringify(receipt))) {
    fail("Claude receipt contained secret-like material");
  }
  const operationContract = [
    ...contract.realAcceptance.operations,
    ...(contract.realAcceptanceExtensions || []),
  ].find(
    (candidate) => candidate.operation === operation,
  );
  if (
    !operationContract ||
    receipt.schemaVersion !== contract.realAcceptance.receiptSchemaVersion ||
    receipt.providerFamily !== "claude" ||
    receipt.operation !== operation ||
    receipt.targetCommit !== targetCommit ||
    receipt.harnessRevision !== HARNESS_REVISION ||
    receipt.harnessRevision !== contract.realAcceptance.harnessRevision ||
    receipt.model !== model ||
    receipt.scopeDigest !== scopeDigest ||
    !/^[a-f0-9]{64}$/.test(receipt.scopeDigest)
  ) {
    fail("Claude receipt identity or scope does not match the selected operation");
  }
  const expectedVerification = fixtureMode ? "contract_verified" : "live_verified";
  const expectedLive = fixtureMode ? "live_pending" : "live_verified";
  if (
    receipt.verificationState !== expectedVerification ||
    receipt.liveState !== expectedLive
  ) {
    fail("Claude receipt evidence state is not valid for this harness mode");
  }
  const recordedAt = Date.parse(receipt.recordedAt);
  const now = Date.now();
  if (
    !Number.isFinite(recordedAt) ||
    recordedAt < now - RECEIPT_MAX_AGE_MS ||
    recordedAt > now + 5 * 60_000
  ) {
    fail("Claude receipt timestamp is outside the acceptance window");
  }
  const checkKeys = Object.keys(receipt.checks || {}).sort();
  const requiredChecks = [...operationContract.requiredChecks].sort();
  if (!sameCanonical(checkKeys, requiredChecks)) {
    fail("Claude receipt check set is incomplete");
  }
  for (const check of requiredChecks) {
    if (receipt.checks[check] !== "pass") {
      fail(`Claude receipt check ${check} did not pass`);
    }
  }
  const hashKeys = Object.keys(receipt.bodyHashes || {}).sort();
  const requiredHashes = [...operationContract.requiredBodyHashes].sort();
  if (!sameCanonical(hashKeys, requiredHashes)) {
    fail("Claude receipt body hash set is incomplete");
  }
  for (const name of requiredHashes) {
    if (!/^[a-f0-9]{64}$/.test(receipt.bodyHashes[name])) {
      fail(`Claude receipt body hash ${name} is invalid`);
    }
  }
  const expectedGenerations = {
    authIdentityGeneration: account.authIdentityGeneration,
    tokenRefreshGeneration: account.tokenRefreshGeneration,
    providerRevision: binding.providerRevision,
    shareRevision: binding.shareRevision,
    providerBindingDigest: digest("cc-switch-server:claude-provider-binding:v1", {
      providerId,
      runtimeFingerprint: binding.runtimeFingerprint,
      accountId: account.id,
      authIdentityGeneration: account.authIdentityGeneration,
    }),
  };
  if (!sameCanonical(receipt.generations, expectedGenerations)) {
    fail("Claude receipt generations do not match the selected binding");
  }
  if (!sameCanonical(receipt.plan ?? null, plan)) {
    fail("Claude receipt plan projection does not match the fresh control-plane view");
  }
  if (!sameCanonical(receipt.decisions, expectedDecisions())) {
    fail("Claude receipt recovery decisions are incomplete");
  }
  if (
    receipt.decoyRequests?.otherAccount !== 0 ||
    receipt.decoyRequests?.otherProvider !== 0 ||
    receipt.decoyRequests?.otherShare !== 0 ||
    Object.keys(receipt.decoyRequests || {}).length !== 3 ||
    receipt.sensitiveScan?.status !== "pass" ||
    receipt.sensitiveScan?.matches !== 0 ||
    Object.keys(receipt.sensitiveScan || {}).length !== 2
  ) {
    fail("Claude receipt did not prove isolation and secret safety");
  }
}

async function main() {
  const targetCommit = gitCommit();
  const account = await validateAccount();
  const binding = await validateBinding(account);
  await validateModel();
  const plan = await validatePlan(account);
  const scopeDigest = digest("cc-switch-server:claude-real-scope:v1", {
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
    model,
    plan,
    harnessRevision: HARNESS_REVISION,
  });
  let receipt;
  try {
    receipt = JSON.parse(fs.readFileSync(receiptPath, "utf8"));
  } catch {
    fail("Claude receipt file is not valid JSON");
  }
  validateReceipt(receipt, scopeDigest, targetCommit, account, binding, plan);
  const verificationState = fixtureMode ? "contract_verified" : "live_verified";
  const liveState = fixtureMode ? "live_pending" : "live_verified";
  console.log(
    `[PASS] Claude ${operation} private receipt accepted (verificationState=${verificationState}, liveState=${liveState})`,
  );
}

main().catch(() => {
  console.error("[FAIL] Claude acceptance failed (details redacted)");
  process.exitCode = 1;
});
