#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

// cursor dual-rail evidence remains live_pending until each private rail receipt passes.

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const contract = JSON.parse(
  fs.readFileSync(path.join(repoRoot, "assets/contract/cursor-reference-delta.json"), "utf8"),
);
const railSpecs = Object.freeze({
  oauth: Object.freeze({
    providerType: "cursor_oauth",
    providerEnv: "CURSOR_OAUTH_PROVIDER_ID",
    shareEnv: "CURSOR_OAUTH_SHARE_ID",
    accountEnv: "CURSOR_OAUTH_TEST_ACCOUNT",
    authKind: "managed_account",
  }),
  api_key: Object.freeze({
    providerType: "cursor_api_key",
    providerEnv: "CURSOR_API_KEY_PROVIDER_ID",
    shareEnv: "CURSOR_API_KEY_SHARE_ID",
    accountEnv: null,
    authKind: "static_credential",
  }),
});

function env(name, fallback = "") {
  return String(process.env[name] || fallback).trim();
}

function argValue(name, fallback = "") {
  const index = process.argv.indexOf(name);
  return index >= 0 && index + 1 < process.argv.length ? process.argv[index + 1] : fallback;
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
const providerId = env(spec.providerEnv);
const shareId = env(spec.shareEnv);
const accountSelector = spec.accountEnv ? env(spec.accountEnv) : "";
const app = env("CURSOR_REAL_PROVIDER_APP", "codex");
const fastModel = env("CURSOR_REAL_FAST_MODEL");
const receiptFile = env("CURSOR_REAL_RECEIPT_FILE");
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
  ["CURSOR_REAL_FAST_MODEL", fastModel],
  ["CURSOR_REAL_RECEIPT_FILE", receiptFile],
];
if (spec.accountEnv) requiredInputs.push([spec.accountEnv, accountSelector]);
const missingInputs = requiredInputs.filter(([, value]) => !usable(value)).map(([name]) => name);
if (missingInputs.length > 0) {
  console.log(
    JSON.stringify({ rail, verificationState: "blocked_inputs", liveState: "live_pending", missingInputs }),
  );
  process.exit(0);
}
if (!["claude", "codex", "gemini"].includes(app)) fail("CURSOR_REAL_PROVIDER_APP is unsupported");
if (!/^[A-Za-z0-9][A-Za-z0-9._:/-]{0,255}-fast$/.test(fastModel)) {
  fail("CURSOR_REAL_FAST_MODEL must be one exact bounded *-fast model id");
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
if (!path.isAbsolute(receiptFile)) fail("CURSOR_REAL_RECEIPT_FILE must be an absolute path");
const receiptPath = path.resolve(receiptFile);
const receiptRelative = path.relative(repoRoot, receiptPath);
if (!receiptRelative.startsWith("..") && !path.isAbsolute(receiptRelative)) {
  fail("CURSOR_REAL_RECEIPT_FILE must stay outside the repository");
}
if (!fs.existsSync(receiptPath) || !fs.statSync(receiptPath).isFile()) {
  fail("CURSOR_REAL_RECEIPT_FILE is unavailable");
}
if (!fixtureMode && (fs.statSync(receiptPath).mode & 0o077) !== 0) {
  fail("CURSOR_REAL_RECEIPT_FILE must not be group/world accessible");
}

const secrets = [serverToken, routerToken].filter(usable);
function hasSecretLike(value) {
  const text = String(value || "");
  return (
    secrets.some((secret) => text.includes(secret)) ||
    /Bearer\s+[A-Za-z0-9._~+/-]{10,}/i.test(text) ||
    /\b(?:sk|key|jwt)-[A-Za-z0-9_-]{8,}\b/i.test(text) ||
    /\beyJ[A-Za-z0-9_-]{12,}\.[A-Za-z0-9_-]{12,}\.[A-Za-z0-9_-]{8,}\b/.test(text)
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
    const response = await fetch(`${base}${requestPath}`, { method: "GET", headers, signal: controller.signal });
    const bytes = new Uint8Array(await response.arrayBuffer());
    if (bytes.byteLength > 4 * 1024 * 1024) fail(`${label} exceeded the response-size bound`);
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

async function validateControlPlane() {
  const providerList = await requestJson(serverUrl, "/api/providers", adminHeaders(), "Cursor Provider list");
  if (providerList?.ok !== true || !Array.isArray(providerList.providers)) {
    fail("Cursor Provider list violated the control-plane contract");
  }
  const view = exactOne(
    providerList.providers.filter(
      (candidate) => candidate?.app === app && candidate?.provider?.id === providerId,
    ),
    "Cursor Provider binding",
  );
  if (
    view.providerType !== spec.providerType ||
    view.providerTypeId !== spec.providerType ||
    view.runtime?.driverId !== "special.cursor" ||
    view.runtime?.configurationState !== "ready" ||
    view.runtime?.authRef?.kind !== spec.authKind
  ) {
    fail("Cursor Provider rail or runtime binding does not match the selected rail");
  }
  const authRef = view.runtime.authRef;
  let identityGeneration;
  if (rail === "oauth") {
    if (
      authRef.accountId !== accountSelector ||
      authRef.expectedProviderType !== "cursor_oauth" ||
      !Number.isSafeInteger(authRef.authIdentityGeneration) ||
      authRef.authIdentityGeneration < 1
    ) {
      fail("Cursor OAuth Provider is not fixed to the selected Account generation");
    }
    identityGeneration = authRef.authIdentityGeneration;
  } else {
    if (!Number.isSafeInteger(authRef.credentialGeneration) || authRef.credentialGeneration < 1) {
      fail("Cursor API-key Provider is missing its credential generation");
    }
    identityGeneration = authRef.credentialGeneration;
  }

  const shareList = await requestJson(serverUrl, "/api/shares", adminHeaders(), "Cursor Share list");
  if (shareList?.ok !== true || !Array.isArray(shareList.shares)) {
    fail("Cursor Share list violated the control-plane contract");
  }
  const share = exactOne(shareList.shares.filter((candidate) => candidate?.id === shareId), "Cursor Share");
  const bindings = [
    { app: share.app, providerId: share.providerId, providerType: share.providerType },
    ...(Array.isArray(share.bindings) ? share.bindings : []),
  ];
  if (
    !bindings.some(
      (binding) =>
        binding?.app === app &&
        binding?.providerId === providerId &&
        binding?.providerType === spec.providerType,
    )
  ) {
    fail("Cursor Share is not fixed to the selected Provider rail");
  }
  return {
    identityGeneration,
    runtimeFingerprint: String(view.runtime.runtimeFingerprint || ""),
    providerRevision: view.providerRevision ?? view.provider?.revision ?? 0,
    shareRevision: share.configRevision ?? 0,
  };
}

async function validateCatalog() {
  const query = new URLSearchParams({ app, providerId });
  const catalog = await requestJson(
    shareUrl,
    `/v1/models?${query}`,
    shareHeaders(),
    "Cursor model catalog",
  );
  if (!Array.isArray(catalog?.data)) fail("Cursor model catalog violated the public contract");
  const matches = catalog.data.filter((entry) => entry?.id === fastModel);
  if (matches.length !== 1) fail("Cursor model catalog did not preserve the exact fresh *-fast id");
}

function validateReceipt(receipt, expectedScopeDigest) {
  const allowed = new Set([
    "schemaVersion",
    "rail",
    "verificationState",
    "liveState",
    "scopeDigest",
    "checks",
    "recordedAt",
  ]);
  if (!receipt || typeof receipt !== "object" || Array.isArray(receipt)) fail("Cursor receipt must be an object");
  for (const key of Object.keys(receipt)) {
    if (!allowed.has(key)) fail(`Cursor receipt contains forbidden field ${key}`);
  }
  if (hasSecretLike(JSON.stringify(receipt))) fail("Cursor receipt contained secret-like material");
  if (receipt.schemaVersion !== contract.realAcceptance.receiptSchemaVersion) {
    fail("Cursor receipt schema version changed");
  }
  if (receipt.rail !== rail) fail("Cursor receipt rail does not match the selected rail");
  if (!/^[a-f0-9]{64}$/.test(receipt.scopeDigest) || receipt.scopeDigest !== expectedScopeDigest) {
    fail("Cursor receipt scope digest does not match the selected binding");
  }
  const expectedVerification = fixtureMode ? "contract_verified" : "live_verified";
  const expectedLive = fixtureMode ? "live_pending" : "live_verified";
  if (receipt.verificationState !== expectedVerification || receipt.liveState !== expectedLive) {
    fail("Cursor receipt evidence state is not valid for this harness mode");
  }
  if (!receipt.checks || typeof receipt.checks !== "object" || Array.isArray(receipt.checks)) {
    fail("Cursor receipt checks are missing");
  }
  const keys = Object.keys(receipt.checks).sort();
  const required = [...contract.realAcceptance.requiredChecks].sort();
  if (JSON.stringify(keys) !== JSON.stringify(required)) fail("Cursor receipt check set is incomplete");
  for (const check of required) {
    if (receipt.checks[check] !== "pass") fail(`Cursor receipt check ${check} did not pass`);
  }
}

async function main() {
  const scope = await validateControlPlane();
  await validateCatalog();
  const scopeDigest = digest("cc-switch-server:cursor-real-scope:v1", {
    rail,
    app,
    providerId,
    shareId,
    accountSelector: rail === "oauth" ? accountSelector : null,
    fastModel,
    ...scope,
  });
  let receipt;
  try {
    receipt = JSON.parse(fs.readFileSync(receiptPath, "utf8"));
  } catch {
    fail("CURSOR_REAL_RECEIPT_FILE is not valid JSON");
  }
  validateReceipt(receipt, scopeDigest);
  const verificationState = fixtureMode ? "contract_verified" : "live_verified";
  const liveState = fixtureMode ? "live_pending" : "live_verified";
  console.log(
    `[PASS] Cursor ${rail} private receipt accepted (verificationState=${verificationState}, liveState=${liveState})`,
  );
}

main().catch(() => {
  console.error("[FAIL] Cursor acceptance failed (details redacted)");
  process.exitCode = 1;
});
