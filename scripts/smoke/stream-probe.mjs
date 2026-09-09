#!/usr/bin/env node

import { pathToFileURL } from "node:url";

const OPENAI_CHAT_PROTOCOL = "openai-chat";
const GENERIC_PROTOCOL = "generic";

export class SseEventDecoder {
  constructor() {
    this.pending = "";
    this.lineStart = 0;
    this.scanAt = 0;
  }

  push(chunk) {
    this.pending += String(chunk || "");
    return this.#drain(false);
  }

  finish() {
    return this.#drain(true);
  }

  #drain(finish) {
    const events = [];
    while (true) {
      const boundary = nextLineBoundary(this.pending, this.scanAt, finish);
      if (!boundary) {
        this.scanAt = Math.max(
          0,
          this.pending.length - (this.pending.endsWith("\r") ? 1 : 0),
        );
        break;
      }
      const nextLine = boundary.index + boundary.length;
      if (boundary.index === this.lineStart) {
        events.push(this.pending.slice(0, nextLine));
        this.pending = this.pending.slice(nextLine);
        this.lineStart = 0;
        this.scanAt = 0;
      } else {
        this.lineStart = nextLine;
        this.scanAt = nextLine;
      }
    }
    if (finish && this.pending) {
      events.push(this.pending);
      this.pending = "";
      this.lineStart = 0;
      this.scanAt = 0;
    }
    return events;
  }
}

function nextLineBoundary(value, start, finish) {
  for (let index = start; index < value.length; index += 1) {
    if (value[index] === "\n") return { index, length: 1 };
    if (value[index] !== "\r") continue;
    if (value[index + 1] === "\n") return { index, length: 2 };
    if (index + 1 < value.length || finish) return { index, length: 1 };
    return null;
  }
  return null;
}

export function parseSseFrame(frame) {
  let eventName = "";
  const data = [];
  for (const line of String(frame).split(/\r\n|\r|\n/)) {
    if (!line || line.startsWith(":")) continue;
    const separator = line.indexOf(":");
    const field = separator < 0 ? line : line.slice(0, separator);
    let value = separator < 0 ? "" : line.slice(separator + 1);
    if (value.startsWith(" ")) value = value.slice(1);
    if (field === "event") eventName = value;
    if (field === "data") data.push(value);
  }
  return { eventName, data: data.length > 0 ? data.join("\n") : null };
}

function parseArgs(args) {
  const options = {
    headers: {},
    url: "",
    body: "",
    timeoutMs: 60_000,
    maxBytes: 128 * 1024,
    requireDone: false,
    requireUsage: false,
    protocol: GENERIC_PROTOCOL,
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    const next = () => {
      index += 1;
      if (index >= args.length) throw new Error(`${arg} requires a value`);
      return args[index];
    };

    if (arg === "--url") {
      options.url = next();
    } else if (arg === "--body") {
      options.body = next();
    } else if (arg === "--header") {
      const header = next();
      const separator = header.indexOf(":");
      if (separator <= 0) throw new Error(`invalid header: ${header}`);
      options.headers[header.slice(0, separator).trim()] = header
        .slice(separator + 1)
        .trim();
    } else if (arg === "--timeout-ms") {
      options.timeoutMs = Number(next());
    } else if (arg === "--max-bytes") {
      options.maxBytes = Number(next());
    } else if (arg === "--protocol") {
      options.protocol = next();
    } else if (arg === "--require-done") {
      options.requireDone = true;
    } else if (arg === "--require-usage") {
      options.requireUsage = true;
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  return options;
}

function validateOptions(options) {
  if (!options.url) throw new Error("--url is required");
  if (!options.body) throw new Error("--body is required");
  if (!Number.isFinite(options.timeoutMs) || options.timeoutMs <= 0) {
    throw new Error("--timeout-ms must be positive");
  }
  if (!Number.isFinite(options.maxBytes) || options.maxBytes <= 0) {
    throw new Error("--max-bytes must be positive");
  }
  if (![GENERIC_PROTOCOL, OPENAI_CHAT_PROTOCOL].includes(options.protocol)) {
    throw new Error(`unsupported --protocol: ${options.protocol}`);
  }
}

function newSummary(protocol) {
  const chat = protocol === OPENAI_CHAT_PROTOCOL;
  return {
    ok: false,
    protocol,
    status: 0,
    chunks: 0,
    sseEvents: 0,
    chatChunks: 0,
    bytes: 0,
    firstChunkMs: null,
    doneEvent: false,
    doneCount: 0,
    finishReasonSeen: false,
    usageSeen: false,
    createdValid: chat ? false : null,
    createdStable: chat ? false : null,
    capped: false,
    durationMs: 0,
    preview: "",
    error: null,
  };
}

export async function runProbe(inputOptions) {
  const options = {
    headers: {},
    timeoutMs: 60_000,
    maxBytes: 128 * 1024,
    requireDone: false,
    requireUsage: false,
    protocol: GENERIC_PROTOCOL,
    ...inputOptions,
  };
  validateOptions(options);

  const started = Date.now();
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), options.timeoutMs);
  const textDecoder = new TextDecoder();
  const eventDecoder = new SseEventDecoder();
  const summary = newSummary(options.protocol);
  const chatState = { created: null, sawDone: false };
  let reader = null;

  const inspectText = (text, finish = false) => {
    if (text && summary.preview.length < 2048) {
      summary.preview = (summary.preview + text).slice(0, 2048);
    }
    const events = eventDecoder.push(text);
    if (finish) events.push(...eventDecoder.finish());
    for (const frame of events) {
      if (!frame.trim()) continue;
      summary.sseEvents += 1;
      const event = parseSseFrame(frame);
      if (options.protocol === OPENAI_CHAT_PROTOCOL) {
        inspectOpenAiChatEvent(event, summary, chatState);
      } else {
        inspectGenericEvent(event, summary);
      }
    }
  };

  try {
    const headers = new Headers({
      "Content-Type": "application/json",
      Accept: "text/event-stream, application/json, */*",
    });
    for (const [name, value] of new Headers(options.headers)) headers.set(name, value);
    const response = await fetch(options.url, {
      method: "POST",
      headers,
      body: options.body,
      signal: controller.signal,
    });
    summary.status = response.status;
    if (!response.body) throw new Error("response body is not readable");
    if (
      options.protocol === OPENAI_CHAT_PROTOCOL &&
      response.ok &&
      !(response.headers.get("content-type") || "")
        .toLowerCase()
        .includes("text/event-stream")
    ) {
      throw new Error("OpenAI Chat stream response is not text/event-stream");
    }

    reader = response.body.getReader();
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      if (!value || value.length === 0) continue;
      summary.chunks += 1;
      summary.bytes += value.length;
      if (summary.firstChunkMs === null) summary.firstChunkMs = Date.now() - started;
      if (summary.bytes > options.maxBytes) {
        summary.capped = true;
        throw new Error(`stream exceeded the ${options.maxBytes}-byte smoke bound`);
      }
      inspectText(textDecoder.decode(value, { stream: true }));
    }
    inspectText(textDecoder.decode(), true);
    if (options.protocol === OPENAI_CHAT_PROTOCOL) {
      finalizeOpenAiChat(summary, chatState);
    }
  } catch (error) {
    summary.error = error?.name === "AbortError" ? "timeout" : String(error);
    if (reader) {
      try {
        await reader.cancel();
      } catch {
        // The original protocol or transport error remains authoritative.
      }
    }
  } finally {
    clearTimeout(timeout);
    summary.durationMs = Date.now() - started;
  }

  const commonOk =
    summary.status >= 200 &&
    summary.status < 300 &&
    summary.chunks > 0 &&
    !summary.capped &&
    !summary.error &&
    (!options.requireDone || summary.doneEvent) &&
    (!options.requireUsage || summary.usageSeen);
  summary.ok =
    commonOk &&
    (options.protocol !== OPENAI_CHAT_PROTOCOL ||
      (summary.chatChunks > 0 &&
        summary.createdValid &&
        summary.createdStable &&
        summary.doneCount === 1 &&
        summary.finishReasonSeen));
  return summary;
}

function inspectGenericEvent({ eventName, data }, summary) {
  if (data === "[DONE]") {
    summary.doneCount += 1;
    summary.doneEvent = true;
    return;
  }
  let payload = null;
  if (data) {
    try {
      payload = JSON.parse(data);
    } catch {
      // Generic probes retain the historical best-effort contract.
    }
  }
  if (
    eventName === "message_stop" ||
    payload?.type === "message_stop" ||
    payload?.type === "response.completed" ||
    payload?.type === "response.incomplete" ||
    hasFinishReason(payload)
  ) {
    summary.doneEvent = true;
  }
  if (hasUsage(payload, data)) summary.usageSeen = true;
}

function inspectOpenAiChatEvent({ data }, summary, state) {
  if (data === null || data.trim() === "" || data === "ping" || data === "keepalive") {
    return;
  }
  if (data.trim() === "[DONE]") {
    summary.doneCount += 1;
    summary.doneEvent = true;
    if (summary.doneCount !== 1) throw new Error("OpenAI Chat stream emitted duplicate [DONE]");
    state.sawDone = true;
    return;
  }
  if (state.sawDone) throw new Error("OpenAI Chat stream emitted data after [DONE]");

  let payload;
  try {
    payload = JSON.parse(data);
  } catch (error) {
    throw new Error(`OpenAI Chat stream emitted invalid SSE JSON: ${error.message}`);
  }
  if (payload?.error || payload?.object === "error") {
    throw new Error("OpenAI Chat stream emitted an error envelope");
  }
  if (
    !payload ||
    payload.object !== "chat.completion.chunk" ||
    typeof payload.id !== "string" ||
    payload.id.trim() === "" ||
    typeof payload.model !== "string" ||
    payload.model.trim() === "" ||
    !Array.isArray(payload.choices)
  ) {
    throw new Error("OpenAI Chat SSE payload does not satisfy the chunk envelope contract");
  }
  if (!Number.isSafeInteger(payload.created) || payload.created <= 0) {
    summary.createdValid = false;
    throw new Error("OpenAI Chat chunk created must be a positive integer Unix timestamp");
  }
  if (state.created === null) state.created = payload.created;
  if (state.created !== payload.created) {
    summary.createdStable = false;
    throw new Error("OpenAI Chat chunk created changed within one stream");
  }

  for (const choice of payload.choices) {
    if (
      !choice ||
      !Number.isInteger(choice.index) ||
      choice.index < 0 ||
      !choice.delta ||
      typeof choice.delta !== "object" ||
      Array.isArray(choice.delta) ||
      (choice.finish_reason !== null && typeof choice.finish_reason !== "string")
    ) {
      throw new Error("OpenAI Chat stream emitted an invalid choices entry");
    }
    if (typeof choice.finish_reason === "string" && choice.finish_reason.length > 0) {
      summary.finishReasonSeen = true;
    }
  }
  if (
    payload.usage !== undefined &&
    payload.usage !== null &&
    (typeof payload.usage !== "object" || Array.isArray(payload.usage))
  ) {
    throw new Error("OpenAI Chat stream emitted an invalid usage object");
  }
  if (payload.usage !== undefined && payload.usage !== null) {
    for (const key of ["prompt_tokens", "completion_tokens", "total_tokens"]) {
      if (!Number.isSafeInteger(payload.usage[key]) || payload.usage[key] < 0) {
        throw new Error(`OpenAI Chat stream emitted invalid usage.${key}`);
      }
    }
    summary.usageSeen = true;
  }
  summary.chatChunks += 1;
  summary.createdValid = true;
  summary.createdStable = true;
}

function finalizeOpenAiChat(summary, state) {
  if (summary.chatChunks === 0) throw new Error("OpenAI Chat stream ended without chunks");
  if (summary.doneCount !== 1) {
    throw new Error("OpenAI Chat stream must terminate with exactly one [DONE]");
  }
  if (!summary.finishReasonSeen) {
    throw new Error("OpenAI Chat stream ended without choices[].finish_reason");
  }
  summary.createdValid = true;
  summary.createdStable = state.created !== null;
}

function hasFinishReason(payload) {
  if (!payload || typeof payload !== "object") return false;
  if (Array.isArray(payload.choices)) {
    return payload.choices.some((choice) => typeof choice?.finish_reason === "string");
  }
  if (Array.isArray(payload.candidates)) {
    return payload.candidates.some(
      (candidate) => typeof candidate?.finishReason === "string",
    );
  }
  return false;
}

function hasUsage(payload, rawData) {
  if (payload && typeof payload === "object") {
    if (payload.usage || payload.usageMetadata) return true;
    if (
      payload.input_tokens !== undefined ||
      payload.prompt_tokens !== undefined ||
      payload.totalTokenCount !== undefined
    ) {
      return true;
    }
  }
  return (
    typeof rawData === "string" &&
    /"(?:usage|usageMetadata|input_tokens|prompt_tokens|totalTokenCount)"/.test(rawData)
  );
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const summary = await runProbe(options);
  process.stdout.write(`${JSON.stringify(summary, null, 2)}\n`);
  process.exitCode = summary.ok ? 0 : 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 2;
  });
}
