#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const oracle = JSON.parse(
  fs.readFileSync(path.join(repoRoot, "assets/contract/qoder-cli-oracle.json"), "utf8"),
);
const HARNESS_REVISION = 2;
const CATALOG_MAX_AGE_MS = 2 * 60 * 60 * 1_000;

const railSpecs = Object.freeze({
  global_oauth: Object.freeze({
    site: "global",
    accountEnv: "QODER_GLOBAL_OAUTH_TEST_ACCOUNT",
    modelEnv: "CC_SWITCH_QODER_GLOBAL_OAUTH_MODEL",
    shareEnv: "CC_SWITCH_QODER_GLOBAL_OAUTH_SHARE_ID",
    providerEnvs: Object.freeze({
      claude: "CC_SWITCH_QODER_GLOBAL_OAUTH_CLAUDE_PROVIDER_ID",
      codex: "CC_SWITCH_QODER_GLOBAL_OAUTH_CODEX_PROVIDER_ID",
      gemini: "CC_SWITCH_QODER_GLOBAL_OAUTH_GEMINI_PROVIDER_ID",
    }),
  }),
  global_pat: Object.freeze({
    site: "global",
    accountEnv: "QODER_GLOBAL_PAT_TEST_ACCOUNT",
    modelEnv: "CC_SWITCH_QODER_GLOBAL_PAT_MODEL",
    shareEnv: "CC_SWITCH_QODER_GLOBAL_PAT_SHARE_ID",
    providerEnvs: Object.freeze({
      claude: "CC_SWITCH_QODER_GLOBAL_PAT_CLAUDE_PROVIDER_ID",
      codex: "CC_SWITCH_QODER_GLOBAL_PAT_CODEX_PROVIDER_ID",
      gemini: "CC_SWITCH_QODER_GLOBAL_PAT_GEMINI_PROVIDER_ID",
    }),
  }),
  cn_oauth: Object.freeze({
    site: "cn",
    accountEnv: "QODER_CN_OAUTH_TEST_ACCOUNT",
    modelEnv: "CC_SWITCH_QODER_CN_OAUTH_MODEL",
    shareEnv: "CC_SWITCH_QODER_CN_OAUTH_SHARE_ID",
    providerEnvs: Object.freeze({
      claude: "CC_SWITCH_QODER_CN_OAUTH_CLAUDE_PROVIDER_ID",
      codex: "CC_SWITCH_QODER_CN_OAUTH_CODEX_PROVIDER_ID",
      gemini: "CC_SWITCH_QODER_CN_OAUTH_GEMINI_PROVIDER_ID",
    }),
  }),
});

function argValue(name, fallback = "") {
  const index = process.argv.indexOf(name);
  return index >= 0 && index + 1 < process.argv.length ? process.argv[index + 1] : fallback;
}

function env(name, fallback = "") {
  return String(process.env[name] || fallback).trim();
}

function usable(value) {
  const text = String(value || "").trim();
  return Boolean(text) && !text.includes("<") && !text.includes(">");
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

function fail(message) {
  throw new Error(message);
}

const rail = argValue("--rail", env("CC_SWITCH_QODER_REAL_RAIL"));
const spec = railSpecs[rail];
if (!spec) {
  console.log(
    JSON.stringify({
      verificationState: "blocked_inputs",
      liveState: "live_pending",
      missingInputs: ["--rail global_oauth|global_pat|cn_oauth"],
    }),
  );
  process.exit(0);
}

const oracleRail = oracle.rails.find((candidate) => candidate.id === rail);
if (!oracleRail || oracleRail.site !== spec.site) fail(`frozen oracle is missing rail ${rail}`);

const fixtureMode = env("CC_SWITCH_QODER_HARNESS_MODE") === "fixture";
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
const requestedModel = env(spec.modelEnv);
const receiptFile = env("QODER_REAL_RECEIPT_FILE");
const configuredTimeoutMs = Number(env("CC_SWITCH_REAL_TIMEOUT_MS", "120000"));
const timeoutMs = Number.isFinite(configuredTimeoutMs)
  ? Math.max(1_000, Math.min(300_000, Math.trunc(configuredTimeoutMs)))
  : 120_000;

const requiredInputs = [
  ["RUN_REAL=1", process.env.RUN_REAL === "1" ? "1" : ""],
  ["SERVER_URL", serverUrl],
  ["CC_SWITCH_SERVER_TOKEN", serverToken],
  ["CC_SWITCH_SHARE_URL", shareUrl],
  ["ROUTER_API_TOKEN", routerToken],
  [spec.accountEnv, accountSelector],
  [spec.shareEnv, shareId],
  ...Object.entries(spec.providerEnvs).map(([app, name]) => [name, providerIds[app]]),
  ["QODER_REAL_RECEIPT_FILE", receiptFile],
];
const missingInputs = requiredInputs
  .filter(([, value]) => !usable(value))
  .map(([name]) => name);
if (missingInputs.length > 0) {
  console.log(
    JSON.stringify({
      rail,
      site: spec.site,
      verificationState: "blocked_inputs",
      liveState: "live_pending",
      missingInputs,
    }),
  );
  process.exit(0);
}

function assertSafeOrigin(value, label, { share = false } = {}) {
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

assertSafeOrigin(serverUrl, "SERVER_URL");
assertSafeOrigin(shareUrl, "CC_SWITCH_SHARE_URL", { share: true });
if (!path.isAbsolute(receiptFile)) fail("QODER_REAL_RECEIPT_FILE must be an absolute path");
const receiptRelative = path.relative(repoRoot, path.resolve(receiptFile));
if (!receiptRelative.startsWith("..") && !path.isAbsolute(receiptRelative)) {
  fail("QODER_REAL_RECEIPT_FILE must stay outside the repository");
}

if (!/^(authorization|x-api-key|x-goog-api-key)$/i.test(routerTokenHeader)) {
  fail("ROUTER_API_TOKEN_HEADER is unsupported");
}

function assertReceiptDirectory() {
  const directory = path.dirname(receiptFile);
  let realDirectory;
  try {
    realDirectory = fs.realpathSync(directory);
  } catch {
    fail("QODER_REAL_RECEIPT_FILE parent directory does not exist");
  }
  const relative = path.relative(repoRoot, realDirectory);
  if (!relative.startsWith("..") && !path.isAbsolute(relative)) {
    fail("QODER_REAL_RECEIPT_FILE parent must stay outside the repository");
  }
  return directory;
}

const secrets = [serverToken, routerToken].filter(usable);
function redact(text) {
  let output = String(text || "");
  for (const secret of secrets) output = output.split(secret).join("[REDACTED]");
  return output
    .replace(/Bearer\s+[^\s,"'}]+/gi, "Bearer [REDACTED]")
    .replace(/\bpt-[A-Za-z0-9_-]{6,}\b/g, "[REDACTED_QODER_PAT]")
    .replace(/\beyJ[A-Za-z0-9_-]{12,}\.[A-Za-z0-9_-]{12,}\.[A-Za-z0-9_-]{8,}\b/g, "[REDACTED_JWT]");
}

let sensitiveMatches = 0;
let sensitiveBytes = 0;
function scanSensitive(text, label) {
  const value = String(text || "");
  sensitiveBytes += Buffer.byteLength(value);
  const patterns = [
    /Bearer\s+[A-Za-z0-9._~+/-]{10,}/i,
    /\bpt-[A-Za-z0-9_-]{8,}\b/,
    /\beyJ[A-Za-z0-9_-]{12,}\.[A-Za-z0-9_-]{12,}\.[A-Za-z0-9_-]{8,}\b/,
    /"(?:access|refresh|id)_token"\s*:\s*"(?!\[REDACTED\])[^"<][^"]+"/i,
  ];
  for (const secret of secrets) {
    if (secret && value.includes(secret)) sensitiveMatches += 1;
  }
  for (const pattern of patterns) {
    if (pattern.test(value)) sensitiveMatches += 1;
  }
  if (sensitiveMatches > 0) fail(`${label} contained secret-like material`);
}

function adminHeaders(extra = {}) {
  return new Headers({
    accept: "application/json",
    authorization: `Bearer ${serverToken}`,
    ...extra,
  });
}

function shareHeaders(extra = {}) {
  const headers = new Headers({ accept: "application/json", ...extra });
  if (/^authorization$/i.test(routerTokenHeader)) {
    headers.set("authorization", `Bearer ${routerToken}`);
  } else {
    headers.set(routerTokenHeader, routerToken);
  }
  return headers;
}

async function request(base, requestPath, init = {}, { admin = false } = {}) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  const headers = admin ? adminHeaders() : shareHeaders();
  for (const [name, value] of new Headers(init.headers || {})) headers.set(name, value);
  if (init.body !== undefined) headers.set("content-type", "application/json");
  try {
    const response = await fetch(`${base}${requestPath}`, {
      ...init,
      headers,
      signal: controller.signal,
    });
    return { response, stop: () => clearTimeout(timer) };
  } catch (error) {
    clearTimeout(timer);
    fail(`${admin ? "control-plane" : "Share"} request failed (${error?.name || "network"})`);
  }
}

async function readLimited(response, label, limit = 8 * 1024 * 1024) {
  const declaredLength = Number(response.headers.get("content-length"));
  if (Number.isFinite(declaredLength) && declaredLength > limit) {
    await response.body?.cancel();
    fail(`${label} exceeded the response-size bound`);
  }
  if (!response.body) return "";
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let text = "";
  let bytes = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      bytes += value.byteLength;
      if (bytes > limit) {
        await reader.cancel();
        fail(`${label} exceeded the response-size bound`);
      }
      text += decoder.decode(value, { stream: true });
    }
  } finally {
    reader.releaseLock();
  }
  text += decoder.decode();
  scanSensitive(text, label);
  return text;
}

async function requireJson(base, requestPath, init, label, options = {}) {
  const { response, stop } = await request(base, requestPath, init, options);
  try {
    const text = await readLimited(response, label);
    if (!response.ok) fail(`${label} returned HTTP ${response.status}`);
    try {
      return JSON.parse(text);
    } catch {
      fail(`${label} returned invalid JSON`);
    }
  } finally {
    stop();
  }
}

function own(object, key) {
  return Object.prototype.hasOwnProperty.call(object, key);
}

function nonNegative(value) {
  return typeof value === "number" && Number.isFinite(value) && value >= 0;
}

function resolveAccount(accounts) {
  const selector = accountSelector.toLowerCase();
  const matches = accounts.filter(
    (account) =>
      account?.providerType === "qoder_cosy" &&
      (account.id === accountSelector || account.email?.trim().toLowerCase() === selector),
  );
  if (matches.length !== 1) fail(`${spec.accountEnv} did not select exactly one Qoder Account`);
  const account = matches[0];
  if (
    account.qoderCredentialScope?.site !== spec.site ||
    account.qoderCredentialScope?.credentialRail !==
      (rail === "global_pat" ? "pat_job_token" : rail)
  ) {
    fail("selected Qoder Account profile does not match the selected site and rail");
  }
  if (!account.id?.startsWith(`qoder-${spec.site}-`)) {
    fail(`selected Qoder Account does not belong to the ${spec.site} site`);
  }
  if (!Number.isSafeInteger(account.authIdentityGeneration) || account.authIdentityGeneration < 1) {
    fail("selected Qoder Account is missing authIdentityGeneration");
  }
  if (!Number.isSafeInteger(account.tokenRefreshGeneration) || account.tokenRefreshGeneration < 0) {
    fail("selected Qoder Account is missing tokenRefreshGeneration");
  }
  if (account.needsRelogin !== false) fail("selected Qoder Account requires relogin");
  if (rail === "global_pat") {
    if (
      account.hasApiKey !== true ||
      account.hasAccessToken !== false ||
      account.hasRefreshToken !== false
    ) {
      fail("Global PAT rail must persist only its PAT credential");
    }
  } else if (
    account.hasAccessToken !== true ||
    account.hasRefreshToken !== true ||
    account.hasApiKey !== false
  ) {
    fail("Qoder OAuth rail must persist access and refresh tokens without a PAT");
  }
  return account;
}

async function accountSnapshot() {
  const response = await requireJson(
    serverUrl,
    "/api/accounts",
    { method: "GET" },
    "Qoder Account list",
    { admin: true },
  );
  if (response?.ok !== true || !Array.isArray(response.accounts)) {
    fail("Qoder Account list violated the control-plane contract");
  }
  return resolveAccount(response.accounts);
}

async function validateBindings(account) {
  const response = await requireJson(
    serverUrl,
    "/api/providers",
    { method: "GET" },
    "Qoder Provider list",
    { admin: true },
  );
  if (response?.ok !== true || !Array.isArray(response.providers)) {
    fail("Qoder Provider list violated the control-plane contract");
  }
  const bindings = [];
  const surfaceBindings = {};
  for (const [app, providerId] of Object.entries(providerIds)) {
    const matches = response.providers.filter(
      (view) => view?.app === app && view?.provider?.id === providerId,
    );
    if (matches.length !== 1) fail(`${app} Qoder Provider binding is not unique`);
    const view = matches[0];
    const authRef = view.runtime?.authRef;
    if (
      view.providerType !== "qoder_cosy" ||
      view.providerTypeId !== "qoder_cosy" ||
      view.runtime?.driverId !== "special.qoder_cosy" ||
      view.runtime?.configurationState !== "ready" ||
      authRef?.kind !== "managed_account" ||
      authRef.expectedProviderType !== "qoder_cosy" ||
      authRef.accountId !== account.id ||
      authRef.authIdentityGeneration !== account.authIdentityGeneration
    ) {
      fail(`${app} Qoder Provider is not fixed to the selected Account generation`);
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
      fail(`${app} Qoder Provider runtime scope is incomplete`);
    }
    bindings.push({
      app,
      providerId,
      providerRevision,
      runtimeFingerprint,
      accountId: authRef.accountId,
      authIdentityGeneration: authRef.authIdentityGeneration,
    });
    surfaceBindings[app] = {
      providerRevision,
      runtimeFingerprintDigest: digest(
        `cc-switch-server:qoder-${app}-runtime:v1`,
        runtimeFingerprint,
      ),
    };
  }
  return {
    digest: digest("cc-switch-server:qoder-provider-bindings:v2", bindings),
    surfaceBindings,
  };
}

async function validateShareBinding() {
  const response = await requireJson(
    serverUrl,
    "/api/shares",
    { method: "GET" },
    "Qoder Share list",
    { admin: true },
  );
  if (response?.ok !== true || !Array.isArray(response.shares)) {
    fail("Qoder Share list violated the control-plane contract");
  }
  const matches = response.shares.filter((share) => share?.id === shareId);
  if (matches.length !== 1) fail(`${spec.shareEnv} did not select exactly one Share`);
  const share = matches[0];
  if (share.enabled !== true || share.status !== "active") {
    fail("selected Qoder Share is not active");
  }
  const bindings = Array.isArray(share.bindings) ? share.bindings : [];
  const primaryBindings = bindings.filter((binding) => binding?.app === share.app);
  if (
    primaryBindings.length !== 1 ||
    primaryBindings[0].providerId !== share.providerId ||
    primaryBindings[0].providerType !== share.providerType
  ) {
    fail("Qoder Share primary binding is ambiguous");
  }
  for (const [app, providerId] of Object.entries(providerIds)) {
    const selectedBindings = bindings.filter((binding) => binding?.app === app);
    if (
      selectedBindings.length !== 1 ||
      selectedBindings[0].providerId !== providerId ||
      selectedBindings[0].providerType !== "qoder_cosy"
    ) {
      fail(`Qoder Share is not fixed to the selected ${app} Provider`);
    }
  }
  const shareRevision = share.configRevision;
  if (!Number.isSafeInteger(shareRevision) || shareRevision < 0) {
    fail("Qoder Share revision scope is incomplete");
  }
  return {
    shareRevision,
    shareIdentityDigest: digest("cc-switch-server:qoder-share-identity:v1", shareId),
    shareBindingDigest: digest("cc-switch-server:qoder-share-binding:v2", {
      shareId,
      shareRevision,
      bindings: Object.entries(providerIds)
        .map(([app, providerId]) => ({ app, providerId, providerType: "qoder_cosy" }))
        .sort((left, right) => left.app.localeCompare(right.app)),
    }),
  };
}

function freshCatalogTimestamp(fetchedAtMs, now = Date.now()) {
  return (
    Number.isSafeInteger(fetchedAtMs) &&
    fetchedAtMs >= now - CATALOG_MAX_AGE_MS &&
    fetchedAtMs <= now + 5 * 60_000
  );
}

async function validateCatalogs(requiredModel = requestedModel) {
  const catalogs = [];
  for (const [app, providerId] of Object.entries(providerIds)) {
    const query = new URLSearchParams({
      app,
      providerId,
      expectedShareId: shareId,
      expectedProviderId: providerId,
    });
    const catalog = await requireJson(
      shareUrl,
      `/v1/models?${query}`,
      { method: "GET" },
      `${app} Qoder model catalog`,
    );
    if (
      !Array.isArray(catalog?.data) ||
      catalog.source !== "qoder_live_model_catalog" ||
      catalog.stale !== false ||
      !freshCatalogTimestamp(catalog.fetchedAtMs)
    ) {
      fail(`${app} Qoder model catalog is not a fresh bound-account catalog`);
    }
    const modelIds = catalog.data.map((entry) => entry?.id);
    if (
      modelIds.some((id) => typeof id !== "string" || !id || id.trim() !== id) ||
      new Set(modelIds).size !== modelIds.length
    ) {
      fail(`${app} Qoder model catalog has incomplete or duplicate model IDs`);
    }
    const models = new Set(modelIds);
    if (models.size === 0) fail(`${app} Qoder model catalog is empty`);
    catalogs.push({
      app,
      source: catalog.source,
      stale: false,
      fetchedAtMs: catalog.fetchedAtMs,
      digest: digest(
        `cc-switch-server:qoder-${app}-catalog:v1`,
        [...catalog.data].sort((left, right) => left.id < right.id ? -1 : left.id > right.id ? 1 : 0),
      ),
      models,
    });
  }
  const commonModels = [...catalogs[0].models].filter((model) =>
    catalogs.slice(1).every((catalog) => catalog.models.has(model)),
  );
  const model = requiredModel || commonModels.sort()[0];
  if (!model || !catalogs.every((catalog) => catalog.models.has(model))) {
    fail("Qoder Provider catalogs have no common selected model");
  }
  return {
    model,
    snapshots: Object.fromEntries(
      catalogs.map(({ app, models: _models, ...snapshot }) => [app, snapshot]),
    ),
  };
}

async function validateQuota(account) {
  const quota = await requireJson(
    serverUrl,
    `/api/accounts/${encodeURIComponent(account.id)}/quota?refresh=true&force=true`,
    { method: "GET" },
    "Qoder quota refresh",
    { admin: true },
  );
  if (
    quota?.ok !== true ||
    quota.account?.id !== account.id ||
    quota.account?.providerType !== "qoder_cosy" ||
    quota.quota?.success !== true
  ) {
    fail("Qoder quota refresh did not return the selected Account snapshot");
  }
  const state = quota.quota.extraUsage?.qoderQuota?.availability;
  if (!["available", "exhausted", "unknown"].includes(state)) {
    fail("Qoder quota refresh is missing an explicit availability state");
  }
  return state;
}

function nextBoundary(buffer) {
  const match = /\r\n\r\n|\n\n|\r\r/.exec(buffer);
  return match ? { index: match.index, length: match[0].length } : null;
}

function parseFrame(frame, label) {
  let event = "";
  const data = [];
  for (const line of frame.split(/\r\n|\r|\n/)) {
    if (!line || line.startsWith(":")) continue;
    if (line.startsWith("event:")) event = line.slice(6).trimStart();
    if (line.startsWith("data:")) data.push(line.slice(5).trimStart());
  }
  if (data.length === 0) return null;
  const joined = data.join("\n");
  if (joined === "[DONE]") return { event, done: true, payload: null };
  try {
    return { event, done: false, payload: JSON.parse(joined) };
  } catch {
    fail(`${label} emitted malformed SSE JSON`);
  }
}

async function collectSse(requestPath, body, label, extraHeaders = {}) {
  const { response, stop } = await request(shareUrl, requestPath, {
    method: "POST",
    headers: { accept: "text/event-stream", ...extraHeaders },
    body: JSON.stringify(body),
  });
  try {
    if (!response.ok) {
      await readLimited(response, label);
      fail(`${label} returned HTTP ${response.status}`);
    }
    if (!(response.headers.get("content-type") || "").toLowerCase().includes("text/event-stream")) {
      fail(`${label} returned a non-SSE content type`);
    }
    if (!response.body) fail(`${label} returned no stream body`);
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    let bytes = 0;
    const frames = [];
    while (true) {
      const { done, value } = await reader.read();
      bytes += value?.byteLength || 0;
      if (bytes > 8 * 1024 * 1024) fail(`${label} exceeded the stream-size bound`);
      buffer += decoder.decode(value || new Uint8Array(), { stream: !done });
      while (true) {
        const boundary = nextBoundary(buffer);
        if (!boundary) break;
        const frame = buffer.slice(0, boundary.index);
        buffer = buffer.slice(boundary.index + boundary.length);
        const parsed = parseFrame(frame, label);
        if (parsed) frames.push(parsed);
      }
      if (done) break;
    }
    if (buffer.trim()) {
      const parsed = parseFrame(buffer, label);
      if (parsed) frames.push(parsed);
    }
    scanSensitive(JSON.stringify(frames), label);
    if (frames.length === 0) fail(`${label} ended without SSE data`);
    const doneIndex = frames.findIndex((frame) => frame.done);
    if (
      doneIndex >= 0 &&
      (doneIndex !== frames.length - 1 || frames.filter((frame) => frame.done).length !== 1)
    ) {
      fail(`${label} emitted a premature DONE marker`);
    }
    if (
      frames.some(
        (frame) =>
          frame.event === "error" ||
          ["error", "response.failed", "response.incomplete"].includes(frame.payload?.type) ||
          frame.payload?.error != null,
      )
    ) {
      fail(`${label} stream reported an error`);
    }
    return frames;
  } finally {
    stop();
  }
}

function requireUsage(value, label) {
  if (!value || typeof value !== "object") fail(`${label} is missing usage`);
}

const toolName = "lookup";
const toolDescription = "Return one deterministic acceptance value for a supplied key.";
const toolSchema = {
  type: "object",
  additionalProperties: false,
  properties: { key: { type: "string" } },
  required: ["key"],
};
const smokeText = "Call lookup with key qoder-acceptance. Do not answer in prose.";

function validToolArguments(value) {
  let parsed = value;
  if (typeof parsed === "string") {
    try {
      parsed = JSON.parse(parsed);
    } catch {
      return false;
    }
  }
  return parsed !== null && typeof parsed === "object" && !Array.isArray(parsed) &&
    Object.keys(parsed).length === 1 && parsed.key === "qoder-acceptance";
}

function requireToolArguments(value, label) {
  if (!validToolArguments(value)) fail(`${label} tool arguments did not round-trip`);
}

async function validateClaude(model) {
  const base = {
    model,
    max_tokens: 64,
    messages: [{ role: "user", content: smokeText }],
    tools: [{ name: toolName, description: toolDescription, input_schema: toolSchema }],
    tool_choice: { type: "tool", name: toolName },
  };
  const nonstreamBody = { ...base, stream: false };
  const streamBody = { ...base, stream: true };
  const nonstream = await requireJson(
    shareUrl,
    "/v1/messages",
    {
      method: "POST",
      headers: { "anthropic-version": "2023-06-01" },
      body: JSON.stringify(nonstreamBody),
    },
    "Qoder Claude non-stream",
  );
  if (nonstream?.type !== "message" || nonstream.role !== "assistant") {
    fail("Qoder Claude non-stream response is not an assistant message");
  }
  const nonstreamTool = nonstream.content?.find(
    (block) => block?.type === "tool_use" && block.name === toolName,
  );
  if (!nonstreamTool) {
    fail("Qoder Claude non-stream response is missing the lookup tool call");
  }
  if (!usable(nonstreamTool.id)) fail("Qoder Claude non-stream tool id is missing");
  requireToolArguments(nonstreamTool.input, "Qoder Claude non-stream");
  requireUsage(nonstream.usage, "Qoder Claude non-stream response");
  const frames = await collectSse(
    "/v1/messages",
    streamBody,
    "Qoder Claude stream",
    { "anthropic-version": "2023-06-01" },
  );
  const terminals = frames.filter((frame) => frame.payload?.type === "message_stop");
  if (terminals.length !== 1) fail("Qoder Claude stream must emit one message_stop");
  const toolBlocks = new Map();
  for (const frame of frames) {
    const payload = frame.payload;
    if (payload?.type === "content_block_start" && payload.content_block?.type === "tool_use") {
      toolBlocks.set(payload.index, {
        id: payload.content_block.id,
        name: payload.content_block.name,
        input: payload.content_block.input,
        fragments: [],
        stopped: false,
      });
    } else if (payload?.type === "content_block_delta" && toolBlocks.has(payload.index)) {
      if (payload.delta?.type === "input_json_delta" && typeof payload.delta.partial_json === "string") {
        toolBlocks.get(payload.index).fragments.push(payload.delta.partial_json);
      }
    } else if (payload?.type === "content_block_stop" && toolBlocks.has(payload.index)) {
      toolBlocks.get(payload.index).stopped = true;
    }
  }
  const streamTool = [...toolBlocks.values()].find((block) => block.name === toolName);
  if (!streamTool) {
    fail("Qoder Claude stream is missing the lookup tool lifecycle");
  }
  if (!usable(streamTool.id) || !streamTool.stopped) {
    fail("Qoder Claude stream tool lifecycle is incomplete");
  }
  requireToolArguments(
    streamTool.fragments.length > 0 ? streamTool.fragments.join("") : streamTool.input,
    "Qoder Claude stream",
  );
  return {
    terminalCount: terminals.length,
    eof: true,
    bodyHashes: {
      nonstream: digest("cc-switch-server:qoder-claude-nonstream-body:v1", nonstreamBody),
      stream: digest("cc-switch-server:qoder-claude-stream-body:v1", streamBody),
    },
  };
}

async function validateCodex(model) {
  const base = {
    model,
    input: smokeText,
    max_output_tokens: 64,
    store: false,
    tools: [
      {
        type: "function",
        name: toolName,
        description: toolDescription,
        parameters: toolSchema,
      },
    ],
    tool_choice: { type: "function", name: toolName },
  };
  const nonstreamBody = { ...base, stream: false };
  const streamBody = { ...base, stream: true };
  const nonstream = await requireJson(
    shareUrl,
    "/v1/responses",
    { method: "POST", body: JSON.stringify(nonstreamBody) },
    "Qoder Codex non-stream",
  );
  if (nonstream?.object !== "response" || nonstream.status !== "completed") {
    fail("Qoder Codex non-stream response is not completed");
  }
  const nonstreamTool = nonstream.output?.find(
    (item) => item?.type === "function_call" && item.name === toolName,
  );
  if (!nonstreamTool) {
    fail("Qoder Codex non-stream response is missing the lookup tool call");
  }
  if (!usable(nonstreamTool.call_id)) fail("Qoder Codex non-stream tool id is missing");
  requireToolArguments(nonstreamTool.arguments, "Qoder Codex non-stream");
  requireUsage(nonstream.usage, "Qoder Codex non-stream response");
  const frames = await collectSse(
    "/v1/responses",
    streamBody,
    "Qoder Codex stream",
  );
  const terminals = frames.filter((frame) => frame.payload?.type === "response.completed");
  if (terminals.length !== 1) fail("Qoder Codex stream must emit one response.completed");
  const finalItems = terminals[0]?.payload?.response?.output;
  const completedItems = frames
    .filter((frame) => frame.payload?.type === "response.output_item.done")
    .map((frame) => frame.payload.item);
  const toolItems = Array.isArray(finalItems) && finalItems.length > 0
    ? finalItems
    : completedItems;
  const streamTool = toolItems.find(
    (item) => item?.type === "function_call" && item.name === toolName,
  );
  if (!streamTool) {
    fail("Qoder Codex stream is missing the lookup tool lifecycle");
  }
  if (!usable(streamTool.call_id)) fail("Qoder Codex stream tool id is missing");
  requireToolArguments(streamTool.arguments, "Qoder Codex stream");
  return {
    terminalCount: terminals.length,
    eof: true,
    bodyHashes: {
      nonstream: digest("cc-switch-server:qoder-codex-nonstream-body:v1", nonstreamBody),
      stream: digest("cc-switch-server:qoder-codex-stream-body:v1", streamBody),
    },
  };
}

function geminiBody() {
  return {
    contents: [{ role: "user", parts: [{ text: smokeText }] }],
    tools: [
      {
        functionDeclarations: [
          { name: toolName, description: toolDescription, parameters: toolSchema },
        ],
      },
    ],
    toolConfig: {
      functionCallingConfig: { mode: "ANY", allowedFunctionNames: [toolName] },
    },
    generationConfig: { maxOutputTokens: 64 },
  };
}

async function validateGemini(model) {
  const modelPath = encodeURIComponent(model);
  const body = geminiBody();
  const nonstream = await requireJson(
    shareUrl,
    `/v1beta/models/${modelPath}:generateContent`,
    { method: "POST", body: JSON.stringify(body) },
    "Qoder Gemini non-stream",
  );
  if (!Array.isArray(nonstream?.candidates) || nonstream.candidates.length === 0) {
    fail("Qoder Gemini non-stream response has no candidate");
  }
  const nonstreamTool = nonstream.candidates
    .flatMap((candidate) => candidate?.content?.parts || [])
    .find((part) => part?.functionCall?.name === toolName)?.functionCall;
  if (!nonstreamTool) {
    fail("Qoder Gemini non-stream response is missing the lookup tool call");
  }
  requireToolArguments(nonstreamTool.args, "Qoder Gemini non-stream");
  requireUsage(nonstream.usageMetadata, "Qoder Gemini non-stream response");
  const frames = await collectSse(
    `/v1beta/models/${modelPath}:streamGenerateContent?alt=sse`,
    body,
    "Qoder Gemini stream",
  );
  const terminals = frames.filter((frame) =>
    frame.payload?.candidates?.some((candidate) => usable(candidate?.finishReason)),
  );
  if (terminals.length !== 1) fail("Qoder Gemini stream must emit one finishReason terminal");
  const streamTool = frames
    .flatMap((frame) => frame.payload?.candidates || [])
    .flatMap((candidate) => candidate?.content?.parts || [])
    .find((part) => part?.functionCall?.name === toolName)?.functionCall;
  if (!streamTool) {
    fail("Qoder Gemini stream is missing the lookup tool lifecycle");
  }
  requireToolArguments(streamTool.args, "Qoder Gemini stream");
  return {
    terminalCount: terminals.length,
    eof: true,
    bodyHashes: {
      nonstream: digest("cc-switch-server:qoder-gemini-nonstream-body:v1", body),
      stream: digest("cc-switch-server:qoder-gemini-stream-body:v1", body),
    },
  };
}

function assertReceiptSafe(receipt) {
  const forbidden = new Set(oracle.receiptSchema.forbiddenFields);
  const visit = (value) => {
    if (Array.isArray(value)) return value.forEach(visit);
    if (!value || typeof value !== "object") return;
    for (const [key, nested] of Object.entries(value)) {
      if (forbidden.has(key)) fail(`receipt contains forbidden field ${key}`);
      if (/prompt|callback|raw(?:request|response|body)/i.test(key)) {
        fail(`receipt contains unsafe field ${key}`);
      }
      visit(nested);
    }
  };
  visit(receipt);
  const serialized = JSON.stringify(receipt);
  scanSensitive(serialized, "Qoder receipt");
  for (const field of oracle.receiptSchema.requiredFields) {
    if (!own(receipt, field)) fail(`receipt is missing required field ${field}`);
  }
  if (
    receipt.schemaVersion !== oracle.receiptSchema.schemaVersion ||
    receipt.harnessRevision !== oracle.receiptSchema.harnessRevision
  ) {
    fail("receipt schema or harness revision drifted");
  }
}

function assertFreshCatalogs(snapshots, now = Date.now()) {
  if (
    Object.values(snapshots).some(
      (snapshot) => !freshCatalogTimestamp(snapshot.fetchedAtMs, now),
    )
  ) {
    fail("Qoder model catalogs expired during acceptance");
  }
}

function writeReceipt(receipt, verifyBeforePublish) {
  assertReceiptSafe(receipt);
  const directory = assertReceiptDirectory();
  const contents = Buffer.from(`${JSON.stringify(receipt, null, 2)}\n`, "utf8");
  const flags = fs.constants.O_RDONLY |
    (fs.constants.O_DIRECTORY ?? 0) |
    (fs.constants.O_NOFOLLOW ?? 0);
  const directoryDescriptor = fs.openSync(fs.realpathSync(directory), flags);
  const anchoredDirectory = `/proc/self/fd/${directoryDescriptor}`;
  function assertReceiptContents(descriptor, fingerprint, linkCount, requireMode = true) {
    const before = fs.fstatSync(descriptor, { bigint: true });
    if (
      !before.isFile() ||
      before.nlink !== linkCount ||
      before.size !== BigInt(contents.length) ||
      (requireMode && (before.mode & 0o777n) !== 0o600n) ||
      (fingerprint && (
        before.dev !== fingerprint.dev ||
        before.ino !== fingerprint.ino ||
        before.mode !== fingerprint.mode ||
        before.mtimeNs !== fingerprint.mtimeNs ||
        before.ctimeNs !== fingerprint.ctimeNs
      ))
    ) {
      fail("Qoder receipt changed during publication");
    }
    const actual = Buffer.alloc(contents.length);
    let offset = 0;
    while (offset < actual.length) {
      const read = fs.readSync(descriptor, actual, offset, actual.length - offset, offset);
      if (read === 0) fail("Qoder receipt changed during publication");
      offset += read;
    }
    if (
      fs.readSync(descriptor, Buffer.alloc(1), 0, 1, actual.length) !== 0 ||
      !actual.equals(contents)
    ) {
      fail("Qoder receipt changed during publication");
    }
    const after = fs.fstatSync(descriptor, { bigint: true });
    if (
      after.dev !== before.dev ||
      after.ino !== before.ino ||
      after.nlink !== before.nlink ||
      after.size !== before.size ||
      after.mode !== before.mode ||
      after.mtimeNs !== before.mtimeNs ||
      after.ctimeNs !== before.ctimeNs
    ) {
      fail("Qoder receipt changed during publication");
    }
  }
  function assertReceiptPath(file, fingerprint, linkCount) {
    const descriptor = fs.openSync(file, fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW ?? 0));
    try {
      assertReceiptContents(descriptor, fingerprint, linkCount);
    } finally {
      fs.closeSync(descriptor);
    }
  }
  function assertPinnedDirectory() {
    assertReceiptDirectory();
    const current = fs.statSync(directory);
    const pinned = fs.fstatSync(directoryDescriptor);
    if (!pinned.isDirectory() || current.dev !== pinned.dev || current.ino !== pinned.ino) {
      fail("QODER_REAL_RECEIPT_FILE parent changed during publication");
    }
    let anchoredReal;
    try {
      anchoredReal = fs.realpathSync(anchoredDirectory);
    } catch {
      fail("QODER_REAL_RECEIPT_FILE requires descriptor-anchored publication");
    }
    const relative = path.relative(repoRoot, anchoredReal);
    if (!relative.startsWith("..") && !path.isAbsolute(relative)) {
      fail("QODER_REAL_RECEIPT_FILE parent must stay outside the repository");
    }
  }
  const target = path.join(anchoredDirectory, path.basename(receiptFile));
  const temporary = `${target}.tmp-${process.pid}`;
  let created = false;
  let published = false;
  let receiptDescriptor;
  function unlinkTemporaryIfOwned() {
    let current;
    try {
      current = fs.lstatSync(temporary, { bigint: true });
    } catch (error) {
      if (error?.code === "ENOENT") return;
      throw error;
    }
    const owned = fs.fstatSync(receiptDescriptor, { bigint: true });
    if (current.dev === owned.dev && current.ino === owned.ino) {
      fs.unlinkSync(temporary);
    }
  }
  function unlinkPublishedIfOwned() {
    let current;
    try {
      current = fs.lstatSync(target, { bigint: true });
    } catch (error) {
      if (error?.code === "ENOENT") return;
      throw error;
    }
    const owned = fs.fstatSync(receiptDescriptor, { bigint: true });
    if (current.dev === owned.dev && current.ino === owned.ino) {
      fs.unlinkSync(target);
    }
  }
  try {
    assertPinnedDirectory();
    if (fs.existsSync(target)) fail("QODER_REAL_RECEIPT_FILE already exists");
    receiptDescriptor = fs.openSync(
      temporary,
      fs.constants.O_RDWR | fs.constants.O_CREAT | fs.constants.O_EXCL |
        (fs.constants.O_NOFOLLOW ?? 0),
      0o600,
    );
    created = true;
    fs.writeFileSync(receiptDescriptor, contents);
    assertReceiptContents(receiptDescriptor, null, 1n, false);
    fs.fchmodSync(receiptDescriptor, 0o600);
    const fingerprint = fs.fstatSync(receiptDescriptor, { bigint: true });
    assertPinnedDirectory();
    assertReceiptPath(temporary, fingerprint, 1n);
    verifyBeforePublish();
    // Linking is exclusive: a receipt created after the check must not be overwritten.
    fs.linkSync(temporary, target);
    published = true;
    assertPinnedDirectory();
    assertReceiptPath(target, fs.fstatSync(receiptDescriptor, { bigint: true }), 2n);
  } catch (error) {
    if (published) unlinkPublishedIfOwned();
    throw error;
  } finally {
    try {
      if (created) unlinkTemporaryIfOwned();
    } finally {
      try {
        if (receiptDescriptor !== undefined) fs.closeSync(receiptDescriptor);
      } finally {
        fs.closeSync(directoryDescriptor);
      }
    }
  }
}

async function main() {
  assertReceiptDirectory();
  if (process.platform !== "linux" || !fs.existsSync("/proc/self/fd")) {
    fail("QODER_REAL_RECEIPT_FILE requires Linux descriptor-anchored publication");
  }
  const targetCommit = gitCommit();
  const initialAccount = await accountSnapshot();
  const providerBindings = await validateBindings(initialAccount);
  const shareBinding = await validateShareBinding();
  const catalog = await validateCatalogs();
  const quotaState = await validateQuota(initialAccount);
  const surfaceResults = {
    claude: await validateClaude(catalog.model),
    codex: await validateCodex(catalog.model),
    gemini: await validateGemini(catalog.model),
  };
  const terminalChecks = Object.fromEntries(
    Object.entries(surfaceResults).map(([app, result]) => [
      app,
      { terminalCount: result.terminalCount, eof: result.eof },
    ]),
  );
  const bodyHashes = Object.fromEntries(
    Object.entries(surfaceResults).flatMap(([app, result]) =>
      Object.entries(result.bodyHashes).map(([mode, value]) => [`${app}_${mode}`, value]),
    ),
  );
  const finalAccount = await accountSnapshot();
  if (
    finalAccount.id !== initialAccount.id ||
    finalAccount.authIdentityGeneration !== initialAccount.authIdentityGeneration ||
    finalAccount.tokenRefreshGeneration !== initialAccount.tokenRefreshGeneration
  ) {
    fail("Qoder Account identity or credential generation drifted during acceptance");
  }
  const finalProviderBindings = await validateBindings(finalAccount);
  if (
    finalProviderBindings.digest !== providerBindings.digest ||
    JSON.stringify(finalProviderBindings.surfaceBindings) !==
      JSON.stringify(providerBindings.surfaceBindings)
  ) {
    fail("Qoder Provider bindings drifted during acceptance");
  }
  const finalShareBinding = await validateShareBinding();
  if (
    finalShareBinding.shareRevision !== shareBinding.shareRevision ||
    finalShareBinding.shareIdentityDigest !== shareBinding.shareIdentityDigest ||
    finalShareBinding.shareBindingDigest !== shareBinding.shareBindingDigest
  ) {
    fail("Qoder Share binding drifted during acceptance");
  }
  const finalCatalog = await validateCatalogs(catalog.model);
  for (const app of Object.keys(catalog.snapshots)) {
    if (finalCatalog.snapshots[app].digest !== catalog.snapshots[app].digest) {
      fail(`${app} Qoder model catalog changed during acceptance`);
    }
  }
  const assertBoundCatalogsFresh = () => {
    const now = Date.now();
    assertFreshCatalogs(catalog.snapshots, now);
    assertFreshCatalogs(finalCatalog.snapshots, now);
  };

  // This harness observes the happy-path data plane. Login/import, credential
  // rotation, authoritative-empty discovery, and controlled 401 scenarios are
  // separate destructive/manual gates, so this receipt must never promote the
  // whole rail by itself.
  const verificationState = fixtureMode ? "contract_verified" : "partial_live_verified";
  const liveState = "live_pending";
  const accountIdentityDigest = digest("cc-switch-server:qoder-account:v2", {
    id: finalAccount.id,
    site: spec.site,
    rail,
    authIdentityGeneration: finalAccount.authIdentityGeneration,
    tokenRefreshGeneration: finalAccount.tokenRefreshGeneration,
  });
  const scopeDigest = digest("cc-switch-server:qoder-real-scope:v2", {
    targetCommit,
    harnessRevision: HARNESS_REVISION,
    rail,
    site: spec.site,
    model: catalog.model,
    accountIdentityDigest,
    providerBindingDigest: providerBindings.digest,
    shareBindingDigest: shareBinding.shareBindingDigest,
    catalogs: catalog.snapshots,
  });
  const receipt = {
    schemaVersion: 2,
    harnessRevision: HARNESS_REVISION,
    providerType: "qoder_cosy",
    verificationState,
    liveState,
    acceptanceChecks: {
      login_or_pat_import: "not_observed",
      refresh_rotation_or_job_token_exchange: "not_observed",
      fresh_catalog: "pass",
      authoritative_empty_catalog: "not_observed",
      quota: "pass",
      claude_nonstream_stream_tool_usage: "pass",
      codex_nonstream_stream_tool_usage: "pass",
      gemini_nonstream_stream_tool_usage: "pass",
      same_account_first_401_recovery: "not_observed",
      second_401_terminal: "not_observed",
      unique_terminal_then_eof: "pass",
      three_decoy_zero_request_counts: "binding_only",
      site_rail_and_generation_digest: "pass",
      secret_scan: "pass",
    },
    accountIdentityDigest,
    authIdentityGeneration: finalAccount.authIdentityGeneration,
    bodyHashes,
    catalogs: catalog.snapshots,
    catalogSource: catalog.snapshots.claude.source,
    catalogStale: false,
    commit: targetCommit,
    credentialRail: rail === "global_pat" ? "pat_job_token" : rail,
    model: catalog.model,
    otherAccountRequests: 0,
    otherProviderRequests: 0,
    otherSiteRequests: 0,
    pathHeaderSchemaDigest: digest("cc-switch-server:qoder-path-header:v1", {
      site: oracleRail.site,
      credentialRail: oracleRail.credentialRail,
      origins: oracleRail.origins,
      login: oracleRail.login,
      wire: oracleRail.wire,
      catalog: oracleRail.catalog,
      quota: oracleRail.quota,
    }),
    providerBindingDigest: providerBindings.digest,
    quotaState,
    recoveryDecisions: {
      sameAccountFirst401: "not_observed",
      second401: "not_observed",
      crossRailFallback: "disabled",
      crossSiteFallback: "disabled",
      crossAccountFallback: "disabled",
      crossProviderFallback: "disabled",
      postCommitReplay: "disabled",
    },
    sensitiveScan: {
      status: "pass",
      matches: sensitiveMatches,
      scannedBytes: sensitiveBytes,
    },
    scopeDigest,
    shareBindingDigest: shareBinding.shareBindingDigest,
    shareIdentityDigest: shareBinding.shareIdentityDigest,
    shareRevision: shareBinding.shareRevision,
    site: spec.site,
    surfaceBindings: providerBindings.surfaceBindings,
    surfaceChecks: {
      claude: { nonstream: "pass", stream: "pass", tool: "pass" },
      codex: { nonstream: "pass", stream: "pass", tool: "pass" },
      gemini: { nonstream: "pass", stream: "pass", tool: "pass" },
    },
    terminalChecks,
    targetCommit,
    timestamp: new Date().toISOString(),
    tokenRefreshGeneration: finalAccount.tokenRefreshGeneration,
  };
  assertBoundCatalogsFresh();
  writeReceipt(receipt, assertBoundCatalogsFresh);
  console.log(
    `[PASS] Qoder ${rail} acceptance receipt written (verificationState=${verificationState}, liveState=${liveState})`,
  );
}

main().catch((error) => {
  const message = redact(error instanceof Error ? error.message : "Qoder acceptance failed");
  console.error(`[FAIL] ${message}`);
  process.exit(1);
});
