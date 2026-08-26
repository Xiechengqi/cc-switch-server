#!/usr/bin/env node

if (process.env.RUN_REAL !== "1") {
  throw new Error("set RUN_REAL=1 to run real Cursor acceptance");
}

const base = required("CC_SWITCH_SHARE_URL").replace(/\/$/, "");
const token = required("ROUTER_API_TOKEN");
const tokenHeader = process.env.ROUTER_API_TOKEN_HEADER || "Authorization";
const rail = required("CURSOR_RAIL");
if (!new Set(["apikey", "oauth"]).has(rail)) {
  throw new Error("CURSOR_RAIL must be apikey or oauth");
}
const model = process.env.CURSOR_TEST_MODEL || "composer-2.5";
const timeoutMs = Number(process.env.CURSOR_ACCEPTANCE_TIMEOUT_MS || 120000);
const results = [];

const tokens = await invoke("/v1/responses/input_tokens", {
  model,
  input: "Reply with exactly CURSOR_ACCEPTANCE_OK",
});
results.push(check("input_tokens", tokens, (body) =>
  body.object === "response.input_tokens" && body.estimated === true && body.input_tokens > 0));

const first = await invoke("/v1/responses", {
  model,
  store: true,
  input: "Reply with exactly CURSOR_ACCEPTANCE_OK",
});
results.push(check("responses_store", first, (body) =>
  body.object === "response" && typeof body.id === "string"));

if (first.body?.id) {
  const continued = await invoke("/v1/responses", {
    model,
    previous_response_id: first.body.id,
    input: "Reply with exactly CURSOR_CONTINUATION_OK",
  });
  results.push(check("previous_response", continued, (body) => body.object === "response"));
} else {
  results.push({ id: "previous_response", ok: false, status: first.status, reason: "missing response id" });
}

const compact = await invoke("/v1/responses/compact", {
  model,
  input: [
    { type: "message", role: "user", content: "We must preserve the single-account constraint." },
    { type: "message", role: "assistant", content: "Acknowledged." },
  ],
});
results.push(check("compact", compact, (body) =>
  body.object === "response.compaction" && body.output?.[0]?.type === "compaction"));

const tool = await invoke("/v1/responses", {
  model,
  input: "Call lookup with key cursor; do not answer in prose.",
  tools: [{
    type: "function",
    name: "lookup",
    description: "Look up one key",
    parameters: {
      type: "object",
      additionalProperties: false,
      properties: { key: { type: "string" } },
      required: ["key"],
    },
  }],
  tool_choice: { type: "function", function: { name: "lookup" } },
});
const call = tool.body?.output?.find((item) => item.type === "function_call");
results.push({
  id: "named_tool",
  ok: tool.status === 200 && call?.name === "lookup" && typeof call.call_id === "string",
  status: tool.status,
  reason: call?.name === "lookup" ? null : "lookup call missing",
});

if (tool.body?.id && call?.call_id) {
  const resumed = await invoke("/v1/responses", {
    model,
    previous_response_id: tool.body.id,
    input: [{ type: "function_call_output", call_id: call.call_id, output: "cursor-value" }],
  });
  results.push(check("tool_result_same_stream", resumed, (body) => body.object === "response"));
} else {
  results.push({ id: "tool_result_same_stream", ok: false, status: tool.status, reason: "tool context missing" });
}

const ok = results.every((result) => result.ok);
process.stdout.write(`${JSON.stringify({ ok, rail, model, verifiedAt: new Date().toISOString(), results }, null, 2)}\n`);
process.exit(ok ? 0 : 1);

async function invoke(path, body) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(`${base}${path}`, {
      method: "POST",
      headers: {
        ...authHeaders(tokenHeader, token),
        "content-type": "application/json",
        accept: "application/json",
      },
      body: JSON.stringify(body),
      signal: controller.signal,
    });
    const text = await readLimited(response, 2 * 1024 * 1024);
    let parsed = null;
    try { parsed = JSON.parse(text); } catch {}
    return { status: response.status, body: parsed };
  } catch (error) {
    return { status: 0, body: null, error: error?.name === "AbortError" ? "timeout" : "network_error" };
  } finally {
    clearTimeout(timer);
  }
}

function check(id, result, predicate) {
  const ok = result.status === 200 && result.body != null && predicate(result.body);
  return { id, ok, status: result.status, reason: ok ? null : result.error || "contract mismatch" };
}

function authHeaders(header, value) {
  if (/^authorization$/i.test(header)) return { authorization: `Bearer ${value}` };
  if (/^(x-api-key|x-goog-api-key)$/i.test(header)) return { [header]: value };
  throw new Error("unsupported ROUTER_API_TOKEN_HEADER");
}

async function readLimited(response, limit) {
  if (!response.body) return "";
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let size = 0;
  let output = "";
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > limit) throw new Error("response exceeded acceptance limit");
    output += decoder.decode(value, { stream: true });
  }
  return output + decoder.decode();
}

function required(name) {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`missing ${name}`);
  return value;
}
