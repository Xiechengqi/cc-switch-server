#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const shareUrl = (process.env.CC_SWITCH_SHARE_URL || "").trim().replace(/\/+$/, "");
const routerToken = (process.env.ROUTER_API_TOKEN || "").trim();
const routerTokenHeader = (process.env.ROUTER_API_TOKEN_HEADER || "Authorization").trim();
const serverUrl = (process.env.SERVER_URL || "").trim().replace(/\/+$/, "");
const serverToken = (process.env.CC_SWITCH_SERVER_TOKEN || "").trim();
const accountSelector = (process.env.GITHUB_COPILOT_TEST_ACCOUNT || "").trim();
const expectedDomain = normalizeDomain(process.env.GITHUB_COPILOT_GITHUB_DOMAIN || "");
const requestedModel = (process.env.CC_SWITCH_COPILOT_MODEL || "").trim();
const providerIds = Object.freeze({
  claude: (process.env.CC_SWITCH_COPILOT_CLAUDE_PROVIDER_ID || "").trim(),
  codex: (process.env.CC_SWITCH_COPILOT_CODEX_PROVIDER_ID || "").trim(),
  gemini: (process.env.CC_SWITCH_COPILOT_GEMINI_PROVIDER_ID || "").trim(),
});
const providerSurfaces = Object.freeze([
  { app: "claude", providerId: providerIds.claude },
  { app: "codex", providerId: providerIds.codex },
  { app: "gemini", providerId: providerIds.gemini },
]);
const evidenceFile = (process.env.EVIDENCE_FILE || "").trim();
const configuredTimeoutMs = Number(process.env.CC_SWITCH_REAL_TIMEOUT_MS || 120_000);
const timeoutMs = Number.isFinite(configuredTimeoutMs)
  ? Math.max(1_000, Math.min(300_000, Math.trunc(configuredTimeoutMs)))
  : 120_000;
const checks = {
  bindings: "not-run",
  models: "not-run",
  quota: "not-run",
  claude: "not-run",
  codex: "not-run",
  gemini: "not-run",
};
let selectedModel = requestedModel;

function isUsable(value) {
  const text = String(value || "").trim();
  return Boolean(text) && !text.includes("<") && !text.includes(">");
}

function normalizeDomain(value) {
  const text = String(value || "").trim().toLowerCase();
  if (!text) return "";
  try {
    const parsed = new URL(text.includes("://") ? text : `https://${text}`);
    return parsed.hostname.toLowerCase();
  } catch {
    return text.replace(/^https?:\/\//, "").replace(/\/+$/, "");
  }
}

function fail(message) {
  throw new Error(message);
}

function redact(value) {
  let text = String(value);
  const secrets = [
    routerToken,
    serverToken,
    process.env.GITHUB_COPILOT_TOKEN_FIXTURE,
  ].filter(isUsable);
  for (const secret of secrets) text = text.split(secret).join("[REDACTED]");
  return text
    .replace(/Bearer\s+[^\s,"'}]+/gi, "Bearer [REDACTED]")
    .replace(/("(?:access|refresh|id)_token"\s*:\s*")[^"]+/gi, "$1[REDACTED]")
    .replace(/\b(?:gh[opsu]_|github_pat_)[A-Za-z0-9_]{12,}\b/g, "[REDACTED_GITHUB_TOKEN]")
    .replace(/\beyJ[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{8,}\b/g, "[REDACTED_JWT]");
}

function safePreview(value, limit = 500) {
  return redact(value).replace(/[\r\n\t]+/g, " ").slice(0, limit);
}

function writeEvidence(status, verificationState, notes = "") {
  if (!evidenceFile) return;
  const writer = fileURLToPath(new URL("./write-acceptance-evidence.mjs", import.meta.url));
  execFileSync(process.execPath, [writer, "--out", evidenceFile], {
    stdio: "inherit",
    env: {
      ...process.env,
      EVIDENCE_STAGE: "github-copilot-real",
      EVIDENCE_STATUS: status,
      EVIDENCE_VERIFICATION_STATE: verificationState,
      EVIDENCE_VERIFICATION_SCOPE: "bound_github_copilot_account_three_surfaces",
      EVIDENCE_TARGET: "github-copilot-three-surface",
      EVIDENCE_SOURCE: "scripts/smoke/copilot-real.mjs",
      EVIDENCE_PROVIDER: "router-share-binding",
      EVIDENCE_PROVIDER_TYPE: "github_copilot",
      EVIDENCE_NOTES: notes,
      PROBE_MODEL: selectedModel,
      COPILOT_GATE_STATUS: status,
      COPILOT_BINDINGS_STATUS: checks.bindings,
      COPILOT_MODELS_STATUS: checks.models,
      COPILOT_QUOTA_STATUS: checks.quota,
      COPILOT_CLAUDE_STATUS: checks.claude,
      COPILOT_CODEX_STATUS: checks.codex,
      COPILOT_GEMINI_STATUS: checks.gemini,
    },
  });
}

const missingInputs = [
  ["RUN_REAL=1", process.env.RUN_REAL === "1" ? "1" : ""],
  ["CC_SWITCH_SHARE_URL", shareUrl],
  ["ROUTER_API_TOKEN", routerToken],
  ["SERVER_URL", serverUrl],
  ["CC_SWITCH_SERVER_TOKEN", serverToken],
  ["GITHUB_COPILOT_TEST_ACCOUNT", accountSelector],
  ["CC_SWITCH_COPILOT_CLAUDE_PROVIDER_ID", providerIds.claude],
  ["CC_SWITCH_COPILOT_CODEX_PROVIDER_ID", providerIds.codex],
  ["CC_SWITCH_COPILOT_GEMINI_PROVIDER_ID", providerIds.gemini],
]
  .filter(([, value]) => !isUsable(value))
  .map(([name]) => name);

if (missingInputs.length > 0) {
  console.log(
    `[SKIP] GitHub Copilot real-account gate requires non-placeholder ${missingInputs.join(", ")}`,
  );
  writeEvidence("blocked-inputs", "blocked_inputs", `missing ${missingInputs.join(", ")}`);
  process.exit(0);
}

function routerHeaders(extra = {}) {
  const headers = new Headers({ accept: "application/json", ...extra });
  if (/^authorization$/i.test(routerTokenHeader)) {
    headers.set("authorization", `Bearer ${routerToken}`);
  } else if (/^(x-api-key|x-goog-api-key)$/i.test(routerTokenHeader)) {
    headers.set(routerTokenHeader, routerToken);
  } else {
    fail(`unsupported ROUTER_API_TOKEN_HEADER: ${routerTokenHeader}`);
  }
  return headers;
}

async function request(base, path, init = {}, { admin = false } = {}) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  const headers = admin
    ? new Headers({ accept: "application/json", authorization: `Bearer ${serverToken}` })
    : routerHeaders();
  for (const [name, value] of new Headers(init.headers || {})) headers.set(name, value);
  if (init.body !== undefined) headers.set("content-type", "application/json");
  try {
    const response = await fetch(`${base}${path}`, {
      ...init,
      headers,
      signal: controller.signal,
    });
    return { response, stopTimeout: () => clearTimeout(timer) };
  } catch (error) {
    clearTimeout(timer);
    throw error;
  }
}

async function readLimited(response, limit = 2 * 1024 * 1024) {
  if (!response.body) return "";
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let output = "";
  let bytes = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    bytes += value.byteLength;
    if (bytes > limit) fail(`response exceeded ${limit} byte acceptance bound`);
    output += decoder.decode(value, { stream: true });
  }
  return output + decoder.decode();
}

async function requireJson(base, path, init, label, options = {}) {
  const { response, stopTimeout } = await request(base, path, init, options);
  try {
    const text = await readLimited(response);
    if (!response.ok) fail(`${label} returned HTTP ${response.status}: ${safePreview(text)}`);
    try {
      return JSON.parse(text);
    } catch (error) {
      fail(`${label} returned invalid JSON: ${error.message}`);
    }
  } finally {
    stopTimeout();
  }
}

function nonNegativeNumber(value) {
  return typeof value === "number" && Number.isFinite(value) && value >= 0;
}

function normalizeModelId(value) {
  return String(value || "").trim().replace(/^models\//, "");
}

function own(object, key) {
  return Object.prototype.hasOwnProperty.call(object, key);
}

function validateCatalogModel(entry, surface) {
  if (!entry || typeof entry.id !== "string" || !entry.id.trim()) {
    fail(`${surface.app} Copilot catalog contains an invalid model entry`);
  }
  const raw = entry.raw;
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    fail(`${surface.app} Copilot model ${entry.id} is missing raw entitlement metadata`);
  }
  if (raw.entitlementSource !== "copilot_models_api") {
    fail(`${surface.app} Copilot model ${entry.id} is missing authoritative entitlement provenance`);
  }
  if (!own(raw, "modelPickerEnabled") || raw.modelPickerEnabled === false) {
    fail(`${surface.app} Copilot model ${entry.id} has an invalid picker state`);
  }
  if (
    typeof raw.policyState === "string" &&
    !["enabled", "preview"].includes(raw.policyState.toLowerCase())
  ) {
    fail(`${surface.app} Copilot model ${entry.id} has non-routable policy state ${raw.policyState}`);
  }
  if (!Array.isArray(raw.supportedEndpoints)) {
    fail(`${surface.app} Copilot model ${entry.id} is missing supportedEndpoints metadata`);
  }
  if (
    !raw.limits ||
    typeof raw.limits !== "object" ||
    !own(raw.limits, "maxContextWindowTokens") ||
    !own(raw.limits, "maxOutputTokens")
  ) {
    fail(`${surface.app} Copilot model ${entry.id} is missing limits metadata`);
  }
  if (
    !raw.capabilities ||
    typeof raw.capabilities !== "object" ||
    !own(raw.capabilities, "tools") ||
    !own(raw.capabilities, "vision") ||
    !own(raw.capabilities, "reasoning")
  ) {
    fail(`${surface.app} Copilot model ${entry.id} is missing capability metadata`);
  }
  const observedDomain = normalizeDomain(raw.githubDomain);
  if (!observedDomain) {
    fail(`${surface.app} Copilot model ${entry.id} is missing GitHub domain provenance`);
  }
  if (expectedDomain && observedDomain !== expectedDomain) {
    fail(`Copilot domain mismatch: expected ${expectedDomain}, observed ${observedDomain}`);
  }
  let origin;
  try {
    origin = new URL(raw.apiOrigin);
  } catch {
    fail(`${surface.app} Copilot model ${entry.id} is missing a valid API origin`);
  }
  if (
    origin.protocol !== "https:" ||
    origin.username ||
    origin.password ||
    (origin.pathname !== "/" && origin.pathname !== "") ||
    origin.search ||
    origin.hash
  ) {
    fail(`${surface.app} Copilot model ${entry.id} API origin does not satisfy the trusted HTTPS contract`);
  }
  return {
    id: normalizeModelId(entry.id),
    entry,
    domain: observedDomain,
    origin: origin.origin,
  };
}

function validateCatalogResponse(catalog, surface) {
  if (
    catalog?.ok !== true ||
    catalog.outcome !== "success" ||
    catalog.providerId !== surface.providerId ||
    catalog.app !== surface.app ||
    catalog.providerType !== "github_copilot" ||
    catalog.driverId !== "special.copilot" ||
    !Array.isArray(catalog.models)
  ) {
    fail(`${surface.app} fetch-models response does not satisfy the bound Copilot contract`);
  }
  if (catalog.stale !== false) {
    fail(`${surface.app} live Copilot acceptance requires a fresh model catalog`);
  }
  if (!Number.isSafeInteger(catalog.fetchedAtMs) || catalog.fetchedAtMs <= 0) {
    fail(`${surface.app} fresh Copilot catalog is missing fetchedAtMs`);
  }
  const ageMs = Date.now() - catalog.fetchedAtMs;
  if (ageMs < -5 * 60_000 || ageMs >= 10 * 60_000) {
    fail(`${surface.app} Copilot catalog timestamp is outside the fresh acceptance window`);
  }
  if (!["copilot_models_api", "copilot_account_cache"].includes(catalog.source)) {
    fail(`${surface.app} Copilot catalog has non-authoritative source ${catalog.source || "missing"}`);
  }
  const candidates = catalog.models.map((entry) => validateCatalogModel(entry, surface));
  if (candidates.length === 0) {
    fail(`${surface.app} bound Copilot account returned an empty entitlement catalog`);
  }
  return { surface, catalog, candidates };
}

async function validateCatalogs() {
  const catalogs = [];
  for (const surface of providerSurfaces) {
    const catalog = await requireJson(
      serverUrl,
      `/api/providers/${encodeURIComponent(surface.providerId)}/fetch-models`,
      {
        method: "POST",
        body: JSON.stringify({ app: surface.app, merge: false, timeoutMs }),
      },
      `${surface.app} Copilot model discovery`,
      { admin: true },
    );
    catalogs.push(validateCatalogResponse(catalog, surface));
  }

  const candidateMaps = catalogs.map(
    (catalog) => new Map(catalog.candidates.map((candidate) => [candidate.id, candidate])),
  );
  const commonIds = [...candidateMaps[0].keys()].filter((id) =>
    candidateMaps.slice(1).every((models) => models.has(id)),
  );
  const requestedId = normalizeModelId(requestedModel);
  if (requestedId && !commonIds.includes(requestedId)) {
    fail(`Copilot catalogs do not all contain requested model ${requestedModel}`);
  }
  const ranked = (requestedId ? [requestedId] : commonIds).sort((leftId, rightId) => {
    const left = candidateMaps[0].get(leftId)?.entry?.raw || {};
    const right = candidateMaps[0].get(rightId)?.entry?.raw || {};
    const leftRank = Number(left.preview === true) + Number(left.capabilities?.tools === false) * 2;
    const rightRank = Number(right.preview === true) + Number(right.capabilities?.tools === false) * 2;
    return leftRank - rightRank || leftId.localeCompare(rightId);
  });
  if (ranked.length === 0) {
    fail("the three bound Copilot Provider catalogs have no common entitled model");
  }
  selectedModel = ranked[0];

  const selected = candidateMaps.map((models) => models.get(selectedModel));
  const domains = new Set(selected.map((candidate) => candidate.domain));
  const origins = new Set(selected.map((candidate) => candidate.origin));
  if (domains.size !== 1 || origins.size !== 1) {
    fail("the three Copilot Provider catalogs disagree on GitHub domain or API origin");
  }
  checks.models = "pass";
  console.log(
    `[PASS] Copilot model entitlement across three Providers (model=${selectedModel}, domain=${selected[0].domain}, origin=${selected[0].origin})`,
  );
}

function resolveAccount(accounts) {
  const email = accountSelector.toLowerCase();
  const matches = accounts.filter(
    (account) =>
      account?.providerType === "github_copilot" &&
      (account.id === accountSelector ||
        (typeof account.email === "string" && account.email.trim().toLowerCase() === email)),
  );
  if (matches.length !== 1) {
    fail(`GITHUB_COPILOT_TEST_ACCOUNT matched ${matches.length} github_copilot accounts`);
  }
  return matches[0];
}

async function validateControlPlane() {
  const accountsResponse = await requireJson(
    serverUrl,
    "/api/accounts",
    { method: "GET" },
    "Copilot account list",
    { admin: true },
  );
  if (!accountsResponse?.ok || !Array.isArray(accountsResponse.accounts)) {
    fail("account list does not satisfy the public API contract");
  }
  const account = resolveAccount(accountsResponse.accounts);
  if (!Number.isSafeInteger(account.authIdentityGeneration)) {
    fail("selected Copilot account is missing auth identity generation");
  }
  const providersResponse = await requireJson(
    serverUrl,
    "/api/providers",
    { method: "GET" },
    "Copilot Provider list",
    { admin: true },
  );
  if (!providersResponse?.ok || !Array.isArray(providersResponse.providers)) {
    fail("Provider list does not satisfy the public API contract");
  }
  for (const surface of providerSurfaces) {
    const matches = providersResponse.providers.filter(
      (view) => view?.app === surface.app && view?.provider?.id === surface.providerId,
    );
    if (matches.length !== 1) {
      fail(`${surface.app} Provider ID ${surface.providerId} matched ${matches.length} Providers`);
    }
    const view = matches[0];
    const authRef = view.runtime?.authRef;
    if (
      view.providerType !== "github_copilot" ||
      view.providerTypeId !== "github_copilot" ||
      view.runtime?.driverId !== "special.copilot" ||
      view.runtime?.configurationState !== "ready" ||
      authRef?.kind !== "managed_account" ||
      authRef.expectedProviderType !== "github_copilot" ||
      authRef.accountId !== account.id ||
      authRef.authIdentityGeneration !== account.authIdentityGeneration
    ) {
      fail(`${surface.app} Copilot Provider is not ready on the selected Account generation`);
    }
  }
  checks.bindings = "pass";
  console.log(
    `[PASS] Copilot Claude/Codex/Gemini Providers bind one Account generation (${account.id}@${account.authIdentityGeneration})`,
  );
  return account;
}

async function validateQuota(account) {
  const quota = await requireJson(
    serverUrl,
    `/api/accounts/${encodeURIComponent(account.id)}/quota?refresh=true&force=true`,
    { method: "GET" },
    "Copilot quota refresh",
    { admin: true },
  );
  if (!quota?.ok || !quota.quota?.success || quota.account?.providerType !== "github_copilot") {
    fail("Copilot quota refresh did not return a successful bound-account snapshot");
  }
  const premium = quota.quota.tiers?.find((tier) => tier?.name === "premium");
  if (!premium || premium.unit !== "premium_interactions") {
    fail("Copilot quota is missing the premium_interactions tier");
  }
  if (premium.utilization !== undefined && !nonNegativeNumber(premium.utilization)) {
    fail("Copilot premium utilization is invalid");
  }
  if (!Number.isSafeInteger(quota.account.authIdentityGeneration)) {
    fail("Copilot quota response is missing auth identity generation");
  }
  checks.quota = "pass";
  console.log(`[PASS] Copilot premium quota (plan=${quota.quota.credentialMessage || "unknown"})`);
}

const toolName = "lookup";
const toolDescription = "Return one deterministic smoke-test value for the supplied key.";
const toolSchema = {
  type: "object",
  additionalProperties: false,
  properties: { key: { type: "string" } },
  required: ["key"],
};

function nextSseBoundary(buffer) {
  const match = /\r\n\r\n|\n\n|\r\r/.exec(buffer);
  return match ? { index: match.index, length: match[0].length } : null;
}

function parseSseFrame(frame) {
  let eventName = "";
  const data = [];
  for (const line of frame.split(/\r\n|\r|\n/)) {
    if (!line || line.startsWith(":")) continue;
    if (line.startsWith("event:")) eventName = line.slice(6).replace(/^ /, "");
    if (line.startsWith("data:")) data.push(line.slice(5).replace(/^ /, ""));
  }
  if (data.length === 0) return null;
  const joined = data.join("\n");
  if (joined === "[DONE]") return { eventName, done: true, payload: null };
  try {
    return { eventName, done: false, payload: JSON.parse(joined) };
  } catch (error) {
    fail(`stream emitted invalid SSE JSON: ${error.message}`);
  }
}

async function collectSse(path, body, label, extraHeaders = {}) {
  const { response, stopTimeout } = await request(shareUrl, path, {
    method: "POST",
    headers: { accept: "text/event-stream", ...extraHeaders },
    body: JSON.stringify(body),
  });
  try {
    if (!response.ok) {
      fail(`${label} returned HTTP ${response.status}: ${safePreview(await readLimited(response))}`);
    }
    const contentType = response.headers.get("content-type") || "";
    if (!contentType.toLowerCase().includes("text/event-stream")) {
      fail(`${label} returned unexpected content-type ${contentType}`);
    }
    if (!response.body) fail(`${label} returned no body`);
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    const frames = [];
    let buffer = "";
    let bytes = 0;
    while (true) {
      const { done, value } = await reader.read();
      bytes += value?.byteLength || 0;
      if (bytes > 8 * 1024 * 1024) fail(`${label} exceeded the 8 MiB stream bound`);
      buffer += decoder.decode(value || new Uint8Array(), { stream: !done });
      while (true) {
        const boundary = nextSseBoundary(buffer);
        if (!boundary) break;
        const frame = buffer.slice(0, boundary.index);
        buffer = buffer.slice(boundary.index + boundary.length);
        const parsed = parseSseFrame(frame);
        if (parsed) frames.push(parsed);
      }
      if (done) break;
    }
    if (buffer.trim()) {
      const parsed = parseSseFrame(buffer);
      if (parsed) frames.push(parsed);
    }
    if (frames.length === 0) fail(`${label} ended without SSE data`);
    return frames;
  } finally {
    stopTimeout();
  }
}

function requireClaudeTool(message) {
  if (message?.type !== "message" || message.role !== "assistant") {
    fail("Claude non-stream result is not an Anthropic message");
  }
  if (!message.content?.some((block) => block?.type === "tool_use" && block.name === toolName)) {
    fail("Claude non-stream result is missing the forced lookup tool");
  }
  if (!nonNegativeNumber(message.usage?.input_tokens) || !nonNegativeNumber(message.usage?.output_tokens)) {
    fail("Claude non-stream result is missing usage");
  }
}

async function validateClaude() {
  const baseBody = {
    model: selectedModel,
    max_tokens: 96,
    messages: [{ role: "user", content: "Call lookup with key copilot-claude. Do not answer in prose." }],
    tools: [{ name: toolName, description: toolDescription, input_schema: toolSchema }],
    tool_choice: { type: "tool", name: toolName },
  };
  requireClaudeTool(
    await requireJson(
      shareUrl,
      "/v1/messages",
      { method: "POST", headers: { "anthropic-version": "2023-06-01" }, body: JSON.stringify({ ...baseBody, stream: false }) },
      "Copilot Claude non-stream",
    ),
  );
  const frames = await collectSse(
    "/v1/messages",
    { ...baseBody, stream: true },
    "Copilot Claude stream",
    { "anthropic-version": "2023-06-01" },
  );
  const payloads = frames.map((frame) => frame.payload).filter(Boolean);
  if (payloads.filter((payload) => payload.type === "message_stop").length !== 1) {
    fail("Claude stream must emit exactly one message_stop");
  }
  if (!payloads.some((payload) => payload.type === "content_block_start" && payload.content_block?.type === "tool_use" && payload.content_block?.name === toolName)) {
    fail("Claude stream is missing the forced lookup tool lifecycle");
  }
  if (!payloads.some((payload) => payload.message?.usage || payload.usage)) {
    fail("Claude stream is missing usage evidence");
  }
  checks.claude = "pass";
  console.log("[PASS] Copilot Claude non-stream/stream tool, usage, terminal");
}

function requireCodexTool(response) {
  if (response?.object !== "response" || response.status !== "completed") {
    fail("Codex non-stream result is not a completed Responses object");
  }
  if (!response.output?.some((item) => item?.type === "function_call" && item.name === toolName)) {
    fail("Codex non-stream result is missing the forced lookup tool");
  }
  if (!nonNegativeNumber(response.usage?.input_tokens) || !nonNegativeNumber(response.usage?.output_tokens)) {
    fail("Codex non-stream result is missing usage");
  }
}

async function validateCodex() {
  const baseBody = {
    model: selectedModel,
    input: "Call lookup with key copilot-codex. Do not answer in prose.",
    max_output_tokens: 96,
    store: false,
    tools: [{ type: "function", name: toolName, description: toolDescription, parameters: toolSchema }],
    tool_choice: { type: "function", name: toolName },
  };
  requireCodexTool(
    await requireJson(
      shareUrl,
      "/v1/responses",
      { method: "POST", body: JSON.stringify({ ...baseBody, stream: false }) },
      "Copilot Codex non-stream",
    ),
  );
  const frames = await collectSse(
    "/v1/responses",
    { ...baseBody, stream: true },
    "Copilot Codex stream",
  );
  const payloads = frames.map((frame) => frame.payload).filter(Boolean);
  const terminals = payloads.filter((payload) => payload.type === "response.completed");
  if (terminals.length !== 1) fail("Codex stream must emit exactly one response.completed");
  const toolSeen = payloads.some(
    (payload) =>
      payload.item?.type === "function_call" && payload.item?.name === toolName,
  ) || terminals[0]?.response?.output?.some(
    (item) => item?.type === "function_call" && item.name === toolName,
  );
  if (!toolSeen) fail("Codex stream is missing the forced lookup tool lifecycle");
  if (!terminals[0]?.response?.usage) fail("Codex stream terminal is missing usage");
  checks.codex = "pass";
  console.log("[PASS] Copilot Codex non-stream/stream tool, usage, terminal");
}

function geminiToolBody(key) {
  return {
    contents: [{ role: "user", parts: [{ text: `Call lookup with key ${key}. Do not answer in prose.` }] }],
    tools: [{ functionDeclarations: [{ name: toolName, description: toolDescription, parameters: toolSchema }] }],
    toolConfig: { functionCallingConfig: { mode: "ANY", allowedFunctionNames: [toolName] } },
    generationConfig: { maxOutputTokens: 96 },
  };
}

function geminiParts(payload) {
  return payload?.candidates?.flatMap((candidate) => candidate?.content?.parts || []) || [];
}

async function validateGemini() {
  const modelPath = encodeURIComponent(selectedModel);
  const result = await requireJson(
    shareUrl,
    `/v1beta/models/${modelPath}:generateContent`,
    { method: "POST", body: JSON.stringify(geminiToolBody("copilot-gemini")) },
    "Copilot Gemini non-stream",
  );
  if (!geminiParts(result).some((part) => part?.functionCall?.name === toolName)) {
    fail("Gemini non-stream result is missing the forced lookup tool");
  }
  if (!result.usageMetadata || !nonNegativeNumber(result.usageMetadata.totalTokenCount)) {
    fail("Gemini non-stream result is missing usageMetadata");
  }
  const frames = await collectSse(
    `/v1beta/models/${modelPath}:streamGenerateContent?alt=sse`,
    geminiToolBody("copilot-gemini-stream"),
    "Copilot Gemini stream",
  );
  const payloads = frames.map((frame) => frame.payload).filter(Boolean);
  if (!payloads.some((payload) => geminiParts(payload).some((part) => part?.functionCall?.name === toolName))) {
    fail("Gemini stream is missing the forced lookup tool lifecycle");
  }
  const terminals = payloads.filter((payload) =>
    payload?.candidates?.some((candidate) => typeof candidate?.finishReason === "string"),
  );
  if (terminals.length !== 1) fail("Gemini stream must emit exactly one finishReason terminal");
  if (!payloads.some((payload) => payload?.usageMetadata)) {
    fail("Gemini stream is missing usageMetadata");
  }
  checks.gemini = "pass";
  console.log("[PASS] Copilot Gemini non-stream/stream tool, usage, terminal");
}

async function main() {
  const account = await validateControlPlane();
  await validateCatalogs();
  await validateQuota(account);
  await validateClaude();
  await validateCodex();
  await validateGemini();
  writeEvidence("pass", "live_verified", "bound Copilot models/quota and three surfaces passed");
  console.log(`[PASS] GitHub Copilot real-account gate complete (model=${selectedModel})`);
}

main().catch((error) => {
  const message = redact(error instanceof Error ? error.message : error);
  console.error(`[FAIL] ${message}`);
  try {
    writeEvidence("fail", "failed", "Copilot real-account gate failed; inspect sanitized console output");
  } catch (evidenceError) {
    console.error(`[FAIL] writing redacted evidence failed: ${safePreview(evidenceError, 300)}`);
  }
  process.exit(1);
});
