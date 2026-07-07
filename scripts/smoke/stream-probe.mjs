#!/usr/bin/env node

const args = process.argv.slice(2);
const headers = {};
let url = "";
let body = "";
let timeoutMs = 60_000;
let maxBytes = 128 * 1024;
let requireDone = false;
let requireUsage = false;

for (let index = 0; index < args.length; index += 1) {
  const arg = args[index];
  const next = () => {
    index += 1;
    if (index >= args.length) {
      throw new Error(`${arg} requires a value`);
    }
    return args[index];
  };

  if (arg === "--url") {
    url = next();
  } else if (arg === "--body") {
    body = next();
  } else if (arg === "--header") {
    const header = next();
    const separator = header.indexOf(":");
    if (separator <= 0) {
      throw new Error(`invalid header: ${header}`);
    }
    headers[header.slice(0, separator).trim()] = header.slice(separator + 1).trim();
  } else if (arg === "--timeout-ms") {
    timeoutMs = Number(next());
  } else if (arg === "--max-bytes") {
    maxBytes = Number(next());
  } else if (arg === "--require-done") {
    requireDone = true;
  } else if (arg === "--require-usage") {
    requireUsage = true;
  } else {
    throw new Error(`unknown argument: ${arg}`);
  }
}

if (!url) {
  throw new Error("--url is required");
}
if (!body) {
  throw new Error("--body is required");
}
if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
  throw new Error("--timeout-ms must be positive");
}
if (!Number.isFinite(maxBytes) || maxBytes <= 0) {
  throw new Error("--max-bytes must be positive");
}

const started = Date.now();
const controller = new AbortController();
const timeout = setTimeout(() => controller.abort(), timeoutMs);
const decoder = new TextDecoder();
const summary = {
  ok: false,
  status: 0,
  chunks: 0,
  bytes: 0,
  firstChunkMs: null,
  doneEvent: false,
  usageSeen: false,
  capped: false,
  durationMs: 0,
  preview: "",
  error: null,
};

try {
  const response = await fetch(url, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Accept: "text/event-stream, application/json, */*",
      ...headers,
    },
    body,
    signal: controller.signal,
  });
  summary.status = response.status;
  if (!response.body) {
    throw new Error("response body is not readable");
  }

  const reader = response.body.getReader();
  while (true) {
    const { done, value } = await reader.read();
    if (done) {
      break;
    }
    if (!value || value.length === 0) {
      continue;
    }
    summary.chunks += 1;
    summary.bytes += value.length;
    if (summary.firstChunkMs === null) {
      summary.firstChunkMs = Date.now() - started;
    }
    const text = decoder.decode(value, { stream: true });
    inspectChunk(text, summary);
    if (summary.preview.length < 2048) {
      summary.preview = (summary.preview + text).slice(0, 2048);
    }
    if (summary.bytes >= maxBytes) {
      summary.capped = true;
      await reader.cancel();
      break;
    }
  }
} catch (error) {
  summary.error = error && error.name === "AbortError" ? "timeout" : String(error);
} finally {
  clearTimeout(timeout);
  summary.durationMs = Date.now() - started;
}

summary.ok =
  summary.status >= 200 &&
  summary.status < 300 &&
  summary.chunks > 0 &&
  (!requireDone || summary.doneEvent) &&
  (!requireUsage || summary.usageSeen) &&
  !summary.error;

process.stdout.write(`${JSON.stringify(summary, null, 2)}\n`);
process.exit(summary.ok ? 0 : 1);

function inspectChunk(text, target) {
  if (!text) {
    return;
  }
  if (
    text.includes("[DONE]") ||
    text.includes("message_stop") ||
    text.includes("response.completed") ||
    text.includes("finishReason") ||
    text.includes("finish_reason") ||
    text.includes("stop_reason")
  ) {
    target.doneEvent = true;
  }
  if (
    text.includes('"usage"') ||
    text.includes('"usageMetadata"') ||
    text.includes('"input_tokens"') ||
    text.includes('"prompt_tokens"') ||
    text.includes('"totalTokenCount"')
  ) {
    target.usageSeen = true;
  }
}
