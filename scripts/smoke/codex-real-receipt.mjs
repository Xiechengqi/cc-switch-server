#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

// Private receipts are produced by an independently instrumented live run.
// Fixture mode proves only that this validator fails closed; it never promotes
// a Codex operation or publishes a gated model.

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const contract = JSON.parse(
  fs.readFileSync(
    path.join(repoRoot, "assets/contract/codex-reference-delta.json"),
    "utf8",
  ),
);
const HARNESS_REVISION = 1;
const RECEIPT_MAX_AGE_MS = 24 * 60 * 60 * 1_000;
let failureReported = false;

function reportFailure() {
  if (!failureReported) {
    failureReported = true;
    console.error("[FAIL] Codex acceptance failed (details redacted)");
  }
  process.exitCode = 1;
}

process.on("uncaughtException", reportFailure);
process.on("unhandledRejection", reportFailure);

const operationSpecs = Object.freeze({
  gpt_image_2_5: Object.freeze({
    kind: "image",
    providerEnv: "CC_SWITCH_CODEX_GPT_IMAGE_2_5_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CODEX_GPT_IMAGE_2_5_SHARE_ID",
    modelEnv: "CC_SWITCH_CODEX_GPT_IMAGE_2_5_MODEL",
    receiptEnv: "CODEX_GPT_IMAGE_2_5_REAL_RECEIPT_FILE",
    exactModel: "gpt-image-2.5",
  }),
  gpt_image_2_5_flare: Object.freeze({
    kind: "image",
    providerEnv: "CC_SWITCH_CODEX_GPT_IMAGE_2_5_FLARE_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CODEX_GPT_IMAGE_2_5_FLARE_SHARE_ID",
    modelEnv: "CC_SWITCH_CODEX_GPT_IMAGE_2_5_FLARE_MODEL",
    receiptEnv: "CODEX_GPT_IMAGE_2_5_FLARE_REAL_RECEIPT_FILE",
    exactModel: "gpt-image-2.5-flare",
  }),
  gpt_image_2_5_sunburst: Object.freeze({
    kind: "image",
    providerEnv: "CC_SWITCH_CODEX_GPT_IMAGE_2_5_SUNBURST_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CODEX_GPT_IMAGE_2_5_SUNBURST_SHARE_ID",
    modelEnv: "CC_SWITCH_CODEX_GPT_IMAGE_2_5_SUNBURST_MODEL",
    receiptEnv: "CODEX_GPT_IMAGE_2_5_SUNBURST_REAL_RECEIPT_FILE",
    exactModel: "gpt-image-2.5-sunburst",
  }),
  ws_prewarm: Object.freeze({
    kind: "websocket",
    providerEnv: "CC_SWITCH_CODEX_WS_PREWARM_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CODEX_WS_PREWARM_SHARE_ID",
    modelEnv: "CC_SWITCH_CODEX_WS_PREWARM_MODEL",
    receiptEnv: "CODEX_WS_PREWARM_REAL_RECEIPT_FILE",
    exactModel: null,
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
    const commit = execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: repoRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
    if (!/^[a-f0-9]{40}$/.test(commit)) fail("current target commit is invalid");
    return commit;
  } catch {
    fail("current target commit is unavailable");
  }
}

const operation = argValue("--operation", env("CC_SWITCH_CODEX_REAL_OPERATION"));
const spec = operationSpecs[operation];
if (!spec) {
  console.log(
    JSON.stringify({
      verificationState: "blocked_inputs",
      liveState: "live_pending",
      missingInputs: [
        "--operation gpt_image_2_5|gpt_image_2_5_flare|gpt_image_2_5_sunburst|ws_prewarm",
      ],
    }),
  );
  process.exit(0);
}

const fixtureMode = env("CC_SWITCH_CODEX_HARNESS_MODE") === "fixture";
const serverUrl = env("SERVER_URL").replace(/\/+$/, "");
const shareUrl = env("CC_SWITCH_SHARE_URL").replace(/\/+$/, "");
const serverToken = env("CC_SWITCH_SERVER_TOKEN");
const routerToken = env("ROUTER_API_TOKEN");
const routerTokenHeader = env("ROUTER_API_TOKEN_HEADER", "Authorization");
const accountSelector = env("CODEX_OAUTH_TEST_ACCOUNT");
const providerId = env(spec.providerEnv);
const shareId = env(spec.shareEnv);
const model = env(spec.modelEnv);
const receiptFile = env(spec.receiptEnv, env("CODEX_REAL_RECEIPT_FILE"));
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
  ["CODEX_OAUTH_TEST_ACCOUNT", accountSelector],
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
  fail("Codex image operation requires its exact canonical model");
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
  if (parsed.pathname !== "/") {
    fail(`${label} must not contain a path`);
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
  fail("Codex receipt file must be an absolute path");
}
const requestedReceiptPath = path.resolve(receiptFile);
const requestedReceiptRelative = path.relative(repoRoot, requestedReceiptPath);
if (
  !requestedReceiptRelative.startsWith("..") &&
  !path.isAbsolute(requestedReceiptRelative)
) {
  fail("Codex receipt file must stay outside the repository");
}
if (
  !fs.existsSync(requestedReceiptPath) ||
  !fs.statSync(requestedReceiptPath).isFile()
) {
  fail("Codex receipt file is unavailable");
}
const receiptPath = fs.realpathSync(requestedReceiptPath);
const receiptRelative = path.relative(repoRoot, receiptPath);
if (!receiptRelative.startsWith("..") && !path.isAbsolute(receiptRelative)) {
  fail("Codex receipt file must stay outside the repository");
}
const receiptStat = fs.statSync(receiptPath);
if (receiptStat.size <= 0 || receiptStat.size > 1024 * 1024) {
  fail("Codex receipt file size is invalid");
}
if (!fixtureMode && (receiptStat.mode & 0o777) !== 0o600) {
  fail("Codex receipt file must have mode 0600");
}

const secrets = [serverToken, routerToken].filter(usable);
function containsSecret(value) {
  const text = String(value || "");
  return (
    secrets.some((secret) => text.includes(secret)) ||
    /Bearer\s+[A-Za-z0-9._~+/-]{10,}/i.test(text) ||
    /\bsk-[A-Za-z0-9_-]{8,}\b/i.test(text) ||
    /\beyJ[A-Za-z0-9_-]{12,}\.[A-Za-z0-9_-]{12,}\.[A-Za-z0-9_-]{8,}\b/.test(text) ||
    /"(?:access|refresh|id)_token"\s*:\s*"[^"<][^"]+"/i.test(text) ||
    /\/v1\/images\/files\/[a-f0-9]{64}/i.test(text)
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
    "Codex Account list",
  );
  if (response?.ok !== true || !Array.isArray(response.accounts)) {
    fail("Codex Account list violated the control-plane contract");
  }
  const selector = accountSelector.toLowerCase();
  const account = exactOne(
    response.accounts.filter(
      (candidate) =>
        candidate?.providerType === "codex_oauth" &&
        (candidate.id === accountSelector ||
          candidate.email?.trim().toLowerCase() === selector),
    ),
    "Codex Account selector",
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
    fail("Codex Account is not a live OAuth identity generation");
  }
  return account;
}

async function validateBinding(account) {
  const providerList = await requestJson(
    serverUrl,
    "/api/providers",
    adminHeaders(),
    "Codex Provider list",
  );
  if (providerList?.ok !== true || !Array.isArray(providerList.providers)) {
    fail("Codex Provider list violated the control-plane contract");
  }
  const view = exactOne(
    providerList.providers.filter(
      (candidate) => candidate?.app === "codex" && candidate?.provider?.id === providerId,
    ),
    "Codex Provider binding",
  );
  const authRef = view.runtime?.authRef;
  if (
    view.providerType !== "codex_oauth" ||
    view.providerTypeId !== "codex_oauth" ||
    view.runtime?.driverId !== "oauth.openai_codex" ||
    view.runtime?.configurationState !== "ready" ||
    authRef?.kind !== "managed_account" ||
    authRef.expectedProviderType !== "codex_oauth" ||
    authRef.accountId !== account.id ||
    authRef.authIdentityGeneration !== account.authIdentityGeneration
  ) {
    fail("Codex Provider is not fixed to the selected Account generation");
  }

  const shareList = await requestJson(
    serverUrl,
    "/api/shares",
    adminHeaders(),
    "Codex Share list",
  );
  if (shareList?.ok !== true || !Array.isArray(shareList.shares)) {
    fail("Codex Share list violated the control-plane contract");
  }
  const share = exactOne(
    shareList.shares.filter((candidate) => candidate?.id === shareId),
    "Codex Share",
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
        binding?.providerType === "codex_oauth",
    )
  ) {
    fail("Codex Share is not fixed to the selected Provider");
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
    fail("Codex Provider or Share revision scope is incomplete");
  }
  return { providerRevision, runtimeFingerprint, shareRevision };
}

async function validateWebsocketModel() {
  if (spec.kind !== "websocket") return;
  const query = new URLSearchParams({ app: "codex", providerId });
  const catalog = await requestJson(
    shareUrl,
    `/v1/models?${query}`,
    shareHeaders(),
    "Codex model catalog",
  );
  if (!Array.isArray(catalog?.data)) {
    fail("Codex model catalog violated the public contract");
  }
  if (catalog.data.filter((entry) => entry?.id === model).length !== 1) {
    fail("Codex model catalog did not preserve the exact selected model");
  }
}

function expectedDecisions() {
  const common = {
    crossAccountFallback: "disabled",
    crossProviderFallback: "disabled",
    crossShareFallback: "disabled",
    crossRailFallback: "disabled",
    crossSiteFallback: "disabled",
    postCommitReplay: "disabled",
  };
  if (spec.kind === "image") {
    return {
      ...common,
      modelFallback: "disabled",
      quotaCooldownScope: "usage_limit_account_else_share_runtime_model",
      capabilityAccess: "same_share_host_authenticated",
    };
  }
  return {
    ...common,
    websocketHttpFallback: "pre_response_create_only",
    connectionReuse: "same_session_scope_only",
    prewarmActivation: "receipt_and_benchmark_gated",
  };
}

function validateMeasurements(measurements, operationContract) {
  const keys = Object.keys(measurements || {}).sort();
  if (!sameCanonical(keys, [...operationContract.requiredMeasurements].sort())) {
    fail("Codex receipt measurement set is incomplete");
  }
  if (spec.kind === "image") {
    for (const key of operationContract.requiredMeasurements) {
      if (!Number.isSafeInteger(measurements[key]) || measurements[key] < 1) {
        fail(`Codex image measurement ${key} is invalid`);
      }
    }
    if (
      measurements.generationRuns < 2 ||
      measurements.responsesRuns < 2 ||
      measurements.replicaCount < 2
    ) {
      fail("Codex image receipt did not cover both modes and two replicas");
    }
    return;
  }
  if (
    !Number.isSafeInteger(measurements.benchmarkSamples) ||
    measurements.benchmarkSamples < 5 ||
    !Number.isFinite(measurements.coldTtfbP50Ms) ||
    measurements.coldTtfbP50Ms <= 0 ||
    !Number.isFinite(measurements.prewarmedTtfbP50Ms) ||
    measurements.prewarmedTtfbP50Ms <= 0 ||
    measurements.prewarmedTtfbP50Ms >= measurements.coldTtfbP50Ms ||
    measurements.upstreamConnections !== 1 ||
    !Number.isSafeInteger(measurements.turns) ||
    measurements.turns < 2
  ) {
    fail("Codex WS receipt did not prove a stable prewarm benefit and reuse");
  }
}

function validateReceipt(receipt, scopeDigest, targetCommit, account, binding) {
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
    "measurements",
    "generations",
    "decisions",
    "decoyRequests",
    "sensitiveScan",
  ];
  if (!receipt || typeof receipt !== "object" || Array.isArray(receipt)) {
    fail("Codex receipt must be an object");
  }
  if (!sameCanonical(Object.keys(receipt).sort(), [...fields].sort())) {
    fail("Codex receipt fields do not match schema version 1");
  }
  if (containsSecret(JSON.stringify(receipt))) {
    fail("Codex receipt contained secret-like material");
  }
  const operationContract = contract.realAcceptance.operations.find(
    (candidate) => candidate.operation === operation,
  );
  if (
    !operationContract ||
    receipt.schemaVersion !== contract.realAcceptance.receiptSchemaVersion ||
    receipt.providerFamily !== "codex" ||
    receipt.operation !== operation ||
    receipt.targetCommit !== targetCommit ||
    receipt.harnessRevision !== HARNESS_REVISION ||
    receipt.harnessRevision !== contract.realAcceptance.harnessRevision ||
    receipt.model !== model ||
    operationContract.model !== spec.exactModel ||
    receipt.scopeDigest !== scopeDigest ||
    !/^[a-f0-9]{64}$/.test(receipt.scopeDigest)
  ) {
    fail("Codex receipt identity or scope does not match the selected operation");
  }
  const expectedVerification = fixtureMode ? "contract_verified" : "live_verified";
  const expectedLive = fixtureMode ? "live_pending" : "live_verified";
  if (
    receipt.verificationState !== expectedVerification ||
    receipt.liveState !== expectedLive
  ) {
    fail("Codex receipt evidence state is not valid for this harness mode");
  }
  const recordedAt = Date.parse(receipt.recordedAt);
  const now = Date.now();
  if (
    !Number.isFinite(recordedAt) ||
    recordedAt < now - RECEIPT_MAX_AGE_MS ||
    recordedAt > now + 5 * 60_000
  ) {
    fail("Codex receipt timestamp is outside the acceptance window");
  }
  const checkKeys = Object.keys(receipt.checks || {}).sort();
  const requiredChecks = [...operationContract.requiredChecks].sort();
  if (!sameCanonical(checkKeys, requiredChecks)) {
    fail("Codex receipt check set is incomplete");
  }
  for (const check of requiredChecks) {
    if (receipt.checks[check] !== "pass") {
      fail(`Codex receipt check ${check} did not pass`);
    }
  }
  const hashKeys = Object.keys(receipt.bodyHashes || {}).sort();
  const requiredHashes = [...operationContract.requiredBodyHashes].sort();
  if (!sameCanonical(hashKeys, requiredHashes)) {
    fail("Codex receipt body hash set is incomplete");
  }
  for (const name of requiredHashes) {
    if (!/^[a-f0-9]{64}$/.test(receipt.bodyHashes[name])) {
      fail(`Codex receipt body hash ${name} is invalid`);
    }
  }
  validateMeasurements(receipt.measurements, operationContract);
  const expectedGenerations = {
    authIdentityGeneration: account.authIdentityGeneration,
    tokenRefreshGeneration: account.tokenRefreshGeneration,
    providerRevision: binding.providerRevision,
    shareRevision: binding.shareRevision,
    providerBindingDigest: digest("cc-switch-server:codex-provider-binding:v1", {
      providerId,
      runtimeFingerprint: binding.runtimeFingerprint,
      accountId: account.id,
      authIdentityGeneration: account.authIdentityGeneration,
    }),
  };
  if (!sameCanonical(receipt.generations, expectedGenerations)) {
    fail("Codex receipt generations do not match the selected binding");
  }
  if (!sameCanonical(receipt.decisions, expectedDecisions())) {
    fail("Codex receipt recovery decisions are incomplete");
  }
  if (
    receipt.decoyRequests?.otherAccount !== 0 ||
    receipt.decoyRequests?.otherProvider !== 0 ||
    receipt.decoyRequests?.otherShare !== 0 ||
    receipt.decoyRequests?.otherRail !== 0 ||
    receipt.decoyRequests?.otherSite !== 0 ||
    Object.keys(receipt.decoyRequests || {}).length !== 5 ||
    receipt.sensitiveScan?.status !== "pass" ||
    receipt.sensitiveScan?.matches !== 0 ||
    Object.keys(receipt.sensitiveScan || {}).length !== 2
  ) {
    fail("Codex receipt did not prove isolation and secret safety");
  }
}

async function main() {
  const targetCommit = gitCommit();
  const account = await validateAccount();
  const binding = await validateBinding(account);
  await validateWebsocketModel();
  const scopeDigest = digest("cc-switch-server:codex-real-scope:v1", {
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
    harnessRevision: HARNESS_REVISION,
  });
  let receipt;
  try {
    receipt = JSON.parse(fs.readFileSync(receiptPath, "utf8"));
  } catch {
    fail("Codex receipt file is not valid JSON");
  }
  validateReceipt(receipt, scopeDigest, targetCommit, account, binding);
  const verificationState = fixtureMode ? "contract_verified" : "live_verified";
  const liveState = fixtureMode ? "live_pending" : "live_verified";
  console.log(
    `[PASS] Codex ${operation} private receipt accepted (verificationState=${verificationState}, liveState=${liveState})`,
  );
}

main().catch(reportFailure);
