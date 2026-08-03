#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const baseUrl = (process.env.CC_SWITCH_BASE_URL || "").trim().replace(/\/+$/, "");
const inferenceToken = (process.env.CC_SWITCH_INFERENCE_TOKEN || "").trim();
const routeKey = (process.env.CC_SWITCH_GROK_ROUTE_KEY || "").trim();
const model = (process.env.CC_SWITCH_GROK_MODEL || "grok-4.5").trim();
const mediaSmoke = (process.env.CC_SWITCH_GROK_MEDIA_SMOKE || "0").trim() === "1";
const evidenceFile = (process.env.EVIDENCE_FILE || "").trim();
const configuredTimeoutMs = Number(process.env.CC_SWITCH_REAL_TIMEOUT_MS || 120_000);
const timeoutMs = Number.isFinite(configuredTimeoutMs)
  ? Math.max(1_000, Math.min(300_000, Math.trunc(configuredTimeoutMs)))
  : 120_000;
const sessionId = "cc-switch-grok-oauth-real-v1";
const turnIndex = "7";
const checks = {
  ready: "not-run",
  models: "not-run",
  json: "not-run",
  stream: "not-run",
  media: mediaSmoke ? "not-run" : "disabled",
};

function isUsable(value) {
  const trimmed = String(value || "").trim();
  return Boolean(trimmed) && !trimmed.includes("<") && !trimmed.includes(">");
}

function fail(message) {
  throw new Error(message);
}

function redact(value) {
  let text = String(value);
  const secrets = [
    inferenceToken,
    process.env.GROK_OAUTH_REFRESH_TOKEN_FIXTURE,
    process.env.GROK_OAUTH_AUTH_JSON_FIXTURE,
  ].filter(isUsable);
  for (const secret of secrets) {
    text = text.split(secret).join("[REDACTED]");
  }
  return text
    .replace(/Bearer\s+[^\s,"'}]+/gi, "Bearer [REDACTED]")
    .replace(
      /("(?:access|refresh|id)_token"\s*:\s*")[^"]+/gi,
      "$1[REDACTED]",
    )
    .replace(/\beyJ[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{8,}\b/g, "[REDACTED_JWT]");
}

function safePreview(value, limit = 500) {
  return redact(value).replace(/[\r\n\t]+/g, " ").slice(0, limit);
}

function writeEvidence(status, notes = "") {
  if (!evidenceFile) return;
  const writer = fileURLToPath(new URL("./write-acceptance-evidence.mjs", import.meta.url));
  execFileSync(process.execPath, [writer, "--out", evidenceFile], {
    stdio: "inherit",
    env: {
      ...process.env,
      SERVER_URL: isUsable(baseUrl) ? baseUrl : "",
      EVIDENCE_STAGE: "grok-oauth-real",
      EVIDENCE_STATUS: status,
      EVIDENCE_TARGET: isUsable(baseUrl) ? baseUrl : "",
      EVIDENCE_SOURCE: "scripts/smoke/grok-oauth-real.mjs",
      EVIDENCE_APP: "codex",
      EVIDENCE_PROVIDER: isUsable(routeKey) ? routeKey : "",
      EVIDENCE_PROVIDER_TYPE: "grok_oauth",
      EVIDENCE_NOTES: notes,
      PROBE_MODEL: model,
      GROK_GATE_STATUS: status,
      GROK_READY_STATUS: checks.ready,
      GROK_MODELS_STATUS: checks.models,
      GROK_JSON_STATUS: checks.json,
      GROK_STREAM_STATUS: checks.stream,
      GROK_MEDIA_STATUS: checks.media,
    },
  });
}

const missingInputs = [
  ["CC_SWITCH_BASE_URL", baseUrl],
  ["CC_SWITCH_INFERENCE_TOKEN", inferenceToken],
  ["CC_SWITCH_GROK_ROUTE_KEY", routeKey],
]
  .filter(([, value]) => !isUsable(value))
  .map(([name]) => name);

if (missingInputs.length > 0) {
  console.log(
    `[SKIP] Grok OAuth real-account gate requires non-placeholder ${missingInputs.join(", ")}`,
  );
  writeEvidence("blocked-inputs", `missing ${missingInputs.join(", ")}`);
  process.exit(0);
}

if (!isUsable(model)) {
  console.log("[SKIP] Grok OAuth real-account gate requires a non-placeholder model");
  writeEvidence("blocked-inputs", "missing CC_SWITCH_GROK_MODEL");
  process.exit(0);
}

function commonHeaders({ inference = false } = {}) {
  const headers = new Headers({
    accept: "application/json",
    "x-api-key": inferenceToken,
  });
  if (inference) {
    headers.set("x-session-id", sessionId);
    headers.set("x-grok-turn-idx", turnIndex);
  }
  return headers;
}

async function request(path, init = {}, options = {}) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  const headers = commonHeaders(options);
  for (const [name, value] of new Headers(init.headers || {})) {
    headers.set(name, value);
  }
  if (init.body !== undefined) {
    headers.set("content-type", "application/json");
  }
  const routedPath =
    path === "/ready"
      ? path
      : `/r/${encodeURIComponent(routeKey)}${path}`;
  try {
    const response = await fetch(`${baseUrl}${routedPath}`, {
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

async function responseText(response, label) {
  const text = await response.text();
  if (!response.ok) {
    fail(`${label} returned HTTP ${response.status}: ${safePreview(text)}`);
  }
  return text;
}

async function requireJson(path, init, label, options = {}) {
  const { response, stopTimeout } = await request(path, init, options);
  try {
    const text = await responseText(response, label);
    try {
      return JSON.parse(text);
    } catch (error) {
      fail(`${label} returned invalid JSON: ${error.message}`);
    }
  } finally {
    stopTimeout();
  }
}

function isNonNegativeInteger(value) {
  return Number.isInteger(value) && value >= 0;
}

function validateModels(catalog) {
  if (catalog?.object !== "list" || !Array.isArray(catalog.data)) {
    fail("models response does not satisfy the OpenAI list contract");
  }
  if (!catalog.data.some((entry) => entry?.id === model && entry?.object === "model")) {
    fail(`models response does not contain configured model ${model}`);
  }
  if (typeof catalog.source !== "string" || catalog.source.trim() === "") {
    fail("models response is missing catalog source metadata");
  }
  if (typeof catalog.stale !== "boolean") {
    fail("models response is missing stale metadata");
  }
  if (
    catalog.fetchedAtMs !== undefined &&
    (!isNonNegativeInteger(catalog.fetchedAtMs) || catalog.fetchedAtMs === 0)
  ) {
    fail("models response has invalid fetchedAtMs metadata");
  }
  if (!catalog.stale && !isNonNegativeInteger(catalog.fetchedAtMs)) {
    fail("fresh models response is missing fetchedAtMs metadata");
  }
}

function validateJsonResponse(response) {
  if (
    response?.object !== "response" ||
    typeof response.id !== "string" ||
    response.id.trim() === "" ||
    response.status !== "completed" ||
    !Array.isArray(response.output) ||
    response.output.length === 0
  ) {
    fail("non-stream Responses result does not satisfy the completed response contract");
  }
  for (const key of ["input_tokens", "output_tokens", "total_tokens"]) {
    if (response.usage?.[key] !== undefined && !isNonNegativeInteger(response.usage[key])) {
      fail(`non-stream Responses usage.${key} is invalid`);
    }
  }
}

function nextSseBoundary(buffer) {
  const match = /\r\n\r\n|\n\n|\r\r/.exec(buffer);
  return match ? { index: match.index, length: match[0].length } : null;
}

function parseSseFrame(frame) {
  let eventName = "";
  const data = [];
  for (const line of frame.split(/\r\n|\r|\n/)) {
    if (!line || line.startsWith(":")) continue;
    if (line.startsWith("event:")) {
      eventName = line.slice(6).replace(/^ /, "");
    } else if (line.startsWith("data:")) {
      data.push(line.slice(5).replace(/^ /, ""));
    }
  }
  if (data.length === 0) return null;
  const joined = data.join("\n");
  if (joined === "[DONE]") return { type: "[DONE]" };
  let payload;
  try {
    payload = JSON.parse(joined);
  } catch (error) {
    fail(`stream emitted invalid SSE JSON: ${error.message}`);
  }
  if (!payload || typeof payload.type !== "string") {
    fail("stream SSE payload is missing type");
  }
  if (eventName && eventName !== payload.type) {
    fail(`stream event mismatch: event=${eventName}, data.type=${payload.type}`);
  }
  return payload;
}

async function validateStream() {
  const { response, stopTimeout } = await request(
    "/v1/responses",
    {
      method: "POST",
      headers: { accept: "text/event-stream" },
      body: JSON.stringify({
        model,
        input: "Reply with exactly: grok-oauth-stream-ok",
        max_output_tokens: 32,
        stream: true,
        store: false,
        metadata: { session_id: sessionId },
      }),
    },
    { inference: true },
  );
  try {
    if (!response.ok) {
      fail(
        `streaming Responses returned HTTP ${response.status}: ${safePreview(await response.text())}`,
      );
    }
    const contentType = response.headers.get("content-type") || "";
    if (!contentType.toLowerCase().includes("text/event-stream")) {
      fail(`streaming Responses returned unexpected content-type: ${contentType}`);
    }
    if (!response.body) fail("streaming Responses returned no body");

    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    let totalBytes = 0;
    let sawEvent = false;
    let sawBusiness = false;
    let sawCompleted = false;

    while (!sawCompleted) {
      const { done, value } = await reader.read();
      totalBytes += value?.byteLength || 0;
      if (totalBytes > 8 * 1024 * 1024) fail("stream exceeded the 8 MiB smoke bound");
      buffer += decoder.decode(value || new Uint8Array(), { stream: !done });
      while (true) {
        const boundary = nextSseBoundary(buffer);
        if (!boundary) break;
        const frame = buffer.slice(0, boundary.index);
        buffer = buffer.slice(boundary.index + boundary.length);
        const payload = parseSseFrame(frame);
        if (!payload || payload.type === "[DONE]") continue;
        sawEvent = true;
        if (payload.type === "error" || payload.type === "response.failed") {
          const detail = payload.error?.message || payload.response?.error?.message || payload.type;
          fail(`stream returned provider error: ${safePreview(detail, 300)}`);
        }
        if (payload.type === "response.incomplete") {
          fail("stream ended with response.incomplete");
        }
        if (
          payload.type.startsWith("response.output_") ||
          payload.type.startsWith("response.content_part.")
        ) {
          sawBusiness = true;
        }
        if (payload.type === "response.completed") {
          if (payload.response?.status && payload.response.status !== "completed") {
            fail(`response.completed carried status ${payload.response.status}`);
          }
          sawCompleted = true;
        }
      }
      if (done) break;
    }
    if (sawCompleted) await reader.cancel();
    if (!sawEvent) fail("stream ended without SSE events");
    if (!sawBusiness) fail("stream ended without business output");
    if (!sawCompleted) fail("stream ended before response.completed");
  } finally {
    stopTimeout();
  }
}

async function validateMedia() {
  const image = await requireJson(
    "/v1/images/generations",
    {
      method: "POST",
      body: JSON.stringify({
        model,
        prompt: "A small black square centered on a plain white background.",
        n: 1,
      }),
    },
    "image generation",
    { inference: true },
  );
  if (
    !Array.isArray(image?.data) ||
    image.data.length === 0 ||
    !image.data.some(
      (entry) =>
        (typeof entry?.url === "string" && entry.url.length > 0) ||
        (typeof entry?.b64_json === "string" && entry.b64_json.length > 0),
    )
  ) {
    fail("image generation response is missing url or b64_json output");
  }
}

async function main() {
  const { response: ready, stopTimeout } = await request("/ready");
  try {
    if (!ready.ok) {
      fail(`/ready returned HTTP ${ready.status}: ${safePreview(await ready.text())}`);
    }
  } finally {
    stopTimeout();
  }
  checks.ready = "pass";
  console.log("[PASS] server readiness");

  const models = await requireJson(
    "/v1/models",
    { method: "GET" },
    "models",
  );
  validateModels(models);
  checks.models = "pass";
  console.log(`[PASS] model catalog metadata (source=${models.source}, stale=${models.stale})`);

  const result = await requireJson(
    "/v1/responses",
    {
      method: "POST",
      body: JSON.stringify({
        model,
        input: "Reply with exactly: grok-oauth-json-ok",
        max_output_tokens: 32,
        stream: false,
        store: false,
        metadata: { session_id: sessionId },
      }),
    },
    "non-stream Responses",
    { inference: true },
  );
  validateJsonResponse(result);
  checks.json = "pass";
  console.log("[PASS] non-stream Responses contract");

  await validateStream();
  checks.stream = "pass";
  console.log("[PASS] Responses SSE lifecycle and terminal contract");

  if (mediaSmoke) {
    await validateMedia();
    checks.media = "pass";
    console.log("[PASS] Grok image generation contract");
  } else {
    console.log("[SKIP] Grok media smoke is disabled (CC_SWITCH_GROK_MEDIA_SMOKE=0)");
  }

  writeEvidence("pass", "real xAI normal-path smoke completed");
  console.log(`[PASS] Grok OAuth real-account gate complete (model=${model})`);
}

main().catch((error) => {
  const message = redact(error instanceof Error ? error.message : error);
  console.error(`[FAIL] ${message}`);
  try {
    writeEvidence("fail", "real xAI smoke failed; inspect sanitized console output");
  } catch (evidenceError) {
    console.error(
      `[FAIL] writing redacted evidence failed: ${safePreview(
        evidenceError instanceof Error ? evidenceError.message : evidenceError,
        300,
      )}`,
    );
  }
  process.exit(1);
});
