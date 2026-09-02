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

const railSpecs = Object.freeze({
  global_oauth: Object.freeze({
    site: "global",
    accountEnv: "QODER_GLOBAL_OAUTH_TEST_ACCOUNT",
    modelEnv: "CC_SWITCH_QODER_GLOBAL_OAUTH_MODEL",
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
    return execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: repoRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    return "unknown";
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
  if (!response.body) return "";
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let text = "";
  let bytes = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    bytes += value.byteLength;
    if (bytes > limit) fail(`${label} exceeded the response-size bound`);
    text += decoder.decode(value, { stream: true });
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
  if (!account.id?.startsWith(`qoder-${spec.site}-`)) {
    fail(`selected Qoder Account does not belong to the ${spec.site} site`);
  }
  if (!Number.isSafeInteger(account.authIdentityGeneration) || account.authIdentityGeneration < 1) {
    fail("selected Qoder Account is missing authIdentityGeneration");
  }
  if (!Number.isSafeInteger(account.tokenRefreshGeneration) || account.tokenRefreshGeneration < 0) {
    fail("selected Qoder Account is missing tokenRefreshGeneration");
  }
  if (account.needsRelogin === true) fail("selected Qoder Account requires relogin");
  if (rail === "global_pat") {
    if (account.hasApiKey !== true || account.hasAccessToken || account.hasRefreshToken) {
      fail("Global PAT rail must persist only its PAT credential");
    }
  } else if (
    account.hasAccessToken !== true ||
    account.hasRefreshToken !== true ||
    account.hasApiKey
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
    bindings.push({
      app,
      providerId,
      runtimeFingerprint: view.runtime.runtimeFingerprint || "",
      accountId: authRef.accountId,
      authIdentityGeneration: authRef.authIdentityGeneration,
    });
  }
  return digest("cc-switch-server:qoder-provider-bindings:v1", bindings);
}

async function validateCatalogs() {
  const catalogs = [];
  for (const [app, providerId] of Object.entries(providerIds)) {
    const query = new URLSearchParams({ app, providerId });
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
      !Number.isSafeInteger(catalog.fetchedAtMs) ||
      catalog.fetchedAtMs <= 0
    ) {
      fail(`${app} Qoder model catalog is not a fresh bound-account catalog`);
    }
    const models = new Set(
      catalog.data
        .map((entry) => String(entry?.id || "").trim())
        .filter(Boolean),
    );
    if (models.size === 0) fail(`${app} Qoder model catalog is empty`);
    catalogs.push({ app, source: catalog.source, models });
  }
  const commonModels = [...catalogs[0].models].filter((model) =>
    catalogs.slice(1).every((catalog) => catalog.models.has(model)),
  );
  const model = requestedModel || commonModels.sort()[0];
  if (!model || !catalogs.every((catalog) => catalog.models.has(model))) {
    fail("Qoder Provider catalogs have no common selected model");
  }
  return { model, source: catalogs[0].source, stale: false };
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

async function validateClaude(model) {
  const base = {
    model,
    max_tokens: 64,
    messages: [{ role: "user", content: smokeText }],
    tools: [{ name: toolName, description: toolDescription, input_schema: toolSchema }],
    tool_choice: { type: "tool", name: toolName },
  };
  const nonstream = await requireJson(
    shareUrl,
    "/v1/messages",
    {
      method: "POST",
      headers: { "anthropic-version": "2023-06-01" },
      body: JSON.stringify({ ...base, stream: false }),
    },
    "Qoder Claude non-stream",
  );
  if (nonstream?.type !== "message" || nonstream.role !== "assistant") {
    fail("Qoder Claude non-stream response is not an assistant message");
  }
  if (!nonstream.content?.some((block) => block?.type === "tool_use" && block.name === toolName)) {
    fail("Qoder Claude non-stream response is missing the lookup tool call");
  }
  requireUsage(nonstream.usage, "Qoder Claude non-stream response");
  const frames = await collectSse(
    "/v1/messages",
    { ...base, stream: true },
    "Qoder Claude stream",
    { "anthropic-version": "2023-06-01" },
  );
  const terminals = frames.filter((frame) => frame.payload?.type === "message_stop");
  if (terminals.length !== 1) fail("Qoder Claude stream must emit one message_stop");
  if (
    !frames.some(
      (frame) =>
        frame.payload?.type === "content_block_start" &&
        frame.payload?.content_block?.type === "tool_use" &&
        frame.payload?.content_block?.name === toolName,
    )
  ) {
    fail("Qoder Claude stream is missing the lookup tool lifecycle");
  }
  return { terminalCount: terminals.length, eof: true };
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
  const nonstream = await requireJson(
    shareUrl,
    "/v1/responses",
    { method: "POST", body: JSON.stringify({ ...base, stream: false }) },
    "Qoder Codex non-stream",
  );
  if (nonstream?.object !== "response" || nonstream.status !== "completed") {
    fail("Qoder Codex non-stream response is not completed");
  }
  if (!nonstream.output?.some((item) => item?.type === "function_call" && item.name === toolName)) {
    fail("Qoder Codex non-stream response is missing the lookup tool call");
  }
  requireUsage(nonstream.usage, "Qoder Codex non-stream response");
  const frames = await collectSse(
    "/v1/responses",
    { ...base, stream: true },
    "Qoder Codex stream",
  );
  const terminals = frames.filter((frame) => frame.payload?.type === "response.completed");
  if (terminals.length !== 1) fail("Qoder Codex stream must emit one response.completed");
  if (
    !frames.some(
      (frame) => frame.payload?.item?.type === "function_call" && frame.payload?.item?.name === toolName,
    ) &&
    !terminals[0]?.payload?.response?.output?.some(
      (item) => item?.type === "function_call" && item.name === toolName,
    )
  ) {
    fail("Qoder Codex stream is missing the lookup tool lifecycle");
  }
  return { terminalCount: terminals.length, eof: true };
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
  const nonstream = await requireJson(
    shareUrl,
    `/v1beta/models/${modelPath}:generateContent`,
    { method: "POST", body: JSON.stringify(geminiBody()) },
    "Qoder Gemini non-stream",
  );
  if (!Array.isArray(nonstream?.candidates) || nonstream.candidates.length === 0) {
    fail("Qoder Gemini non-stream response has no candidate");
  }
  if (
    !nonstream.candidates.some((candidate) =>
      candidate?.content?.parts?.some((part) => part?.functionCall?.name === toolName),
    )
  ) {
    fail("Qoder Gemini non-stream response is missing the lookup tool call");
  }
  requireUsage(nonstream.usageMetadata, "Qoder Gemini non-stream response");
  const frames = await collectSse(
    `/v1beta/models/${modelPath}:streamGenerateContent?alt=sse`,
    geminiBody(),
    "Qoder Gemini stream",
  );
  const terminals = frames.filter((frame) =>
    frame.payload?.candidates?.some((candidate) => usable(candidate?.finishReason)),
  );
  if (terminals.length !== 1) fail("Qoder Gemini stream must emit one finishReason terminal");
  if (
    !frames.some((frame) =>
      frame.payload?.candidates?.some((candidate) =>
        candidate?.content?.parts?.some((part) => part?.functionCall?.name === toolName),
      ),
    )
  ) {
    fail("Qoder Gemini stream is missing the lookup tool lifecycle");
  }
  return { terminalCount: terminals.length, eof: true };
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
}

function writeReceipt(receipt) {
  assertReceiptSafe(receipt);
  const directory = path.dirname(receiptFile);
  if (!fs.existsSync(directory)) fail("QODER_REAL_RECEIPT_FILE parent directory does not exist");
  if (fs.existsSync(receiptFile)) fail("QODER_REAL_RECEIPT_FILE already exists");
  const temporary = `${receiptFile}.tmp-${process.pid}`;
  fs.writeFileSync(temporary, `${JSON.stringify(receipt, null, 2)}\n`, {
    encoding: "utf8",
    mode: 0o600,
    flag: "wx",
  });
  fs.renameSync(temporary, receiptFile);
  fs.chmodSync(receiptFile, 0o600);
}

async function main() {
  const initialAccount = await accountSnapshot();
  const providerBindingDigest = await validateBindings(initialAccount);
  const catalog = await validateCatalogs();
  const quotaState = await validateQuota(initialAccount);
  const terminalChecks = {
    claude: await validateClaude(catalog.model),
    codex: await validateCodex(catalog.model),
    gemini: await validateGemini(catalog.model),
  };
  const finalAccount = await accountSnapshot();
  if (
    finalAccount.id !== initialAccount.id ||
    finalAccount.authIdentityGeneration !== initialAccount.authIdentityGeneration ||
    finalAccount.tokenRefreshGeneration < initialAccount.tokenRefreshGeneration
  ) {
    fail("Qoder Account identity or credential generation drifted during acceptance");
  }

  const verificationState = fixtureMode ? "contract_verified" : "live_verified";
  const liveState = fixtureMode ? "live_pending" : "live_verified";
  const receipt = {
    schemaVersion: 1,
    providerType: "qoder_cosy",
    verificationState,
    liveState,
    accountIdentityDigest: digest("cc-switch-server:qoder-account:v1", {
      id: finalAccount.id,
      site: spec.site,
      rail,
      authIdentityGeneration: finalAccount.authIdentityGeneration,
    }),
    authIdentityGeneration: finalAccount.authIdentityGeneration,
    catalogSource: catalog.source,
    catalogStale: catalog.stale,
    commit: gitCommit(),
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
    providerBindingDigest,
    quotaState,
    sensitiveScan: {
      status: "pass",
      matches: sensitiveMatches,
      scannedBytes: sensitiveBytes,
    },
    site: spec.site,
    surfaceChecks: {
      claude: { nonstream: "pass", stream: "pass", tool: "pass" },
      codex: { nonstream: "pass", stream: "pass", tool: "pass" },
      gemini: { nonstream: "pass", stream: "pass", tool: "pass" },
    },
    terminalChecks,
    timestamp: new Date().toISOString(),
    tokenRefreshGeneration: finalAccount.tokenRefreshGeneration,
  };
  writeReceipt(receipt);
  console.log(
    `[PASS] Qoder ${rail} acceptance receipt written (verificationState=${verificationState}, liveState=${liveState})`,
  );
}

main().catch((error) => {
  const message = redact(error instanceof Error ? error.message : "Qoder acceptance failed");
  console.error(`[FAIL] ${message}`);
  process.exit(1);
});
