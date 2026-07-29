#!/usr/bin/env node

const serverBase = requiredEnv("CURSOR_SERVER_URL");
const oracleBase = requiredEnv("CURSOR_SDK_ORACLE_URL");
const serverToken = requiredEnv("CURSOR_SERVER_TOKEN");
const oracleToken = requiredEnv("CURSOR_SDK_ORACLE_TOKEN");
const model = process.env.CURSOR_TEST_MODEL || "composer-2.5";
const timeoutMs = numberEnv("CURSOR_DIFFERENTIAL_TIMEOUT_MS", 120_000);

const fixtures = [
  {
    id: "non_stream_text",
    body: {
      model,
      stream: false,
      messages: [{ role: "user", content: "Reply with exactly CURSOR_DIFFERENTIAL_OK" }],
    },
  },
  {
    id: "stream_text",
    body: {
      model,
      stream: true,
      messages: [{ role: "user", content: "Reply with exactly CURSOR_DIFFERENTIAL_OK" }],
    },
  },
  {
    id: "declared_tool",
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
];

const results = [];
for (const fixture of fixtures) {
  const [server, oracle] = await Promise.all([
    invoke(serverBase, serverToken, fixture.body),
    invoke(oracleBase, oracleToken, fixture.body),
  ]);
  const comparison = compareSemantics(server, oracle, fixture.id);
  results.push({ id: fixture.id, server, oracle, comparison });
}

const ok = results.every((result) => result.comparison.ok);
process.stdout.write(`${JSON.stringify({ ok, model, results }, null, 2)}\n`);
process.exit(ok ? 0 : 1);

async function invoke(base, token, body) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(`${base.replace(/\/$/, "")}/v1/chat/completions`, {
      method: "POST",
      headers: {
        authorization: `Bearer ${token}`,
        "content-type": "application/json",
        accept: body.stream ? "text/event-stream" : "application/json",
      },
      body: JSON.stringify(body),
      signal: controller.signal,
    });
    const text = await readLimited(response, 2 * 1024 * 1024);
    return summarize(response.status, text, body.stream);
  } catch (error) {
    return {
      status: 0,
      streaming: Boolean(body.stream),
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

function compareSemantics(server, oracle, id) {
  const reasons = [];
  if (Math.trunc(server.status / 100) !== Math.trunc(oracle.status / 100)) {
    reasons.push("HTTP status classes differ");
  }
  if (server.error || oracle.error) reasons.push("one or both requests failed");
  if (id === "declared_tool") {
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
