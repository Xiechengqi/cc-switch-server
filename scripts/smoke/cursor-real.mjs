#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

// cursor dual-rail evidence remains live_pending: OAuth and API-key evidence
// stays independent. Fixture mode proves only that this validator fails closed;
// it never promotes either rail.

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const contract = JSON.parse(
  fs.readFileSync(
    path.join(repoRoot, "assets/contract/cursor-reference-delta.json"),
    "utf8",
  ),
);
const HARNESS_REVISION = 2;
const RECEIPT_MAX_AGE_MS = 24 * 60 * 60 * 1_000;
const API_KEY_CATALOG_MAX_AGE_MS = 5 * 60 * 1_000;
const CATALOG_CLOCK_SKEW_MS = 60_000;
const MAX_CONTROL_RESPONSE_BYTES = 4 * 1024 * 1024;
let failureReported = false;

function reportFailure() {
  if (!failureReported) {
    failureReported = true;
    console.error("[FAIL] Cursor acceptance failed (details redacted)");
  }
  process.exitCode = 1;
}

process.on("uncaughtException", reportFailure);
process.on("unhandledRejection", reportFailure);

const railSpecs = Object.freeze({
  oauth: Object.freeze({
    providerType: "cursor_oauth",
    providerEnv: "CC_SWITCH_CURSOR_OAUTH_PROVIDER_ID",
    legacyProviderEnv: "CURSOR_OAUTH_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CURSOR_OAUTH_SHARE_ID",
    legacyShareEnv: "CURSOR_OAUTH_SHARE_ID",
    modelEnv: "CC_SWITCH_CURSOR_OAUTH_MODEL",
    receiptEnv: "CURSOR_OAUTH_REAL_RECEIPT_FILE",
    accountEnv: "CURSOR_OAUTH_TEST_ACCOUNT",
    authKind: "managed_account",
  }),
  api_key: Object.freeze({
    providerType: "cursor_apikey",
    providerEnv: "CC_SWITCH_CURSOR_API_KEY_PROVIDER_ID",
    legacyProviderEnv: "CURSOR_API_KEY_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CURSOR_API_KEY_SHARE_ID",
    legacyShareEnv: "CURSOR_API_KEY_SHARE_ID",
    modelEnv: "CC_SWITCH_CURSOR_API_KEY_MODEL",
    receiptEnv: "CURSOR_API_KEY_REAL_RECEIPT_FILE",
    accountEnv: null,
    authKind: "static_credential",
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

const rail = argValue("--rail", env("CC_SWITCH_CURSOR_REAL_RAIL"));
const spec = railSpecs[rail];
if (!spec) {
  console.log(
    JSON.stringify({
      verificationState: "blocked_inputs",
      liveState: "live_pending",
      missingInputs: ["--rail oauth|api_key"],
    }),
  );
  process.exit(0);
}

const fixtureMode = env("CC_SWITCH_CURSOR_HARNESS_MODE") === "fixture";
const serverUrl = env("SERVER_URL").replace(/\/+$/, "");
const shareUrl = env("CC_SWITCH_SHARE_URL").replace(/\/+$/, "");
const serverToken = env("CC_SWITCH_SERVER_TOKEN");
const routerToken = env("ROUTER_API_TOKEN");
const routerTokenHeader = env("ROUTER_API_TOKEN_HEADER", "Authorization");
const providerId = env(spec.providerEnv, env(spec.legacyProviderEnv));
const shareId = env(spec.shareEnv, env(spec.legacyShareEnv));
const accountSelector = spec.accountEnv ? env(spec.accountEnv) : "";
const app = env("CURSOR_REAL_PROVIDER_APP", "codex");
const model = env(spec.modelEnv, env("CURSOR_REAL_FAST_MODEL"));
const receiptFile = env(spec.receiptEnv, env("CURSOR_REAL_RECEIPT_FILE"));
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
  [spec.providerEnv, providerId],
  [spec.shareEnv, shareId],
  [spec.modelEnv, model],
  [spec.receiptEnv, receiptFile],
];
if (spec.accountEnv) requiredInputs.push([spec.accountEnv, accountSelector]);
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
if (!["claude", "codex", "gemini"].includes(app)) {
  fail("CURSOR_REAL_PROVIDER_APP is unsupported");
}
if (!/^[A-Za-z0-9][A-Za-z0-9._:/-]{0,255}-fast$/.test(model)) {
  fail(`${spec.modelEnv} must be one exact bounded *-fast model id`);
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
  fail("Cursor receipt file must be an absolute path");
}
const requestedReceiptPath = path.resolve(receiptFile);
const requestedRelative = path.relative(repoRoot, requestedReceiptPath);
if (!requestedRelative.startsWith("..") && !path.isAbsolute(requestedRelative)) {
  fail("Cursor receipt file must stay outside the repository");
}
if (
  !fs.existsSync(requestedReceiptPath) ||
  !fs.statSync(requestedReceiptPath).isFile()
) {
  fail("Cursor receipt file is unavailable");
}
const receiptPath = fs.realpathSync(requestedReceiptPath);
const receiptRelative = path.relative(repoRoot, receiptPath);
if (!receiptRelative.startsWith("..") && !path.isAbsolute(receiptRelative)) {
  fail("Cursor receipt file must stay outside the repository");
}
const receiptStat = fs.statSync(receiptPath);
if (receiptStat.nlink !== 1) {
  fail("Cursor receipt file must have one filesystem link");
}
if (receiptStat.size <= 0 || receiptStat.size > 1024 * 1024) {
  fail("Cursor receipt file size is invalid");
}
if (!fixtureMode && (receiptStat.mode & 0o777) !== 0o600) {
  fail("Cursor receipt file must have mode 0600");
}

function readReceiptSnapshot() {
  const flags = fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW ?? 0);
  const descriptor = fs.openSync(receiptPath, flags);
  try {
    const stat = fs.fstatSync(descriptor);
    if (
      !stat.isFile() ||
      stat.dev !== receiptStat.dev ||
      stat.ino !== receiptStat.ino ||
      stat.nlink !== 1 ||
      stat.size <= 0 ||
      stat.size > 1024 * 1024 ||
      (!fixtureMode && (stat.mode & 0o777) !== 0o600)
    ) {
      fail("Cursor receipt file changed before its snapshot");
    }
    const beforeRead = fs.fstatSync(descriptor, { bigint: true });
    const bytes = Buffer.alloc(stat.size);
    let offset = 0;
    while (offset < bytes.length) {
      const read = fs.readSync(descriptor, bytes, offset, bytes.length - offset, null);
      if (read === 0) fail("Cursor receipt file changed during its snapshot");
      offset += read;
    }
    if (fs.readSync(descriptor, Buffer.alloc(1), 0, 1, null) !== 0) {
      fail("Cursor receipt file changed during its snapshot");
    }
    const afterRead = fs.fstatSync(descriptor, { bigint: true });
    if (
      afterRead.dev !== beforeRead.dev ||
      afterRead.ino !== beforeRead.ino ||
      afterRead.nlink !== beforeRead.nlink ||
      afterRead.size !== beforeRead.size ||
      afterRead.mode !== beforeRead.mode ||
      afterRead.mtimeNs !== beforeRead.mtimeNs ||
      afterRead.ctimeNs !== beforeRead.ctimeNs
    ) {
      fail("Cursor receipt file changed during its snapshot");
    }
    return { contents: bytes.toString("utf8"), bytes, fingerprint: afterRead };
  } finally {
    fs.closeSync(descriptor);
  }
}

function sameReceiptFingerprint(current, fingerprint) {
  return current.isFile() &&
    current.dev === fingerprint.dev &&
    current.ino === fingerprint.ino &&
    current.nlink === fingerprint.nlink &&
    current.size === fingerprint.size &&
    current.mode === fingerprint.mode &&
    current.mtimeNs === fingerprint.mtimeNs &&
    current.ctimeNs === fingerprint.ctimeNs;
}

function assertReceiptStillCurrent(fingerprint, expectedBytes) {
  const currentPath = fs.realpathSync(requestedReceiptPath);
  if (currentPath !== receiptPath) {
    fail("Cursor receipt path changed during acceptance");
  }
  const flags = fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW ?? 0);
  const descriptor = fs.openSync(receiptPath, flags);
  try {
    const beforeRead = fs.fstatSync(descriptor, { bigint: true });
    if (!sameReceiptFingerprint(beforeRead, fingerprint)) {
      fail("Cursor receipt file changed during acceptance");
    }
    const bytes = Buffer.alloc(expectedBytes.length);
    let offset = 0;
    while (offset < bytes.length) {
      const read = fs.readSync(descriptor, bytes, offset, bytes.length - offset, null);
      if (read === 0) fail("Cursor receipt file changed during acceptance");
      offset += read;
    }
    if (fs.readSync(descriptor, Buffer.alloc(1), 0, 1, null) !== 0 || !bytes.equals(expectedBytes)) {
      fail("Cursor receipt file changed during acceptance");
    }
    const afterRead = fs.fstatSync(descriptor, { bigint: true });
    const current = fs.statSync(requestedReceiptPath, { bigint: true });
    if (
      !sameReceiptFingerprint(afterRead, fingerprint) ||
      !sameReceiptFingerprint(current, fingerprint) ||
      fs.realpathSync(requestedReceiptPath) !== receiptPath
    ) {
      fail("Cursor receipt file changed during acceptance");
    }
  } finally {
    fs.closeSync(descriptor);
  }
}

const secrets = [serverToken, routerToken].filter(usable);
function hasSecretLike(value) {
  const text = String(value || "");
  return (
    secrets.some((secret) => text.includes(secret)) ||
    /Bearer\s+[A-Za-z0-9._~+/-]{10,}/i.test(text) ||
    /\b(?:sk|key|jwt)-[A-Za-z0-9_-]{8,}\b/i.test(text) ||
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
    const declaredLength = Number(response.headers.get("content-length"));
    if (Number.isFinite(declaredLength) && declaredLength > MAX_CONTROL_RESPONSE_BYTES) {
      controller.abort();
      fail(`${label} exceeded the response-size bound`);
    }
    const chunks = [];
    let totalBytes = 0;
    if (response.body) {
      const reader = response.body.getReader();
      try {
        while (true) {
          const { done, value } = await reader.read();
          if (done) break;
          totalBytes += value.byteLength;
          if (totalBytes > MAX_CONTROL_RESPONSE_BYTES) {
            controller.abort();
            fail(`${label} exceeded the response-size bound`);
          }
          chunks.push(value);
        }
      } finally {
        reader.releaseLock();
      }
    }
    const bytes = Buffer.concat(chunks, totalBytes);
    const text = new TextDecoder().decode(bytes);
    if (hasSecretLike(text)) fail(`${label} contained secret-like material`);
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
  if (rail !== "oauth") return null;
  const response = await requestJson(
    serverUrl,
    "/api/accounts",
    adminHeaders(),
    "Cursor Account list",
  );
  if (response?.ok !== true || !Array.isArray(response.accounts)) {
    fail("Cursor Account list violated the control-plane contract");
  }
  const selector = accountSelector.toLowerCase();
  const account = exactOne(
    response.accounts.filter(
      (candidate) =>
        candidate?.providerType === "cursor_oauth" &&
        (candidate.id === accountSelector ||
          candidate.email?.trim().toLowerCase() === selector),
    ),
    "Cursor Account selector",
  );
  if (
    !Number.isSafeInteger(account.authIdentityGeneration) ||
    account.authIdentityGeneration < 1 ||
    !Number.isSafeInteger(account.tokenRefreshGeneration) ||
    account.tokenRefreshGeneration < 0 ||
    account.hasAccessToken !== true ||
    account.hasRefreshToken !== true ||
    account.hasApiKey !== false ||
    account.needsRelogin !== false
  ) {
    fail("Cursor Account is not a live refreshable OAuth identity generation");
  }
  return account;
}

async function validateBinding(account) {
  const providerList = await requestJson(
    serverUrl,
    "/api/providers",
    adminHeaders(),
    "Cursor Provider list",
  );
  if (providerList?.ok !== true || !Array.isArray(providerList.providers)) {
    fail("Cursor Provider list violated the control-plane contract");
  }
  const view = exactOne(
    providerList.providers.filter(
      (candidate) => candidate?.app === app && candidate?.provider?.id === providerId,
    ),
    "Cursor Provider binding",
  );
  const authRef = view.runtime?.authRef;
  if (
    view.providerType !== spec.providerType ||
    view.providerTypeId !== spec.providerType ||
    view.runtime?.driverId !== "special.cursor" ||
    view.runtime?.configurationState !== "ready" ||
    authRef?.kind !== spec.authKind
  ) {
    fail("Cursor Provider rail or runtime binding does not match the selected rail");
  }
  let generations;
  if (rail === "oauth") {
    if (
      !account ||
      authRef.accountId !== account.id ||
      authRef.expectedProviderType !== "cursor_oauth" ||
      authRef.authIdentityGeneration !== account.authIdentityGeneration
    ) {
      fail("Cursor OAuth Provider is not fixed to the selected Account generation");
    }
    generations = {
      authIdentityGeneration: account.authIdentityGeneration,
      tokenRefreshGeneration: account.tokenRefreshGeneration,
    };
  } else {
    if (
      authRef.authScheme !== "bearer" ||
      !Array.isArray(authRef.slots) ||
      !authRef.slots.includes("apiKey") ||
      !Number.isSafeInteger(authRef.credentialGeneration) ||
      authRef.credentialGeneration < 1
    ) {
      fail("Cursor API-key Provider is missing its exact credential generation");
    }
    generations = { credentialGeneration: authRef.credentialGeneration };
  }

  const shareList = await requestJson(
    serverUrl,
    "/api/shares",
    adminHeaders(),
    "Cursor Share list",
  );
  if (shareList?.ok !== true || !Array.isArray(shareList.shares)) {
    fail("Cursor Share list violated the control-plane contract");
  }
  const share = exactOne(
    shareList.shares.filter((candidate) => candidate?.id === shareId),
    "Cursor Share",
  );
  const bindings = Array.isArray(share.bindings) ? share.bindings : [];
  const selectedBindings = bindings.filter((binding) => binding?.app === app);
  const primaryBindings = bindings.filter((binding) => binding?.app === share.app);
  if (
    share.enabled !== true ||
    share.status !== "active" ||
    selectedBindings.length !== 1 ||
    selectedBindings[0].providerId !== providerId ||
    selectedBindings[0].providerType !== spec.providerType ||
    primaryBindings.length !== 1 ||
    primaryBindings[0].providerId !== share.providerId ||
    primaryBindings[0].providerType !== share.providerType
  ) {
    fail("Cursor Share is not fixed to the selected Provider rail");
  }
  const providerRevision = view.runtime.providerRevision;
  const runtimeFingerprint = view.runtime.runtimeFingerprint;
  const shareRevision = share.configRevision;
  if (
    !Number.isSafeInteger(providerRevision) ||
    providerRevision < 0 ||
    !Number.isSafeInteger(view.revision) ||
    view.revision !== providerRevision ||
    typeof runtimeFingerprint !== "string" ||
    !runtimeFingerprint.trim() ||
    !Number.isSafeInteger(shareRevision) ||
    shareRevision < 0
  ) {
    fail("Cursor Provider or Share revision scope is incomplete");
  }
  return {
    providerRevision,
    runtimeFingerprint,
    shareRevision,
    ...generations,
  };
}

function freshApiKeyCatalogTimestamp(fetchedAtMs, now = Date.now()) {
  return (
    Number.isSafeInteger(fetchedAtMs) &&
    fetchedAtMs >= now - API_KEY_CATALOG_MAX_AGE_MS &&
    fetchedAtMs <= now + CATALOG_CLOCK_SKEW_MS
  );
}

function freshReceiptTimestamp(recordedAtMs, now = Date.now()) {
  return Number.isFinite(recordedAtMs) &&
    recordedAtMs >= now - RECEIPT_MAX_AGE_MS &&
    recordedAtMs <= now + 5 * 60_000;
}

async function validateCatalog() {
  const query = new URLSearchParams({
    app,
    providerId,
    expectedShareId: shareId,
    expectedProviderId: providerId,
  });
  const catalog = await requestJson(
    shareUrl,
    `/v1/models?${query}`,
    shareHeaders(),
    "Cursor model catalog",
  );
  if (!Array.isArray(catalog?.data)) {
    fail("Cursor model catalog violated the public contract");
  }
  if (rail === "api_key") {
    const modelIds = catalog.data.map((entry) => entry?.id);
    if (
      modelIds.some((id) => typeof id !== "string" || !id || id.trim() !== id) ||
      new Set(modelIds).size !== modelIds.length
    ) {
      fail("Cursor API-key catalog has incomplete or duplicate model IDs");
    }
  }
  if (catalog.data.filter((entry) => entry?.id === model).length !== 1) {
    fail("Cursor model catalog did not preserve the exact fresh *-fast id");
  }
  if (rail === "api_key") {
    if (
      catalog.source !== "cursor_public_api" ||
      catalog.stale !== false ||
      !freshApiKeyCatalogTimestamp(catalog.fetchedAtMs)
    ) {
      fail("Cursor API-key catalog is not fresh bound-credential evidence");
    }
  }
  return rail === "api_key" ? {
    fetchedAtMs: catalog.fetchedAtMs,
    digest: digest(
      "cc-switch-server:cursor-api-key-catalog:v1",
      [...catalog.data].sort((left, right) =>
        left.id < right.id ? -1 : left.id > right.id ? 1 : 0),
    ),
  } : null;
}

function expectedDecisions() {
  return {
    sameIdentity401: "replayed_once",
    second401: "terminal",
    identityGenerationDrift: "terminal",
    crossRailFallback: "disabled",
    crossAccountFallback: "disabled",
    crossProviderFallback: "disabled",
    crossShareFallback: "disabled",
    postCommitReplay: "disabled",
    staleCatalogWireAuthorization: "disabled",
  };
}

function validateReceipt(receipt, scopeDigest, targetCommit, account, binding) {
  const fields = [
    "schemaVersion",
    "providerFamily",
    "rail",
    "verificationState",
    "liveState",
    "targetCommit",
    "harnessRevision",
    "recordedAt",
    "scopeDigest",
    "app",
    "model",
    "checks",
    "bodyHashes",
    "generations",
    "measurements",
    "decisions",
    "decoyRequests",
    "sensitiveScan",
  ];
  if (!receipt || typeof receipt !== "object" || Array.isArray(receipt)) {
    fail("Cursor receipt must be an object");
  }
  if (!sameCanonical(Object.keys(receipt).sort(), [...fields].sort())) {
    fail("Cursor receipt fields do not match schema version 2");
  }
  if (hasSecretLike(JSON.stringify(receipt))) {
    fail("Cursor receipt contained secret-like material");
  }
  const railContract = contract.realAcceptance.rails.find(
    (candidate) => candidate.rail === rail,
  );
  if (
    !railContract ||
    receipt.schemaVersion !== contract.realAcceptance.receiptSchemaVersion ||
    receipt.providerFamily !== "cursor" ||
    receipt.rail !== rail ||
    receipt.targetCommit !== targetCommit ||
    receipt.harnessRevision !== HARNESS_REVISION ||
    receipt.harnessRevision !== contract.realAcceptance.harnessRevision ||
    receipt.app !== app ||
    receipt.model !== model ||
    receipt.scopeDigest !== scopeDigest ||
    !/^[a-f0-9]{64}$/.test(receipt.scopeDigest)
  ) {
    fail("Cursor receipt identity or scope does not match the selected rail");
  }
  const expectedVerification = fixtureMode ? "contract_verified" : "live_verified";
  const expectedLive = fixtureMode ? "live_pending" : "live_verified";
  if (
    receipt.verificationState !== expectedVerification ||
    receipt.liveState !== expectedLive
  ) {
    fail("Cursor receipt evidence state is not valid for this harness mode");
  }
  const recordedAt = Date.parse(receipt.recordedAt);
  if (!freshReceiptTimestamp(recordedAt)) {
    fail("Cursor receipt timestamp is outside the acceptance window");
  }
  const checkKeys = Object.keys(receipt.checks || {}).sort();
  const requiredChecks = [...railContract.requiredChecks].sort();
  if (!sameCanonical(checkKeys, requiredChecks)) {
    fail("Cursor receipt check set is incomplete");
  }
  for (const check of requiredChecks) {
    if (receipt.checks[check] !== "pass") {
      fail(`Cursor receipt check ${check} did not pass`);
    }
  }
  const hashKeys = Object.keys(receipt.bodyHashes || {}).sort();
  const requiredHashes = [...railContract.requiredBodyHashes].sort();
  if (!sameCanonical(hashKeys, requiredHashes)) {
    fail("Cursor receipt body hash set is incomplete");
  }
  for (const name of requiredHashes) {
    if (!/^[a-f0-9]{64}$/.test(receipt.bodyHashes[name])) {
      fail(`Cursor receipt body hash ${name} is invalid`);
    }
  }
  const providerBindingDigest = digest("cc-switch-server:cursor-provider-binding:v2", {
    rail,
    app,
    providerId,
    runtimeFingerprint: binding.runtimeFingerprint,
    accountId: account?.id ?? null,
    authIdentityGeneration: binding.authIdentityGeneration ?? null,
    credentialGeneration: binding.credentialGeneration ?? null,
  });
  const expectedGenerations = {
    ...(rail === "oauth"
      ? {
          authIdentityGeneration: binding.authIdentityGeneration,
          tokenRefreshGeneration: binding.tokenRefreshGeneration,
        }
      : { credentialGeneration: binding.credentialGeneration }),
    providerRevision: binding.providerRevision,
    shareRevision: binding.shareRevision,
    providerBindingDigest,
  };
  if (!sameCanonical(receipt.generations, expectedGenerations)) {
    fail("Cursor receipt generations do not match the selected binding");
  }
  const measurementKeys = Object.keys(receipt.measurements || {}).sort();
  const requiredMeasurements = [...railContract.requiredMeasurements].sort();
  if (!sameCanonical(measurementKeys, requiredMeasurements)) {
    fail("Cursor receipt measurement set is incomplete");
  }
  const minimums = {
    surfaceRuns: 6,
    catalogRuns: 3,
    toolRuns: 1,
    imageRuns: 1,
    parkResumeRuns: 1,
  };
  for (const [name, minimum] of Object.entries(minimums)) {
    if (
      !Number.isSafeInteger(receipt.measurements[name]) ||
      receipt.measurements[name] < minimum
    ) {
      fail(`Cursor receipt measurement ${name} is below the frozen minimum`);
    }
  }
  if (!sameCanonical(receipt.decisions, expectedDecisions())) {
    fail("Cursor receipt recovery decisions are incomplete");
  }
  if (
    !sameCanonical(
      Object.keys(receipt.decisions).sort(),
      [...railContract.requiredDecisions].sort(),
    ) ||
    receipt.decoyRequests?.otherRail !== 0 ||
    receipt.decoyRequests?.otherAccount !== 0 ||
    receipt.decoyRequests?.otherProvider !== 0 ||
    receipt.decoyRequests?.otherShare !== 0 ||
    Object.keys(receipt.decoyRequests || {}).length !== 4 ||
    receipt.sensitiveScan?.status !== "pass" ||
    receipt.sensitiveScan?.matches !== 0 ||
    Object.keys(receipt.sensitiveScan || {}).length !== 2
  ) {
    fail("Cursor receipt did not prove fixed binding and secret safety");
  }
  return recordedAt;
}

async function main() {
  const { contents: receiptSnapshot, bytes: receiptBytes, fingerprint: receiptFingerprint } = readReceiptSnapshot();
  const targetCommit = gitCommit();
  const account = await validateAccount();
  const binding = await validateBinding(account);
  const catalogSnapshot = await validateCatalog();
  const scopeDigest = digest("cc-switch-server:cursor-real-scope:v2", {
    rail,
    targetCommit,
    harnessRevision: HARNESS_REVISION,
    app,
    providerId,
    providerRevision: binding.providerRevision,
    runtimeFingerprint: binding.runtimeFingerprint,
    shareId,
    shareRevision: binding.shareRevision,
    accountId: account?.id ?? null,
    authIdentityGeneration: binding.authIdentityGeneration ?? null,
    tokenRefreshGeneration: binding.tokenRefreshGeneration ?? null,
    credentialGeneration: binding.credentialGeneration ?? null,
    model,
  });
  let receipt;
  try {
    receipt = JSON.parse(receiptSnapshot);
  } catch {
    fail("Cursor receipt file is not valid JSON");
  }
  const recordedAt = validateReceipt(receipt, scopeDigest, targetCommit, account, binding);
  const finalAccount = await validateAccount();
  const finalBinding = await validateBinding(finalAccount);
  if (!sameCanonical(
    { accountId: finalAccount?.id ?? null, binding: finalBinding },
    { accountId: account?.id ?? null, binding },
  )) {
    fail("Cursor Account, Provider, Share, or credential scope drifted during acceptance");
  }
  // The API-key catalog is credential-bound and can change while the receipt and
  // final binding are checked. OAuth's static alias list is not entitlement evidence.
  const finalCatalogSnapshot = rail === "api_key" ? await validateCatalog() : null;
  const acceptanceNow = Date.now();
  if (rail === "api_key" && (
    !freshApiKeyCatalogTimestamp(catalogSnapshot.fetchedAtMs, acceptanceNow) ||
    !freshApiKeyCatalogTimestamp(finalCatalogSnapshot.fetchedAtMs, acceptanceNow)
  )) {
    fail("Cursor API-key catalog expired during acceptance");
  }
  if (rail === "api_key" && catalogSnapshot.digest !== finalCatalogSnapshot.digest) {
    fail("Cursor API-key catalog changed during acceptance");
  }
  if (!freshReceiptTimestamp(recordedAt, acceptanceNow)) {
    fail("Cursor receipt expired during acceptance");
  }
  assertReceiptStillCurrent(receiptFingerprint, receiptBytes);
  const verificationState = fixtureMode ? "contract_verified" : "live_verified";
  const liveState = fixtureMode ? "live_pending" : "live_verified";
  console.log(
    `[PASS] Cursor ${rail} private receipt accepted (verificationState=${verificationState}, liveState=${liveState})`,
  );
}

main().catch(reportFailure);
