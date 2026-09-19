#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

// This validator consumes one auth-kind/region-scoped private Kiro receipt.
// Fixture mode validates only the gate itself and can never promote live state.

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const contract = JSON.parse(
  fs.readFileSync(
    path.join(repoRoot, "assets/contract/kiro-reference-delta.json"),
    "utf8",
  ),
);
const HARNESS_REVISION = 1;
const RECEIPT_MAX_AGE_MS = 24 * 60 * 60 * 1_000;
const authKinds = new Set(["builder_id", "idc", "social", "api_key"]);
const regions = new Set(["us-east-1", "eu-central-1"]);

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

function scopeName(authKind, region) {
  return `${authKind}_${region}`.replaceAll("-", "_").toUpperCase();
}

function scopeSpec(authKind, region) {
  const scope = scopeName(authKind, region);
  return Object.freeze({
    scope,
    accountEnv: `KIRO_${scope}_TEST_ACCOUNT`,
    claudeProviderEnv: `CC_SWITCH_KIRO_${scope}_CLAUDE_PROVIDER_ID`,
    codexProviderEnv: `CC_SWITCH_KIRO_${scope}_CODEX_PROVIDER_ID`,
    shareEnv: `CC_SWITCH_KIRO_${scope}_SHARE_ID`,
    modelEnv: `CC_SWITCH_KIRO_${scope}_MODEL`,
    receiptEnv: `KIRO_${scope}_REAL_RECEIPT_FILE`,
  });
}

const authKind = argValue("--auth-kind", env("CC_SWITCH_KIRO_REAL_AUTH_KIND"));
const region = argValue("--region", env("CC_SWITCH_KIRO_REAL_REGION"));
if (!authKinds.has(authKind) || !regions.has(region)) {
  console.log(
    JSON.stringify({
      authKind: authKinds.has(authKind) ? authKind : null,
      region: regions.has(region) ? region : null,
      verificationState: "blocked_inputs",
      liveState: "live_pending",
      missingInputs: [
        "--auth-kind builder_id|idc|social|api_key",
        "--region us-east-1|eu-central-1",
      ],
    }),
  );
  process.exit(0);
}

const spec = scopeSpec(authKind, region);
const fixtureMode = env("CC_SWITCH_KIRO_HARNESS_MODE") === "fixture";
const serverUrl = env("SERVER_URL").replace(/\/+$/, "");
const shareUrl = env("CC_SWITCH_SHARE_URL").replace(/\/+$/, "");
const serverToken = env("CC_SWITCH_SERVER_TOKEN");
const routerToken = env("ROUTER_API_TOKEN");
const routerTokenHeader = env("ROUTER_API_TOKEN_HEADER", "Authorization");
const accountSelector = env(spec.accountEnv);
const claudeProviderId = env(spec.claudeProviderEnv);
const codexProviderId = env(spec.codexProviderEnv);
const shareId = env(spec.shareEnv);
const model = env(spec.modelEnv);
const signedUser = env("CC_SWITCH_KIRO_SIGNED_USER");
const sessionId = env("CC_SWITCH_KIRO_SESSION_ID");
const receiptFile = env(spec.receiptEnv);
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
  [spec.claudeProviderEnv, claudeProviderId],
  [spec.codexProviderEnv, codexProviderId],
  [spec.shareEnv, shareId],
  [spec.modelEnv, model],
  ["CC_SWITCH_KIRO_SIGNED_USER", signedUser],
  ["CC_SWITCH_KIRO_SESSION_ID", sessionId],
  [spec.receiptEnv, receiptFile],
];
const missingInputs = requiredInputs
  .filter(([, value]) => !usable(value))
  .map(([name]) => name);
if (missingInputs.length > 0) {
  console.log(
    JSON.stringify({
      authKind,
      region,
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
if (!/^[A-Za-z0-9][A-Za-z0-9._:@+-]{0,319}$/.test(signedUser)) {
  fail("CC_SWITCH_KIRO_SIGNED_USER is invalid");
}
if (!/^[A-Za-z0-9][A-Za-z0-9._:/-]{0,255}$/.test(sessionId)) {
  fail("CC_SWITCH_KIRO_SESSION_ID is invalid");
}
if (claudeProviderId === codexProviderId) {
  fail("Kiro Claude and Codex Provider IDs must be distinct");
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
  fail("Kiro receipt file must be an absolute path");
}
const requestedReceiptPath = path.resolve(receiptFile);
const requestedRelative = path.relative(repoRoot, requestedReceiptPath);
if (!requestedRelative.startsWith("..") && !path.isAbsolute(requestedRelative)) {
  fail("Kiro receipt file must stay outside the repository");
}
if (
  !fs.existsSync(requestedReceiptPath) ||
  !fs.statSync(requestedReceiptPath).isFile()
) {
  fail("Kiro receipt file is unavailable");
}
const receiptPath = fs.realpathSync(requestedReceiptPath);
const receiptRelative = path.relative(repoRoot, receiptPath);
if (!receiptRelative.startsWith("..") && !path.isAbsolute(receiptRelative)) {
  fail("Kiro receipt file must stay outside the repository");
}
const receiptStat = fs.statSync(receiptPath);
if (receiptStat.size <= 0 || receiptStat.size > 1024 * 1024) {
  fail("Kiro receipt file size is invalid");
}
if (!fixtureMode && (receiptStat.mode & 0o777) !== 0o600) {
  fail("Kiro receipt file must have mode 0600");
}

const secrets = [serverToken, routerToken].filter(usable);
function containsSecret(value) {
  const text = String(value || "");
  return (
    secrets.some((secret) => text.includes(secret)) ||
    /Bearer\s+[A-Za-z0-9._~+/-]{10,}/i.test(text) ||
    /\bksk_[A-Za-z0-9_-]{8,}\b/i.test(text) ||
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
    "Kiro Account list",
  );
  if (response?.ok !== true || !Array.isArray(response.accounts)) {
    fail("Kiro Account list violated the control-plane contract");
  }
  const selector = accountSelector.toLowerCase();
  const account = exactOne(
    response.accounts.filter(
      (candidate) =>
        candidate?.providerType === "kiro_oauth" &&
        (candidate.id === accountSelector ||
          candidate.email?.trim().toLowerCase() === selector),
    ),
    "Kiro Account selector",
  );
  const apiKey = authKind === "api_key";
  if (
    !Number.isSafeInteger(account.authIdentityGeneration) ||
    account.authIdentityGeneration < 1 ||
    !Number.isSafeInteger(account.tokenRefreshGeneration) ||
    account.tokenRefreshGeneration < 0 ||
    account.hasAccessToken !== true ||
    account.hasApiKey !== apiKey ||
    account.hasRefreshToken !== !apiKey ||
    account.needsRelogin === true ||
    account.hasProfile !== true ||
    account.hasRaw !== true ||
    (apiKey && String(account.tokenType || "").toUpperCase() !== "API_KEY")
  ) {
    fail("Kiro Account does not match the selected credential ownership contract");
  }
  return account;
}

function validateProvider(view, app, providerId, account) {
  const authRef = view.runtime?.authRef;
  if (
    view.app !== app ||
    view.provider?.id !== providerId ||
    view.providerType !== "kiro_oauth" ||
    view.providerTypeId !== "kiro_oauth" ||
    view.runtime?.driverId !== "special.kiro" ||
    view.runtime?.configurationState !== "ready" ||
    authRef?.kind !== "managed_account" ||
    authRef.expectedProviderType !== "kiro_oauth" ||
    authRef.accountId !== account.id ||
    authRef.authIdentityGeneration !== account.authIdentityGeneration
  ) {
    fail(`Kiro ${app} Provider is not fixed to the selected Account generation`);
  }
  const providerRevision = view.providerRevision ?? view.provider?.revision ?? 0;
  const runtimeFingerprint = String(view.runtime.runtimeFingerprint || "");
  if (
    !Number.isSafeInteger(providerRevision) ||
    providerRevision < 0 ||
    !runtimeFingerprint
  ) {
    fail(`Kiro ${app} Provider runtime scope is incomplete`);
  }
  return { providerRevision, runtimeFingerprint };
}

async function validateBinding(account) {
  const providerList = await requestJson(
    serverUrl,
    "/api/providers",
    adminHeaders(),
    "Kiro Provider list",
  );
  if (providerList?.ok !== true || !Array.isArray(providerList.providers)) {
    fail("Kiro Provider list violated the control-plane contract");
  }
  const claudeView = exactOne(
    providerList.providers.filter(
      (candidate) =>
        candidate?.app === "claude" && candidate?.provider?.id === claudeProviderId,
    ),
    "Kiro Claude Provider binding",
  );
  const codexView = exactOne(
    providerList.providers.filter(
      (candidate) =>
        candidate?.app === "codex" && candidate?.provider?.id === codexProviderId,
    ),
    "Kiro Codex Provider binding",
  );
  const claude = validateProvider(claudeView, "claude", claudeProviderId, account);
  const codex = validateProvider(codexView, "codex", codexProviderId, account);

  const shareList = await requestJson(
    serverUrl,
    "/api/shares",
    adminHeaders(),
    "Kiro Share list",
  );
  if (shareList?.ok !== true || !Array.isArray(shareList.shares)) {
    fail("Kiro Share list violated the control-plane contract");
  }
  const share = exactOne(
    shareList.shares.filter((candidate) => candidate?.id === shareId),
    "Kiro Share",
  );
  const bindings = [
    { app: share.app, providerId: share.providerId, providerType: share.providerType },
    ...(Array.isArray(share.bindings) ? share.bindings : []),
  ];
  for (const [app, providerId] of [
    ["claude", claudeProviderId],
    ["codex", codexProviderId],
  ]) {
    if (
      !bindings.some(
        (binding) =>
          binding?.app === app &&
          binding?.providerId === providerId &&
          binding?.providerType === "kiro_oauth",
      )
    ) {
      fail(`Kiro Share is not fixed to the selected ${app} Provider`);
    }
  }
  const shareRevision = share.configRevision ?? 0;
  if (!Number.isSafeInteger(shareRevision) || shareRevision < 0) {
    fail("Kiro Share revision scope is incomplete");
  }
  return { claude, codex, shareRevision };
}

async function validateModel(app, providerId) {
  const query = new URLSearchParams({ app, providerId });
  const catalog = await requestJson(
    shareUrl,
    `/v1/models?${query}`,
    shareHeaders(),
    `Kiro ${app} model catalog`,
  );
  if (!Array.isArray(catalog?.data)) {
    fail(`Kiro ${app} model catalog violated the public contract`);
  }
  if (catalog.data.filter((entry) => entry?.id === model).length !== 1) {
    fail(`Kiro ${app} model catalog did not preserve the exact selected model`);
  }
  if (
    typeof catalog.source !== "string" ||
    !/^[a-z0-9][a-z0-9_.:-]{0,127}$/i.test(catalog.source) ||
    catalog.stale !== false ||
    !Number.isSafeInteger(catalog.fetchedAtMs) ||
    catalog.fetchedAtMs <= 0
  ) {
    fail(`Kiro ${app} model catalog is not one fresh authoritative snapshot`);
  }
  return {
    source: catalog.source,
    fetchedAtMs: catalog.fetchedAtMs,
    stale: false,
    digest: digest(`cc-switch-server:kiro-${app}-catalog:v1`, catalog.data),
  };
}

function expectedDecisions() {
  return {
    sameAccount401:
      authKind === "api_key" ? "terminal_nonrefreshable" : "replayed_once_precommit",
    second401: "terminal",
    identityGenerationDrift: "terminal",
    crossAuthKindFallback: "disabled",
    crossRegionFallback: "disabled",
    crossAccountFallback: "disabled",
    crossProviderFallback: "disabled",
    crossShareFallback: "disabled",
    postCommitReplay: "disabled",
    catalogStale: "same_identity_only",
    compactRuntime: "disabled",
    receiptAutoEnablesCompact: "false",
    sharedCacheRuntime: "disabled",
  };
}

function expectedDecoys() {
  return {
    otherAccount: 0,
    otherProvider: 0,
    otherShare: 0,
    otherAuthKind: 0,
    otherRegion: 0,
    amazonQ: 0,
    webCookie: 0,
    poolRouter: 0,
  };
}

function expectedGenerations(account, binding, userNamespaceDigest) {
  return {
    authIdentityGeneration: account.authIdentityGeneration,
    tokenRefreshGeneration: account.tokenRefreshGeneration,
    claudeProviderRevision: binding.claude.providerRevision,
    codexProviderRevision: binding.codex.providerRevision,
    shareRevision: binding.shareRevision,
    credentialIdentityDigest: digest("cc-switch-server:kiro-credential:v1", {
      accountId: account.id,
      authKind,
      region,
      authIdentityGeneration: account.authIdentityGeneration,
      tokenRefreshGeneration: account.tokenRefreshGeneration,
    }),
    claudeBindingDigest: digest("cc-switch-server:kiro-provider-binding:v1", {
      app: "claude",
      providerId: claudeProviderId,
      runtimeFingerprint: binding.claude.runtimeFingerprint,
      accountId: account.id,
      authIdentityGeneration: account.authIdentityGeneration,
    }),
    codexBindingDigest: digest("cc-switch-server:kiro-provider-binding:v1", {
      app: "codex",
      providerId: codexProviderId,
      runtimeFingerprint: binding.codex.runtimeFingerprint,
      accountId: account.id,
      authIdentityGeneration: account.authIdentityGeneration,
    }),
    shareBindingDigest: digest("cc-switch-server:kiro-share-binding:v1", {
      shareId,
      shareRevision: binding.shareRevision,
      claudeProviderId,
      codexProviderId,
      userNamespaceDigest,
    }),
  };
}

function validateNamedPasses(receipt, field, required, label) {
  const keys = Object.keys(receipt[field] || {}).sort();
  if (!sameCanonical(keys, [...required].sort())) {
    fail(`Kiro receipt ${label} set is incomplete`);
  }
  for (const name of required) {
    if (receipt[field][name] !== "pass") {
      fail(`Kiro receipt ${label} ${name} did not pass`);
    }
  }
}

function validateReceipt(receipt, context) {
  const fields = [
    "schemaVersion",
    "providerFamily",
    "authKind",
    "region",
    "verificationState",
    "liveState",
    "targetCommit",
    "harnessRevision",
    "recordedAt",
    "scopeDigest",
    "userNamespaceDigest",
    "sessionDigest",
    "model",
    "catalogs",
    "checks",
    "bodyHashes",
    "measurements",
    "generations",
    "decisions",
    "decoyRequests",
    "sensitiveScan",
  ];
  if (!receipt || typeof receipt !== "object" || Array.isArray(receipt)) {
    fail("Kiro receipt must be an object");
  }
  if (!sameCanonical(Object.keys(receipt).sort(), [...fields].sort())) {
    fail("Kiro receipt fields do not match schema version 1");
  }
  if (containsSecret(JSON.stringify(receipt))) {
    fail("Kiro receipt contained secret-like material");
  }
  if (
    receipt.schemaVersion !== contract.realAcceptance.receiptSchemaVersion ||
    receipt.providerFamily !== "kiro" ||
    receipt.authKind !== authKind ||
    receipt.region !== region ||
    receipt.targetCommit !== context.targetCommit ||
    receipt.harnessRevision !== HARNESS_REVISION ||
    receipt.harnessRevision !== contract.realAcceptance.harnessRevision ||
    receipt.model !== model ||
    receipt.userNamespaceDigest !== context.userNamespaceDigest ||
    receipt.sessionDigest !== context.sessionDigest ||
    receipt.scopeDigest !== context.scopeDigest ||
    !/^[a-f0-9]{64}$/.test(receipt.scopeDigest)
  ) {
    fail("Kiro receipt identity or scope does not match the selected auth kind and region");
  }
  const expectedVerification = fixtureMode ? "contract_verified" : "live_verified";
  const expectedLive = fixtureMode ? "live_pending" : "live_verified";
  if (
    receipt.verificationState !== expectedVerification ||
    receipt.liveState !== expectedLive
  ) {
    fail("Kiro receipt evidence state is not valid for this harness mode");
  }
  const recordedAt = Date.parse(receipt.recordedAt);
  const now = Date.now();
  if (
    !Number.isFinite(recordedAt) ||
    recordedAt < now - RECEIPT_MAX_AGE_MS ||
    recordedAt > now + 5 * 60_000
  ) {
    fail("Kiro receipt timestamp is outside the acceptance window");
  }
  if (!sameCanonical(receipt.catalogs, context.catalogs)) {
    fail("Kiro receipt catalogs do not match the fresh bound snapshots");
  }
  validateNamedPasses(
    receipt,
    "checks",
    contract.realAcceptance.requiredChecks,
    "check",
  );
  const hashKeys = Object.keys(receipt.bodyHashes || {}).sort();
  if (
    !sameCanonical(hashKeys, [...contract.realAcceptance.requiredBodyHashes].sort())
  ) {
    fail("Kiro receipt body hash set is incomplete");
  }
  for (const name of contract.realAcceptance.requiredBodyHashes) {
    if (!/^[a-f0-9]{64}$/.test(receipt.bodyHashes[name])) {
      fail(`Kiro receipt body hash ${name} is invalid`);
    }
  }
  const measurementKeys = Object.keys(receipt.measurements || {}).sort();
  if (
    !sameCanonical(
      measurementKeys,
      [...contract.realAcceptance.requiredMeasurements].sort(),
    )
  ) {
    fail("Kiro receipt measurement set is incomplete");
  }
  for (const name of contract.realAcceptance.requiredMeasurements) {
    if (
      !Number.isSafeInteger(receipt.measurements[name]) ||
      receipt.measurements[name] < 1
    ) {
      fail(`Kiro receipt measurement ${name} is invalid`);
    }
  }
  if (!sameCanonical(receipt.generations, context.generations)) {
    fail("Kiro receipt generations do not match the selected bindings");
  }
  if (
    !sameCanonical(
      Object.keys(receipt.decisions || {}).sort(),
      [...contract.realAcceptance.requiredDecisions].sort(),
    ) ||
    !sameCanonical(receipt.decisions, expectedDecisions())
  ) {
    fail("Kiro receipt recovery decisions are incomplete");
  }
  if (
    !sameCanonical(receipt.decoyRequests, expectedDecoys()) ||
    receipt.sensitiveScan?.status !== "pass" ||
    receipt.sensitiveScan?.matches !== 0 ||
    Object.keys(receipt.sensitiveScan || {}).length !== 2
  ) {
    fail("Kiro receipt did not prove isolation and secret safety");
  }
}

async function main() {
  const targetCommit = gitCommit();
  const account = await validateAccount();
  const binding = await validateBinding(account);
  const catalogs = {
    claude: await validateModel("claude", claudeProviderId),
    codex: await validateModel("codex", codexProviderId),
  };
  const userNamespaceDigest = digest(
    "cc-switch-server:kiro-signed-user:v1",
    signedUser.toLowerCase(),
  );
  const sessionDigest = digest("cc-switch-server:kiro-session:v1", sessionId);
  const generations = expectedGenerations(account, binding, userNamespaceDigest);
  const scopeDigest = digest("cc-switch-server:kiro-real-scope:v1", {
    authKind,
    region,
    targetCommit,
    accountId: account.id,
    generations,
    claudeProviderId,
    codexProviderId,
    shareId,
    userNamespaceDigest,
    sessionDigest,
    model,
    catalogs,
    harnessRevision: HARNESS_REVISION,
  });
  let receipt;
  try {
    receipt = JSON.parse(fs.readFileSync(receiptPath, "utf8"));
  } catch {
    fail("Kiro receipt file is not valid JSON");
  }
  validateReceipt(receipt, {
    targetCommit,
    userNamespaceDigest,
    sessionDigest,
    scopeDigest,
    catalogs,
    generations,
  });
  const verificationState = fixtureMode ? "contract_verified" : "live_verified";
  const liveState = fixtureMode ? "live_pending" : "live_verified";
  console.log(
    `[PASS] Kiro ${authKind}/${region} private receipt accepted (verificationState=${verificationState}, liveState=${liveState})`,
  );
}

main().catch(() => {
  console.error("[FAIL] Kiro acceptance failed (details redacted)");
  process.exitCode = 1;
});
