#!/usr/bin/env node

const shareBase = requiredEnv("CC_SWITCH_SHARE_URL");
const oracleBase = requiredEnv("CURSOR_SDK_ORACLE_URL");
const routerToken = requiredEnv("ROUTER_API_TOKEN");
const routerTokenHeader = process.env.ROUTER_API_TOKEN_HEADER || "Authorization";
const oracleToken = requiredEnv("CURSOR_SDK_ORACLE_TOKEN");
const model = process.env.CURSOR_TEST_MODEL || "composer-2.5";
const timeoutMs = numberEnv("CURSOR_DIFFERENTIAL_TIMEOUT_MS", 120_000);
const textPrompt = "Reply with exactly CURSOR_DIFFERENTIAL_OK";

function chatBody(stream, prompt = textPrompt) {
  return {
    model,
    stream,
    messages: [{ role: "user", content: prompt }],
  };
}

const fixtures = [
  {
    id: "anthropic_stream_text",
    expectation: "text",
    server: {
      path: "/v1/messages",
      stream: true,
      headers: { "anthropic-version": "2023-06-01" },
      body: {
        model,
        max_tokens: 128,
        stream: true,
        messages: [{ role: "user", content: textPrompt }],
      },
    },
    oracle: { path: "/v1/chat/completions", stream: true, body: chatBody(true) },
  },
  {
    id: "chat_stream_text",
    expectation: "text",
    server: { path: "/v1/chat/completions", stream: true, body: chatBody(true) },
    oracle: { path: "/v1/chat/completions", stream: true, body: chatBody(true) },
  },
  {
    id: "responses_stream_text",
    expectation: "text",
    server: {
      path: "/v1/responses",
      stream: true,
      body: { model, stream: true, input: textPrompt },
    },
    oracle: {
      path: "/v1/responses",
      stream: true,
      body: { model, stream: true, input: textPrompt },
    },
  },
  {
    id: "gemini_non_stream_text",
    expectation: "text",
    server: {
      path: `/v1beta/models/${encodeURIComponent(model)}:generateContent`,
      stream: false,
      body: {
        contents: [{ role: "user", parts: [{ text: textPrompt }] }],
        generationConfig: { maxOutputTokens: 128 },
      },
    },
    oracle: { path: "/v1/chat/completions", stream: false, body: chatBody(false) },
  },
  {
    id: "chat_declared_tool",
    expectation: "tool",
    server: {
      path: "/v1/chat/completions",
      stream: false,
      body: {
        model,
        stream: false,
        messages: [{ role: "user", content: "Call the lookup tool with key cursor" }],
        tools: [
          {
            type: "function",
            function: {
              name: "lookup",
              description: "Look up a key",
              parameters: {
                type: "object",
                properties: { key: { type: "string" } },
                required: ["key"],
              },
            },
          },
        ],
        tool_choice: { type: "function", function: { name: "lookup" } },
      },
    },
    oracle: {
      path: "/v1/chat/completions",
      stream: false,
      body: {
        ...chatBody(false, "Call the lookup tool with key cursor"),
        tools: [
          {
            type: "function",
            function: {
              name: "lookup",
              description: "Look up a key",
              parameters: {
                type: "object",
                properties: { key: { type: "string" } },
                required: ["key"],
              },
            },
          },
        ],
        tool_choice: { type: "function", function: { name: "lookup" } },
      },
    },
  },
];

const results = [];
for (const fixture of fixtures) {
  const [server, oracle] = await Promise.all([
    invoke(shareBase, routerAuthHeaders(routerTokenHeader, routerToken), fixture.server),
    invoke(oracleBase, { authorization: `Bearer ${oracleToken}` }, fixture.oracle),
  ]);
  const comparison = compareSemantics(server, oracle, fixture.expectation);
  results.push({ id: fixture.id, server, oracle, comparison });
}

const ok = results.every((result) => result.comparison.ok);
process.stdout.write(`${JSON.stringify({ ok, model, results }, null, 2)}\n`);
process.exit(ok ? 0 : 1);

async function invoke(base, authHeaders, request) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(`${base.replace(/\/$/, "")}${request.path}`, {
      method: "POST",
      headers: {
        ...authHeaders,
        ...(request.headers || {}),
        "content-type": "application/json",
        accept: request.stream ? "text/event-stream" : "application/json",
      },
      body: JSON.stringify(request.body),
      signal: controller.signal,
    });
    const text = await readLimited(response, 2 * 1024 * 1024);
    return summarize(response.status, text, request.stream);
  } catch (error) {
    return {
      status: 0,
      streaming: Boolean(request.stream),
      hasContent: false,
      hasDone: false,
      toolNames: [],
      finishReasons: [],
      error: error?.name === "AbortError" ? "timeout" : String(error),
    };
  } finally {
    clearTimeout(timeout);
  }
}

function routerAuthHeaders(header, token) {
  if (/^authorization$/i.test(header)) {
    return { authorization: `Bearer ${token}` };
  }
  if (/^(x-api-key|x-goog-api-key)$/i.test(header)) {
    return { [header]: token };
  }
  throw new Error(`unsupported ROUTER_API_TOKEN_HEADER: ${header}`);
}

function summarize(status, text, streaming) {
  const toolNames = unique([
    ...matches(text, /"name"\s*:\s*"([A-Za-z0-9_.-]+)"/g),
    ...matches(text, /"function"\s*:\s*\{[^}]*"name"\s*:\s*"([A-Za-z0-9_.-]+)"/g),
  ]).filter((name) => name === "lookup");
  const finishReasons = unique(matches(text, /"finish_reason"\s*:\s*"([^"]+)"/g));
  return {
    status,
    streaming,
    hasContent: /CURSOR_DIFFERENTIAL_OK|"content"\s*:\s*"[^"\s]/.test(text),
    hasDone: !streaming || /\[DONE\]|message_stop|response\.completed|finish_reason/.test(text),
    toolNames,
    finishReasons,
    error: null,
  };
}

function compareSemantics(server, oracle, expectation) {
  const reasons = [];
  if (Math.trunc(server.status / 100) !== Math.trunc(oracle.status / 100)) {
    reasons.push("HTTP status classes differ");
  }
  if (server.error || oracle.error) reasons.push("one or both requests failed");
  if (expectation === "tool") {
    if (!server.toolNames.includes("lookup") || !oracle.toolNames.includes("lookup")) {
      reasons.push("lookup tool call was not emitted by both implementations");
    }
  } else {
    if (!server.hasContent || !oracle.hasContent) reasons.push("one response had no content");
    if (!server.hasDone || !oracle.hasDone) reasons.push("one stream had no terminal event");
  }
  return { ok: reasons.length === 0, reasons };
}

async function readLimited(response, maxBytes) {
  if (!response.body) return "";
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let size = 0;
  let text = "";
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.length;
    if (size > maxBytes) throw new Error("response exceeded 2 MiB");
    text += decoder.decode(value, { stream: true });
  }
  return text + decoder.decode();
}

function matches(text, pattern) {
  return [...text.matchAll(pattern)].map((match) => match[1]);
}

function unique(values) {
  return [...new Set(values)].sort();
}

function requiredEnv(name) {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required`);
  return value;
}

function numberEnv(name, fallback) {
  const value = Number(process.env[name] || fallback);
  if (!Number.isFinite(value) || value <= 0) throw new Error(`${name} must be positive`);
  return value;
}
