#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// This validator consumes one site-scoped private CodeBuddy receipt. The
// destructive login/rotation/12153/empty-catalog/401 checks are performed by a
// private harness; this file validates their receipt against the live Server
// binding. Fixture mode exercises only this validator and cannot promote live
// state.

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const contract = JSON.parse(
  fs.readFileSync(
    path.join(repoRoot, "assets/contract/codebuddy-reference-delta.json"),
    "utf8",
  ),
);
const HARNESS_REVISION = 2;
const RECEIPT_MAX_AGE_MS = 24 * 60 * 60 * 1_000;
const CATALOG_MAX_AGE_MS = 2 * 60 * 60 * 1_000;
const siteSpecs = Object.freeze({
  intl: Object.freeze({
    accountEnv: "CODEBUDDY_INTL_TEST_ACCOUNT",
    providerEnvs: Object.freeze({
      claude: "CC_SWITCH_CODEBUDDY_INTL_CLAUDE_PROVIDER_ID",
      codex: "CC_SWITCH_CODEBUDDY_INTL_CODEX_PROVIDER_ID",
      gemini: "CC_SWITCH_CODEBUDDY_INTL_GEMINI_PROVIDER_ID",
    }),
    shareEnv: "CC_SWITCH_CODEBUDDY_INTL_SHARE_ID",
    modelEnv: "CC_SWITCH_CODEBUDDY_INTL_MODEL",
    receiptEnv: "CODEBUDDY_INTL_REAL_RECEIPT_FILE",
  }),
  cn: Object.freeze({
    accountEnv: "CODEBUDDY_CN_TEST_ACCOUNT",
    providerEnvs: Object.freeze({
      claude: "CC_SWITCH_CODEBUDDY_CN_CLAUDE_PROVIDER_ID",
      codex: "CC_SWITCH_CODEBUDDY_CN_CODEX_PROVIDER_ID",
      gemini: "CC_SWITCH_CODEBUDDY_CN_GEMINI_PROVIDER_ID",
    }),
    shareEnv: "CC_SWITCH_CODEBUDDY_CN_SHARE_ID",
    modelEnv: "CC_SWITCH_CODEBUDDY_CN_MODEL",
    receiptEnv: "CODEBUDDY_CN_REAL_RECEIPT_FILE",
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

function sameCanonical(left, right) {
  return JSON.stringify(canonical(left)) === JSON.stringify(canonical(right));
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

const site = argValue("--site", env("CC_SWITCH_CODEBUDDY_REAL_SITE"));
const spec = siteSpecs[site];
if (!spec) {
  console.log(
    JSON.stringify({
      site: null,
      verificationState: "blocked_inputs",
      liveState: "live_pending",
      missingInputs: ["--site intl|cn"],
    }),
  );
  process.exit(0);
}

const fixtureMode = env("CC_SWITCH_CODEBUDDY_HARNESS_MODE") === "fixture";
const serverUrl = env("SERVER_URL").replace(/\/+$/, "");
const shareUrl = env("CC_SWITCH_SHARE_URL").replace(/\/+$/, "");
const serverToken = env("CC_SWITCH_SERVER_TOKEN");
const routerToken = env("ROUTER_API_TOKEN");
const routerTokenHeader = env("ROUTER_API_TOKEN_HEADER", "Authorization");
const accountSelector = env(spec.accountEnv);
const providerIds = Object.fromEntries(
  Object.entries(spec.providerEnvs).map(([app, name]) => [app, env(name)]),
);
const shareId = env(spec.shareEnv);
const model = env(spec.modelEnv);
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
  ...Object.entries(spec.providerEnvs).map(([app, name]) => [name, providerIds[app]]),
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
      site,
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
if (new Set(Object.values(providerIds)).size !== 3) {
  fail("CodeBuddy Claude, Codex, and Gemini Provider IDs must be distinct");
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
  fail("CodeBuddy receipt file must be an absolute path");
}
const requestedReceiptPath = path.resolve(receiptFile);
const requestedRelative = path.relative(repoRoot, requestedReceiptPath);
if (!requestedRelative.startsWith("..") && !path.isAbsolute(requestedRelative)) {
  fail("CodeBuddy receipt file must stay outside the repository");
}
if (
  !fs.existsSync(requestedReceiptPath) ||
  !fs.statSync(requestedReceiptPath).isFile()
) {
  fail("CodeBuddy receipt file is unavailable");
}
const receiptPath = fs.realpathSync(requestedReceiptPath);
const receiptRelative = path.relative(repoRoot, receiptPath);
if (!receiptRelative.startsWith("..") && !path.isAbsolute(receiptRelative)) {
  fail("CodeBuddy receipt file must stay outside the repository");
}
const receiptStat = fs.statSync(receiptPath);
if (receiptStat.size <= 0 || receiptStat.size > 1024 * 1024) {
  fail("CodeBuddy receipt file size is invalid");
}
if (!fixtureMode && (receiptStat.mode & 0o777) !== 0o600) {
  fail("CodeBuddy receipt file must have mode 0600");
}

const secrets = [serverToken, routerToken].filter(usable);
function containsSecret(value) {
  const text = String(value || "");
  return (
    secrets.some((secret) => text.includes(secret)) ||
    /Bearer\s+[A-Za-z0-9._~+/-]{10,}/i.test(text) ||
    /\beyJ[A-Za-z0-9_-]{12,}\.[A-Za-z0-9_-]{12,}\.[A-Za-z0-9_-]{8,}\b/.test(text) ||
    /"(?:access|refresh|id)_?token"\s*:\s*"[^"<][^"]+"/i.test(text) ||
    /"(?:cookie|authorization)"\s*:\s*"[^"<][^"]+"/i.test(text)
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
    "CodeBuddy Account list",
  );
  if (response?.ok !== true || !Array.isArray(response.accounts)) {
    fail("CodeBuddy Account list violated the control-plane contract");
  }
  const selector = accountSelector.toLowerCase();
  const account = exactOne(
    response.accounts.filter(
      (candidate) =>
        candidate?.providerType === "codebuddy_oauth" &&
        (candidate.id === accountSelector ||
          candidate.email?.trim().toLowerCase() === selector),
    ),
    "CodeBuddy Account selector",
  );
  if (
    !Number.isSafeInteger(account.authIdentityGeneration) ||
    account.authIdentityGeneration < 1 ||
    !Number.isSafeInteger(account.tokenRefreshGeneration) ||
    account.tokenRefreshGeneration < 1 ||
    account.hasAccessToken !== true ||
    account.hasRefreshToken !== true ||
    account.hasApiKey === true ||
    account.needsRelogin === true ||
    account.hasProfile !== true ||
    account.hasRaw !== true
  ) {
    fail("CodeBuddy Account does not match the OAuth credential ownership contract");
  }
  return account;
}

function validateProvider(view, app, providerId, account) {
  const authRef = view.runtime?.authRef;
  if (
    view.app !== app ||
    view.provider?.id !== providerId ||
    view.providerType !== "codebuddy_oauth" ||
    view.providerTypeId !== "codebuddy_oauth" ||
    view.runtime?.driverId !== "special.codebuddy_oauth" ||
    view.runtime?.configurationState !== "ready" ||
    authRef?.kind !== "managed_account" ||
    authRef.expectedProviderType !== "codebuddy_oauth" ||
    authRef.accountId !== account.id ||
    authRef.authIdentityGeneration !== account.authIdentityGeneration
  ) {
    fail(`CodeBuddy ${app} Provider is not fixed to the selected Account generation`);
  }
  const providerRevision = view.providerRevision ?? view.provider?.revision ?? 0;
  const runtimeFingerprint = String(view.runtime.runtimeFingerprint || "");
  if (!Number.isSafeInteger(providerRevision) || providerRevision < 0 || !runtimeFingerprint) {
    fail(`CodeBuddy ${app} Provider runtime scope is incomplete`);
  }
  return { providerRevision, runtimeFingerprint };
}

async function validateBindings(account) {
  const providerList = await requestJson(
    serverUrl,
    "/api/providers",
    adminHeaders(),
    "CodeBuddy Provider list",
  );
  if (providerList?.ok !== true || !Array.isArray(providerList.providers)) {
    fail("CodeBuddy Provider list violated the control-plane contract");
  }
  const providers = {};
  for (const [app, providerId] of Object.entries(providerIds)) {
    const view = exactOne(
      providerList.providers.filter(
        (candidate) => candidate?.app === app && candidate?.provider?.id === providerId,
      ),
      `CodeBuddy ${app} Provider binding`,
    );
    providers[app] = validateProvider(view, app, providerId, account);
  }

  const shareList = await requestJson(
    serverUrl,
    "/api/shares",
    adminHeaders(),
    "CodeBuddy Share list",
  );
  if (shareList?.ok !== true || !Array.isArray(shareList.shares)) {
    fail("CodeBuddy Share list violated the control-plane contract");
  }
  const share = exactOne(
    shareList.shares.filter((candidate) => candidate?.id === shareId),
    "CodeBuddy Share",
  );
  if (share.enabled === false || (share.status && share.status !== "active")) {
    fail("selected CodeBuddy Share is not active");
  }
  const bindings = [
    { app: share.app, providerId: share.providerId, providerType: share.providerType },
    ...(Array.isArray(share.bindings) ? share.bindings : []),
  ];
  for (const [app, providerId] of Object.entries(providerIds)) {
    if (
      !bindings.some(
        (binding) =>
          binding?.app === app &&
          binding?.providerId === providerId &&
          binding?.providerType === "codebuddy_oauth",
      )
    ) {
      fail(`CodeBuddy Share is not fixed to the selected ${app} Provider`);
    }
  }
  const shareRevision = share.configRevision ?? 0;
  if (!Number.isSafeInteger(shareRevision) || shareRevision < 0) {
    fail("CodeBuddy Share revision scope is incomplete");
  }
  return { providers, shareRevision };
}

async function validateCatalog(app, providerId) {
  const query = new URLSearchParams({ app, providerId });
  const catalog = await requestJson(
    shareUrl,
    `/v1/models?${query}`,
    shareHeaders(),
    `CodeBuddy ${app} model catalog`,
  );
  const now = Date.now();
  if (
    !Array.isArray(catalog?.data) ||
    catalog.source !== "codebuddy_live_model_catalog" ||
    catalog.stale !== false ||
    !Number.isSafeInteger(catalog.fetchedAtMs) ||
    catalog.fetchedAtMs < now - CATALOG_MAX_AGE_MS ||
    catalog.fetchedAtMs > now + 5 * 60_000
  ) {
    fail(`CodeBuddy ${app} model catalog is not one fresh bound-account snapshot`);
  }
  if (catalog.data.filter((entry) => entry?.id === model).length !== 1) {
    fail(`CodeBuddy ${app} model catalog did not preserve the exact selected model`);
  }
  return {
    source: catalog.source,
    fetchedAtMs: catalog.fetchedAtMs,
    stale: false,
    digest: digest(`cc-switch-server:codebuddy-${app}-catalog:v2`, catalog.data),
  };
}

function expectedGenerations(account, binding) {
  const surfaceBindings = Object.fromEntries(
    Object.entries(binding.providers).map(([app, value]) => [
      app,
      {
        providerRevision: value.providerRevision,
        runtimeFingerprintDigest: digest(
          `cc-switch-server:codebuddy-${app}-runtime:v2`,
          value.runtimeFingerprint,
        ),
      },
    ]),
  );
  const accountIdentityDigest = digest("cc-switch-server:codebuddy-account:v2", {
    accountId: account.id,
    site,
    authIdentityGeneration: account.authIdentityGeneration,
    tokenRefreshGeneration: account.tokenRefreshGeneration,
  });
  const providerBindingDigest = digest("cc-switch-server:codebuddy-provider-bindings:v2", {
    accountIdentityDigest,
    bindings: Object.entries(providerIds)
      .map(([app, providerId]) => ({ app, providerId, ...surfaceBindings[app] }))
      .sort((left, right) => left.app.localeCompare(right.app)),
  });
  const shareBindingDigest = digest("cc-switch-server:codebuddy-share-binding:v2", {
    shareId,
    shareRevision: binding.shareRevision,
    bindings: Object.entries(providerIds)
      .map(([app, providerId]) => ({ app, providerId, providerType: "codebuddy_oauth" }))
      .sort((left, right) => left.app.localeCompare(right.app)),
  });
  return {
    authIdentityGeneration: account.authIdentityGeneration,
    tokenRefreshGeneration: account.tokenRefreshGeneration,
    accountIdentityDigest,
    providerBindingDigest,
    surfaceBindings,
    shareRevision: binding.shareRevision,
    shareBindingDigest,
  };
}

function expectedDecisions() {
  return {
    sameAccountFirst401: "replayed_once_precommit",
    second401: "terminal",
    identityGenerationDrift: "terminal",
    crossSiteFallback: "disabled",
    crossDomainFallback: "disabled",
    crossAccountFallback: "disabled",
    crossProviderFallback: "disabled",
    crossShareFallback: "disabled",
    postCommitReplay: "disabled",
    authoritativeEmptyCatalog: "empty_success",
    catalogStale: "same_identity_only",
    enterpriseRuntime: "disabled",
    multimodalRuntime: "disabled",
    deepseekV41Runtime: "disabled_without_separate_evidence",
    receiptAutoEnablesCapabilities: "false",
  };
}

function expectedDecoys() {
  return {
    otherAccount: 0,
    otherProvider: 0,
    otherShare: 0,
    otherSite: 0,
    otherDomain: 0,
    poolRouter: 0,
    enterprise: 0,
    multimodal: 0,
  };
}

function validateNamedPasses(receipt, field, required, label) {
  const keys = Object.keys(receipt[field] || {}).sort();
  if (!sameCanonical(keys, [...required].sort())) {
    fail(`CodeBuddy receipt ${label} set is incomplete`);
  }
  for (const name of required) {
    if (receipt[field][name] !== "pass") {
      fail(`CodeBuddy receipt ${label} ${name} did not pass`);
    }
  }
}

function validateReceipt(receipt, context) {
  const fields = [
    "schemaVersion",
    "providerFamily",
    "site",
    "verificationState",
    "liveState",
    "targetCommit",
    "harnessRevision",
    "recordedAt",
    "scopeDigest",
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
    fail("CodeBuddy receipt must be an object");
  }
  if (!sameCanonical(Object.keys(receipt).sort(), fields.sort())) {
    fail("CodeBuddy receipt fields do not match schema version 2");
  }
  if (containsSecret(JSON.stringify(receipt))) {
    fail("CodeBuddy receipt contained secret-like material");
  }
  if (
    receipt.schemaVersion !== contract.realAcceptance.receiptSchemaVersion ||
    receipt.providerFamily !== "codebuddy" ||
    receipt.site !== site ||
    receipt.targetCommit !== context.targetCommit ||
    receipt.harnessRevision !== HARNESS_REVISION ||
    receipt.harnessRevision !== contract.realAcceptance.harnessRevision ||
    receipt.model !== model
  ) {
    fail("CodeBuddy receipt identity does not match the selected site and target");
  }
  const expectedVerification = fixtureMode ? "contract_verified" : "live_verified";
  const expectedLive = fixtureMode ? "live_pending" : "live_verified";
  if (
    receipt.verificationState !== expectedVerification ||
    receipt.liveState !== expectedLive
  ) {
    fail("CodeBuddy receipt evidence state is not valid for this harness mode");
  }
  const recordedAt = Date.parse(receipt.recordedAt);
  const now = Date.now();
  if (
    !Number.isFinite(recordedAt) ||
    recordedAt < now - RECEIPT_MAX_AGE_MS ||
    recordedAt > now + 5 * 60_000
  ) {
    fail("CodeBuddy receipt timestamp is outside the acceptance window");
  }
  if (!sameCanonical(receipt.catalogs, context.catalogs)) {
    fail("CodeBuddy receipt catalogs do not match the fresh bound snapshots");
  }
  validateNamedPasses(receipt, "checks", contract.realAcceptance.requiredChecks, "check");

  const hashKeys = Object.keys(receipt.bodyHashes || {}).sort();
  if (!sameCanonical(hashKeys, [...contract.realAcceptance.requiredBodyHashes].sort())) {
    fail("CodeBuddy receipt body hash set is incomplete");
  }
  for (const name of contract.realAcceptance.requiredBodyHashes) {
    if (!/^[a-f0-9]{64}$/.test(receipt.bodyHashes[name])) {
      fail(`CodeBuddy receipt body hash ${name} is invalid`);
    }
  }

  const measurementKeys = Object.keys(receipt.measurements || {}).sort();
  if (
    !sameCanonical(
      measurementKeys,
      [...contract.realAcceptance.requiredMeasurements].sort(),
    )
  ) {
    fail("CodeBuddy receipt measurement set is incomplete");
  }
  const minimums = {
    catalogRuns: 3,
    claudeRuns: 2,
    codexRuns: 2,
    geminiRuns: 2,
    recoveryRuns: 2,
    terminalEofRuns: 6,
  };
  for (const name of contract.realAcceptance.requiredMeasurements) {
    const minimum = minimums[name] || 1;
    if (!Number.isSafeInteger(receipt.measurements[name]) || receipt.measurements[name] < minimum) {
      fail(`CodeBuddy receipt measurement ${name} is invalid`);
    }
  }
  if (!sameCanonical(receipt.generations, context.generations)) {
    fail("CodeBuddy receipt generations do not match the selected bindings");
  }
  if (
    !sameCanonical(
      Object.keys(receipt.decisions || {}).sort(),
      [...contract.realAcceptance.requiredDecisions].sort(),
    ) ||
    !sameCanonical(receipt.decisions, expectedDecisions())
  ) {
    fail("CodeBuddy receipt recovery decisions are incomplete");
  }
  if (
    !sameCanonical(receipt.decoyRequests, expectedDecoys()) ||
    receipt.sensitiveScan?.status !== "pass" ||
    receipt.sensitiveScan?.matches !== 0 ||
    !Number.isSafeInteger(receipt.sensitiveScan?.scannedBytes) ||
    receipt.sensitiveScan.scannedBytes < 1 ||
    Object.keys(receipt.sensitiveScan || {}).length !== 3
  ) {
    fail("CodeBuddy receipt did not prove isolation and secret safety");
  }
  const scopeDigest = digest("cc-switch-server:codebuddy-real-scope:v2", {
    site,
    targetCommit: context.targetCommit,
    harnessRevision: HARNESS_REVISION,
    model,
    generations: context.generations,
    catalogs: context.catalogs,
    bodyHashes: receipt.bodyHashes,
  });
  if (receipt.scopeDigest !== scopeDigest || !/^[a-f0-9]{64}$/.test(receipt.scopeDigest)) {
    fail("CodeBuddy receipt scope digest does not match the selected bindings and bodies");
  }
}

async function main() {
  const targetCommit = gitCommit();
  const account = await validateAccount();
  const binding = await validateBindings(account);
  const catalogs = Object.fromEntries(
    await Promise.all(
      Object.entries(providerIds).map(async ([app, providerId]) => [
        app,
        await validateCatalog(app, providerId),
      ]),
    ),
  );
  const generations = expectedGenerations(account, binding);
  let receipt;
  try {
    receipt = JSON.parse(fs.readFileSync(receiptPath, "utf8"));
  } catch {
    fail("CodeBuddy receipt file is not valid JSON");
  }
  validateReceipt(receipt, { targetCommit, catalogs, generations });
  const verificationState = fixtureMode ? "contract_verified" : "live_verified";
  const liveState = fixtureMode ? "live_pending" : "live_verified";
  console.log(
    `[PASS] CodeBuddy ${site} private receipt accepted (verificationState=${verificationState}, liveState=${liveState})`,
  );
}

main().catch(() => {
  console.error("[FAIL] CodeBuddy acceptance failed (details redacted)");
  process.exitCode = 1;
});
