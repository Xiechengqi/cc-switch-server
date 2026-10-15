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

const acceptance = contract.realAcceptanceExtensions?.find(
  (extension) => extension.id === "AG-LIVE-V2-2026-10-08",
);
if (!acceptance || acceptance.receiptSchemaVersion !== 2 || acceptance.harnessRevision !== 2) {
  throw new Error("Antigravity current receipt contract is unavailable");
}
const HARNESS_REVISION = acceptance.harnessRevision;
const CATALOG_MAX_AGE_MS = acceptance.catalogMaxAgeMs;
const RECEIPT_MAX_AGE_MS = acceptance.receiptMaxAgeMs;
const CLOCK_SKEW_MS = acceptance.clockSkewMs;
const MAX_CONTROL_RESPONSE_BYTES = 4 * 1024 * 1024;
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
const requestedReceiptPath = path.resolve(receiptFile);
const receiptRelative = path.relative(repoRoot, requestedReceiptPath);
if (!receiptRelative.startsWith("..") && !path.isAbsolute(receiptRelative)) {
  fail("ANTIGRAVITY_REAL_RECEIPT_FILE must stay outside the repository");
}
if (!fs.existsSync(requestedReceiptPath) || !fs.statSync(requestedReceiptPath).isFile()) {
  fail("ANTIGRAVITY_REAL_RECEIPT_FILE is unavailable");
}
let receiptPath;
try {
  receiptPath = fs.realpathSync(requestedReceiptPath);
} catch {
  fail("ANTIGRAVITY_REAL_RECEIPT_FILE is unavailable");
}
const resolvedReceiptRelative = path.relative(repoRoot, receiptPath);
if (!resolvedReceiptRelative.startsWith("..") && !path.isAbsolute(resolvedReceiptRelative)) {
  fail("ANTIGRAVITY_REAL_RECEIPT_FILE must stay outside the repository");
}
const receiptStat = fs.statSync(receiptPath);
if (!fixtureMode && (receiptStat.mode & 0o077) !== 0) {
  fail("ANTIGRAVITY_REAL_RECEIPT_FILE must not be group/world accessible");
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
      (!fixtureMode && (stat.mode & 0o077) !== 0)
    ) {
      fail("ANTIGRAVITY_REAL_RECEIPT_FILE changed before its snapshot");
    }
    const beforeRead = fs.fstatSync(descriptor, { bigint: true });
    const bytes = Buffer.alloc(stat.size);
    let offset = 0;
    while (offset < bytes.length) {
      const read = fs.readSync(descriptor, bytes, offset, bytes.length - offset, null);
      if (read === 0) fail("ANTIGRAVITY_REAL_RECEIPT_FILE changed during its snapshot");
      offset += read;
    }
    if (fs.readSync(descriptor, Buffer.alloc(1), 0, 1, null) !== 0) {
      fail("ANTIGRAVITY_REAL_RECEIPT_FILE changed during its snapshot");
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
      fail("ANTIGRAVITY_REAL_RECEIPT_FILE changed during its snapshot");
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
    fail("ANTIGRAVITY_REAL_RECEIPT_FILE path changed during acceptance");
  }
  const flags = fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW ?? 0);
  const descriptor = fs.openSync(receiptPath, flags);
  try {
    const beforeRead = fs.fstatSync(descriptor, { bigint: true });
    if (!sameReceiptFingerprint(beforeRead, fingerprint)) {
      fail("ANTIGRAVITY_REAL_RECEIPT_FILE changed during acceptance");
    }
    const bytes = Buffer.alloc(expectedBytes.length);
    let offset = 0;
    while (offset < bytes.length) {
      const read = fs.readSync(descriptor, bytes, offset, bytes.length - offset, null);
      if (read === 0) fail("ANTIGRAVITY_REAL_RECEIPT_FILE changed during acceptance");
      offset += read;
    }
    if (fs.readSync(descriptor, Buffer.alloc(1), 0, 1, null) !== 0 || !bytes.equals(expectedBytes)) {
      fail("ANTIGRAVITY_REAL_RECEIPT_FILE changed during acceptance");
    }
    const afterRead = fs.fstatSync(descriptor, { bigint: true });
    const current = fs.statSync(requestedReceiptPath, { bigint: true });
    if (
      !sameReceiptFingerprint(afterRead, fingerprint) ||
      !sameReceiptFingerprint(current, fingerprint) ||
      fs.realpathSync(requestedReceiptPath) !== receiptPath
    ) {
      fail("ANTIGRAVITY_REAL_RECEIPT_FILE changed during acceptance");
    }
  } finally {
    fs.closeSync(descriptor);
  }
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
    account.hasRefreshToken !== true ||
    account.hasApiKey !== false ||
    account.needsRelogin !== false
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
    const providerRevision = view.runtime.providerRevision;
    const runtimeFingerprint = view.runtime.runtimeFingerprint;
    if (
      !Number.isSafeInteger(providerRevision) ||
      providerRevision < 0 ||
      !Number.isSafeInteger(view.revision) ||
      view.revision !== providerRevision ||
      typeof runtimeFingerprint !== "string" ||
      !runtimeFingerprint.trim()
    ) {
      fail(`${app} Antigravity Provider runtime scope is incomplete`);
    }
    bindings.push({
      app,
      providerId,
      providerRevision,
      runtimeFingerprint,
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
  if (share.enabled !== true || share.status !== "active") {
    fail("selected Antigravity Share is not active");
  }
  const shareBindings = Array.isArray(share.bindings) ? share.bindings : [];
  const primaryBindings = shareBindings.filter((binding) => binding?.app === share.app);
  if (
    primaryBindings.length !== 1 ||
    primaryBindings[0]?.providerId !== share.providerId ||
    primaryBindings[0]?.providerType !== share.providerType
  ) {
    fail("Antigravity Share primary fields do not match its one App binding");
  }
  for (const [app, providerId] of Object.entries(providerIds)) {
    const appBindings = shareBindings.filter((binding) => binding?.app === app);
    if (
      appBindings.length !== 1 ||
      appBindings[0]?.providerId !== providerId ||
      appBindings[0]?.providerType !== spec.providerType
    ) {
      fail(`Antigravity Share does not have one exact ${app} Provider rail binding`);
    }
  }
  if (!Number.isSafeInteger(share.configRevision) || share.configRevision < 0) {
    fail("Antigravity Share revision scope is incomplete");
  }
  return { bindings, shareRevision: share.configRevision };
}

function freshCatalogTimestamp(fetchedAtMs, now = Date.now()) {
  return Number.isSafeInteger(fetchedAtMs) &&
    fetchedAtMs >= now - CATALOG_MAX_AGE_MS &&
    fetchedAtMs <= now + CLOCK_SKEW_MS;
}

function freshReceiptTimestamp(recordedAtMs, now = Date.now()) {
  return Number.isFinite(recordedAtMs) &&
    recordedAtMs >= now - RECEIPT_MAX_AGE_MS &&
    recordedAtMs <= now + CLOCK_SKEW_MS;
}

async function validateCatalogs() {
  const snapshots = [];
  for (const [app, providerId] of Object.entries(providerIds)) {
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
      `${app} Antigravity model catalog`,
    );
    if (
      !Array.isArray(catalog?.data) ||
      catalog.source !== "authenticated_fetch_available_models" ||
      catalog.stale !== false ||
      !freshCatalogTimestamp(catalog.fetchedAtMs)
    ) {
      fail(`${app} Antigravity model catalog is not fresh bound-account evidence`);
    }
    const modelIds = catalog.data.map((entry) => entry?.id);
    if (
      modelIds.some((id) => typeof id !== "string" || !id || id.trim() !== id) ||
      new Set(modelIds).size !== modelIds.length
    ) {
      fail(`${app} Antigravity model catalog has incomplete or duplicate model IDs`);
    }
    const matches = catalog.data.filter((entry) => entry?.id === models[app]);
    if (matches.length !== 1) {
      fail(`${app} Antigravity model catalog does not contain the exact selected model`);
    }
    snapshots.push({
      app,
      model: models[app],
      source: catalog.source,
      stale: catalog.stale,
      fetchedAtMs: catalog.fetchedAtMs,
      digest: digest(
        acceptance.catalogDigestDomainPattern.replace("{app}", app),
        [...catalog.data].sort((left, right) => left.id < right.id ? -1 : left.id > right.id ? 1 : 0),
      ),
    });
  }
  return snapshots;
}

function validateReceipt(receipt, expectedScopeDigest, targetCommit, account, bindingState, catalogs) {
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
    "catalogs",
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
    receipt.schemaVersion !== acceptance.receiptSchemaVersion ||
    receipt.providerFamily !== "antigravity" ||
    receipt.rail !== rail ||
    receipt.targetCommit !== targetCommit ||
    receipt.harnessRevision !== HARNESS_REVISION ||
    receipt.scopeDigest !== expectedScopeDigest ||
    !/^[a-f0-9]{64}$/.test(receipt.scopeDigest)
  ) {
    fail("Antigravity receipt identity does not match the selected target scope");
  }
  const recordedAt = Date.parse(receipt.recordedAt);
  if (!freshReceiptTimestamp(recordedAt)) {
    fail("Antigravity receipt recordedAt is outside the acceptance window");
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
  if (!sameCanonical(receipt.catalogs, catalogs)) {
    fail("Antigravity receipt catalog snapshots do not match the fresh bound models");
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
  return recordedAt;
}

async function main() {
  if (receiptStat.nlink !== 1) {
    fail("ANTIGRAVITY_REAL_RECEIPT_FILE must have one filesystem link");
  }
  const { contents: receiptSnapshot, bytes: receiptBytes, fingerprint: receiptFingerprint } = readReceiptSnapshot();
  const targetCommit = gitCommit();
  const account = await validateAccount();
  const bindingState = await validateBindings(account);
  const catalogs = await validateCatalogs();
  const scopeDigest = digest(acceptance.scopeDigestDomain, {
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
    receipt = JSON.parse(receiptSnapshot);
  } catch {
    fail("ANTIGRAVITY_REAL_RECEIPT_FILE is not valid JSON");
  }
  const recordedAt = validateReceipt(receipt, scopeDigest, targetCommit, account, bindingState, catalogs);
  const finalAccount = await validateAccount();
  const finalBinding = await validateBindings(finalAccount);
  if (!sameCanonical(
    {
      accountId: finalAccount.id,
      authIdentityGeneration: finalAccount.authIdentityGeneration,
      tokenRefreshGeneration: finalAccount.tokenRefreshGeneration,
      binding: finalBinding,
    },
    {
      accountId: account.id,
      authIdentityGeneration: account.authIdentityGeneration,
      tokenRefreshGeneration: account.tokenRefreshGeneration,
      binding: bindingState,
    },
  )) {
    fail("Antigravity Account, Provider, or Share scope drifted during acceptance");
  }
  const finalCatalogs = await validateCatalogs();
  for (let index = 0; index < catalogs.length; index += 1) {
    if (
      finalCatalogs[index]?.app !== catalogs[index].app ||
      finalCatalogs[index].digest !== catalogs[index].digest
    ) {
      fail(`${catalogs[index].app} Antigravity model catalog changed during acceptance`);
    }
  }
  const acceptanceNow = Date.now();
  if (
    [...catalogs, ...finalCatalogs]
      .some((catalog) => !freshCatalogTimestamp(catalog.fetchedAtMs, acceptanceNow))
  ) {
    fail("Antigravity model catalogs expired during acceptance");
  }
  if (!freshReceiptTimestamp(recordedAt, acceptanceNow)) {
    fail("Antigravity receipt expired during acceptance");
  }
  assertReceiptStillCurrent(receiptFingerprint, receiptBytes);
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
