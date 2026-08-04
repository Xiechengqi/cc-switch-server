#!/usr/bin/env node

const shareUrl = (process.env.CC_SWITCH_SHARE_URL || "").trim().replace(/\/+$/, "");
const routerToken = (process.env.ROUTER_API_TOKEN || "").trim();
const routerTokenHeader = (process.env.ROUTER_API_TOKEN_HEADER || "Authorization").trim();
const model = (process.env.CC_SWITCH_CLAUDE_MODEL || "claude-sonnet-4-6").trim();
const timeoutMs = Math.max(
  1_000,
  Math.min(300_000, Number(process.env.CC_SWITCH_REAL_TIMEOUT_MS || 120_000)),
);

if (!shareUrl || !routerToken) {
  console.log(
    "[SKIP] Claude OAuth real-account gate requires CC_SWITCH_SHARE_URL and ROUTER_API_TOKEN",
  );
  process.exit(0);
}

function fail(message) {
  throw new Error(message);
}

function redact(value) {
  return String(value).split(routerToken).join("[REDACTED]");
}

function applyRouterAuth(headers) {
  if (/^authorization$/i.test(routerTokenHeader)) {
    headers.set("authorization", `Bearer ${routerToken}`);
    return;
  }
  if (/^(x-api-key|x-goog-api-key)$/i.test(routerTokenHeader)) {
    headers.set(routerTokenHeader, routerToken);
    return;
  }
  fail(`unsupported ROUTER_API_TOKEN_HEADER: ${routerTokenHeader}`);
}

function isNonNegativeInteger(value) {
  return Number.isInteger(value) && value >= 0;
}

async function request(path, init = {}) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  const headers = new Headers(init.headers || {});
  if (init.body !== undefined) {
    headers.set("content-type", "application/json");
  }
  applyRouterAuth(headers);
  try {
    const response = await fetch(`${shareUrl}${path}`, {
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

async function requireJson(path, body, label) {
  const { response, stopTimeout } = await request(path, {
    method: "POST",
    body: JSON.stringify(body),
  });
  try {
    const text = await response.text();
    if (!response.ok) {
      fail(`${label} returned HTTP ${response.status}: ${text.slice(0, 500)}`);
    }
    try {
      return JSON.parse(text);
    } catch (error) {
      fail(`${label} returned invalid JSON: ${error.message}`);
    }
  } finally {
    stopTimeout();
  }
}

function nextSseBoundary(buffer) {
  const match = /\r\n\r\n|\n\n|\r\r/.exec(buffer);
  return match ? { index: match.index, length: match[0].length } : null;
}

function parseSseEvent(frame) {
  let declaredEvent = "";
  const data = [];
  for (const line of frame.split(/\r\n|\r|\n/)) {
    if (!line || line.startsWith(":")) continue;
    if (line.startsWith("event:")) {
      declaredEvent = line.slice(6).replace(/^ /, "");
    } else if (line.startsWith("data:")) {
      data.push(line.slice(5).replace(/^ /, ""));
    }
  }
  if (data.length === 0) return null;
  let payload;
  try {
    payload = JSON.parse(data.join("\n"));
  } catch (error) {
    fail(`stream emitted invalid SSE JSON: ${error.message}`);
  }
  if (!payload || typeof payload.type !== "string") {
    fail("stream SSE payload is missing type");
  }
  if (declaredEvent && declaredEvent !== payload.type) {
    fail(`stream event mismatch: event=${declaredEvent}, data.type=${payload.type}`);
  }
  return payload;
}

async function validateStream() {
  const { response, stopTimeout } = await request("/v1/messages", {
    method: "POST",
    body: JSON.stringify({
      model,
      max_tokens: 32,
      stream: true,
      messages: [{ role: "user", content: "Reply with exactly: oauth-stream-ok" }],
    }),
  });
  try {
    if (!response.ok) {
      const text = await response.text();
      fail(`streaming messages returned HTTP ${response.status}: ${text.slice(0, 500)}`);
    }
    const contentType = response.headers.get("content-type") || "";
    if (!contentType.toLowerCase().includes("text/event-stream")) {
      fail(`streaming messages returned unexpected content-type: ${contentType}`);
    }
    if (!response.body) fail("streaming messages returned no body");

    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    let sawStart = false;
    let sawBusiness = false;
    let sawStop = false;

    while (!sawStop) {
      const { done, value } = await reader.read();
      buffer += decoder.decode(value || new Uint8Array(), { stream: !done });
      while (true) {
        const boundary = nextSseBoundary(buffer);
        if (!boundary) break;
        const frame = buffer.slice(0, boundary.index);
        buffer = buffer.slice(boundary.index + boundary.length);
        const payload = parseSseEvent(frame);
        if (!payload) continue;
        if (payload.type === "error") {
          const detail = payload.error?.message || payload.error?.type || "unknown error";
          fail(`stream returned Anthropic error: ${detail}`);
        }
        if (payload.type === "message_start") {
          if (sawStart) fail("stream emitted duplicate message_start");
          if (payload.message?.type !== "message") {
            fail("message_start is missing an Anthropic message");
          }
          sawStart = true;
        } else if (
          [
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
          ].includes(payload.type)
        ) {
          if (!sawStart) fail(`${payload.type} arrived before message_start`);
          sawBusiness = true;
        } else if (payload.type === "message_stop") {
          if (!sawStart) fail("message_stop arrived before message_start");
          sawStop = true;
        } else if (payload.type !== "ping") {
          fail(`stream emitted unknown event type: ${payload.type}`);
        }
      }
      if (done) break;
    }
    if (sawStop) await reader.cancel();
    if (!sawStart) fail("stream ended before message_start");
    if (!sawBusiness) fail("stream ended without business output");
    if (!sawStop) fail("stream ended before message_stop");
  } finally {
    stopTimeout();
  }
}

async function main() {
  const count = await requireJson(
    "/v1/messages/count_tokens",
    {
      model,
      messages: [{ role: "user", content: "Count this OAuth smoke request." }],
    },
    "count_tokens",
  );
  if (!isNonNegativeInteger(count.input_tokens)) {
    fail("count_tokens response is missing non-negative input_tokens");
  }
  console.log("[PASS] Router Share ingress");
  console.log("[PASS] count_tokens contract");

  const message = await requireJson(
    "/v1/messages",
    {
      model,
      max_tokens: 32,
      stream: false,
      messages: [{ role: "user", content: "Reply with exactly: oauth-json-ok" }],
    },
    "non-stream messages",
  );
  if (
    message.type !== "message" ||
    typeof message.id !== "string" ||
    message.role !== "assistant" ||
    !Array.isArray(message.content) ||
    !isNonNegativeInteger(message.usage?.input_tokens) ||
    !isNonNegativeInteger(message.usage?.output_tokens)
  ) {
    fail("non-stream messages response does not satisfy the Anthropic contract");
  }
  console.log("[PASS] non-stream messages contract");

  await validateStream();
  console.log("[PASS] streaming lifecycle and terminal contract");
  console.log(`[PASS] Claude OAuth real-account gate complete (model=${model})`);
}

main().catch((error) => {
  console.error(`[FAIL] ${redact(error instanceof Error ? error.message : error)}`);
  process.exit(1);
});
