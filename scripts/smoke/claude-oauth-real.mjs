#!/usr/bin/env node

const shareUrl = (process.env.CC_SWITCH_SHARE_URL || "").trim().replace(/\/+$/, "");
const routerToken = (process.env.ROUTER_API_TOKEN || "").trim();
const routerTokenHeader = (process.env.ROUTER_API_TOKEN_HEADER || "Authorization").trim();
const serverUrl = (process.env.SERVER_URL || "").trim().replace(/\/+$/, "");
const serverToken = (process.env.CC_SWITCH_SERVER_TOKEN || "").trim();
const max5xAccount = (process.env.CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT || "").trim();
const max20xAccount = (process.env.CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT || "").trim();
const model = (process.env.CC_SWITCH_CLAUDE_MODEL || "claude-sonnet-4-6").trim();
const timeoutMs = Math.max(
  1_000,
  Math.min(300_000, Number(process.env.CC_SWITCH_REAL_TIMEOUT_MS || 120_000)),
);

function fail(message) {
  throw new Error(message);
}

function redact(value) {
  let redacted = String(value);
  for (const secret of [routerToken, serverToken]) {
    if (secret) redacted = redacted.split(secret).join("[REDACTED]");
  }
  return redacted;
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

async function shareRequest(path, init = {}) {
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
  const { response, stopTimeout } = await shareRequest(path, {
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
  const { response, stopTimeout } = await shareRequest("/v1/messages", {
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

async function validateShareInference() {
  if (!shareUrl || !routerToken) {
    console.log(
      "[SKIP] Claude OAuth Share inference gate requires CC_SWITCH_SHARE_URL and ROUTER_API_TOKEN",
    );
    return false;
  }

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
  console.log(`[PASS] Claude OAuth Share inference gate complete (model=${model})`);
  return true;
}

async function requireServerJson(path, label) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(`${serverUrl}${path}`, {
      headers: { authorization: `Bearer ${serverToken}` },
      signal: controller.signal,
    });
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
    clearTimeout(timer);
  }
}

let accountListPromise;

async function listClaudeAccounts() {
  accountListPromise ||= requireServerJson("/api/accounts", "account list");
  const payload = await accountListPromise;
  if (!payload?.ok || !Array.isArray(payload.accounts)) {
    fail("account list does not satisfy the public API contract");
  }
  return payload.accounts.filter(
    (account) => account?.providerType === "claude_oauth",
  );
}

function resolveClaudeAccount(accounts, selector, label) {
  const normalizedEmail = selector.toLowerCase();
  const matches = accounts.filter(
    (account) =>
      account?.id === selector ||
      (typeof account?.email === "string" &&
        account.email.trim().toLowerCase() === normalizedEmail),
  );
  if (matches.length === 0) {
    fail(`${label} selector did not match a Claude OAuth account`);
  }
  if (matches.length > 1) {
    fail(`${label} selector matched multiple Claude OAuth accounts`);
  }
  return matches[0];
}

function requireCanonicalPlan(payload, expectedPlanType, expectedLabel, label) {
  const account = payload?.account;
  const quota = payload?.quota;
  const subscription = quota?.extraUsage?.subscription;
  const evidence = quota?.extraUsage?.subscriptionEvidence;
  if (!payload?.ok || !quota?.success) {
    fail(`${label} quota refresh did not return a successful public quota`);
  }
  if (account?.providerType !== "claude_oauth") {
    fail(`${label} quota response is not bound to a Claude OAuth account`);
  }
  if (
    account.subscriptionLevel !== expectedLabel ||
    quota.credentialMessage !== expectedLabel
  ) {
    fail(`${label} did not publish the expected subscription label`);
  }
  if (
    subscription?.planType !== expectedPlanType ||
    subscription?.planLabel !== expectedLabel
  ) {
    fail(`${label} did not publish the expected canonical subscription metadata`);
  }
  if (
    typeof subscription.planSource !== "string" ||
    !subscription.planSource ||
    typeof subscription.planStale !== "boolean" ||
    !Number.isFinite(subscription.planObservedAt) ||
    typeof evidence?.source !== "string" ||
    !evidence.source ||
    typeof evidence.stale !== "boolean" ||
    !Number.isFinite(evidence.observedAt) ||
    typeof evidence.conflict !== "boolean"
  ) {
    fail(`${label} did not publish complete subscription evidence metadata`);
  }
  if (
    subscription.planSource !== evidence.source ||
    subscription.planStale !== evidence.stale ||
    subscription.planObservedAt !== evidence.observedAt
  ) {
    fail(`${label} subscription and evidence provenance disagree`);
  }
  if (evidence.conflict) {
    fail(`${label} returned conflicting live subscription evidence`);
  }
  if (subscription.planStale || evidence.stale) {
    fail(`${label} resolved only from cached subscription evidence`);
  }
  return evidence;
}

async function validateMaxPlan({ selector, selectorEnv, expectedPlanType, expectedLabel }) {
  if (!selector) {
    console.log(`[SKIP] ${expectedLabel} plan gate requires ${selectorEnv}`);
    return false;
  }
  if (!serverUrl || !serverToken) {
    console.log(
      `[SKIP] ${expectedLabel} plan gate requires SERVER_URL and CC_SWITCH_SERVER_TOKEN`,
    );
    return false;
  }

  const account = resolveClaudeAccount(
    await listClaudeAccounts(),
    selector,
    expectedLabel,
  );
  const payload = await requireServerJson(
    `/api/accounts/${encodeURIComponent(account.id)}/quota?refresh=true&force=true`,
    `${expectedLabel} quota refresh`,
  );
  const evidence = requireCanonicalPlan(
    payload,
    expectedPlanType,
    expectedLabel,
    expectedLabel,
  );
  console.log(
    `[PASS] ${expectedLabel} plan contract (planType=${expectedPlanType}, source=${evidence.source}, stale=${evidence.stale})`,
  );
  return true;
}

async function runGate(label, operation, failures) {
  try {
    return await operation();
  } catch (error) {
    failures.push(
      `${label}: ${error instanceof Error ? error.message : String(error)}`,
    );
    return false;
  }
}

async function main() {
  const failures = [];
  const results = [];
  results.push(
    await runGate("Share inference", validateShareInference, failures),
  );
  results.push(
    await runGate(
      "Max 5x plan",
      () =>
        validateMaxPlan({
          selector: max5xAccount,
          selectorEnv: "CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT",
          expectedPlanType: "claude_max_5x",
          expectedLabel: "Claude Max 5x",
        }),
      failures,
    ),
  );
  results.push(
    await runGate(
      "Max 20x plan",
      () =>
        validateMaxPlan({
          selector: max20xAccount,
          selectorEnv: "CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT",
          expectedPlanType: "claude_max_20x",
          expectedLabel: "Claude Max 20x",
        }),
      failures,
    ),
  );
  if (failures.length > 0) {
    fail(failures.join(" | "));
  }
  const passed = results.filter(Boolean).length;
  if (passed > 0) {
    console.log(`[PASS] Claude OAuth real-account gates complete (${passed}/3 passed)`);
  }
}

main().catch((error) => {
  console.error(`[FAIL] ${redact(error instanceof Error ? error.message : error)}`);
  process.exit(1);
});
