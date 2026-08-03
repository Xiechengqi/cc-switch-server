#!/usr/bin/env node

import { createHash } from "node:crypto";
import { mkdir, writeFile } from "node:fs/promises";
import { basename, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const DEFAULT_MAX_BYTES = 96 * 1024 * 1024;
const DEFAULT_TIMEOUT_MS = 15 * 60 * 1000;
const DEFAULT_MAX_SILENCE_MS = 25_000;

export class SseDecoder {
  constructor() {
    this.buffer = "";
  }

  push(text) {
    this.buffer += text;
    return this.#drain(false);
  }

  finish(text = "") {
    this.buffer += text;
    return this.#drain(true);
  }

  #drain(finish) {
    const frames = [];
    while (true) {
      const match = /\r\n\r\n|\n\n|\r\r/.exec(this.buffer);
      if (!match) break;
      const frame = this.buffer.slice(0, match.index);
      this.buffer = this.buffer.slice(match.index + match[0].length);
      const parsed = parseSseFrame(frame);
      if (parsed) frames.push(parsed);
    }
    if (finish && this.buffer.trim()) {
      const parsed = parseSseFrame(this.buffer);
      if (parsed) frames.push(parsed);
      this.buffer = "";
    }
    return frames;
  }
}

export function parseSseFrame(frame) {
  let event = "";
  const data = [];
  for (const rawLine of frame.split(/\r?\n|\r/)) {
    if (!rawLine || rawLine.startsWith(":")) continue;
    if (rawLine.startsWith("event:")) {
      event = rawLine.slice("event:".length).trim();
    } else if (rawLine.startsWith("data:")) {
      data.push(rawLine.slice("data:".length).trimStart());
    }
  }
  if (data.length === 0) return null;
  const raw = data.join("\n").trim();
  if (!raw || raw === "[DONE]") return { event, done: true, value: null };
  return { event, done: false, value: JSON.parse(raw) };
}

export function collectImageEvent(frame, result) {
  if (!frame || frame.done || !frame.value) return;
  const value = frame.value;
  const type = String(value.type || frame.event || "");
  if (type === "error" || value.error || type === "response.failed") {
    const message =
      value.error?.message || value.response?.error?.message || value.message || JSON.stringify(value);
    throw new Error(`image stream failed: ${boundedText(message)}`);
  }
  if (type === "response.incomplete" || type === "response.cancelled" || type === "response.canceled") {
    throw new Error(`image stream terminated with ${type}`);
  }

  if (type === "image_generation.partial_image" || type === "image_edit.partial_image") {
    addUniqueImage(result.partial, value.b64_json, value.partial_image_index);
    return;
  }
  if (type === "image_generation.completed" || type === "image_edit.completed") {
    addUniqueImage(result.final, value.b64_json ?? value.url, null, Boolean(value.url));
    result.completed = true;
    return;
  }
  if (type === "response.image_generation_call.partial_image") {
    addUniqueImage(result.partial, value.partial_image_b64 ?? value.partial_image, value.partial_image_index);
    return;
  }
  if (type === "response.output_item.done") {
    const item = value.item || {};
    if (item.type === "image_generation_call") addUniqueImage(result.final, item.result);
    return;
  }
  if (type === "response.completed" || type === "response.done") {
    for (const item of value.response?.output || []) {
      if (item?.type === "image_generation_call") addUniqueImage(result.final, item.result);
    }
    result.completed = true;
  }
}

export function detectImage(bytes) {
  if (bytes.length >= 8 && bytes.subarray(0, 8).equals(Buffer.from("89504e470d0a1a0a", "hex"))) {
    return { format: "png", mimeType: "image/png" };
  }
  if (bytes.length >= 3 && bytes[0] === 0xff && bytes[1] === 0xd8 && bytes[2] === 0xff) {
    return { format: "jpeg", mimeType: "image/jpeg" };
  }
  if (
    bytes.length >= 12 &&
    bytes.subarray(0, 4).toString("ascii") === "RIFF" &&
    bytes.subarray(8, 12).toString("ascii") === "WEBP"
  ) {
    return { format: "webp", mimeType: "image/webp" };
  }
  throw new Error("image output has an unsupported or invalid signature");
}

export function decodeBase64Image(value) {
  const normalized = String(value ?? "").trim();
  if (
    !normalized ||
    normalized.length % 4 !== 0 ||
    !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(normalized)
  ) {
    throw new Error("image output is not strict standard base64");
  }
  const bytes = Buffer.from(normalized, "base64");
  if (bytes.toString("base64") !== normalized) {
    throw new Error("image output has non-canonical base64 encoding");
  }
  return bytes;
}

export function validateCapabilityHeaders(headers, label = "capability response") {
  const cacheControl = String(headers.get("cache-control") || "").toLowerCase();
  if (!cacheControl.includes("no-store")) {
    throw new Error(`${label} is missing Cache-Control: no-store`);
  }
  if (String(headers.get("x-content-type-options") || "").toLowerCase() !== "nosniff") {
    throw new Error(`${label} is missing X-Content-Type-Options: nosniff`);
  }
  const mimeType = String(headers.get("content-type") || "")
    .split(";", 1)[0]
    .trim()
    .toLowerCase();
  if (!["image/png", "image/jpeg", "image/webp"].includes(mimeType)) {
    throw new Error(`${label} has unsupported Content-Type ${mimeType || "<missing>"}`);
  }
  const rawLength = headers.get("content-length");
  const byteLength = Number(rawLength);
  if (rawLength === null || !Number.isSafeInteger(byteLength) || byteLength <= 0) {
    throw new Error(`${label} has an invalid Content-Length`);
  }
  return { mimeType, byteLength };
}

export function redact(value, secrets = []) {
  let output = String(value ?? "");
  for (const secret of secrets) {
    if (secret) output = output.split(secret).join("[REDACTED]");
  }
  output = output.replace(/Bearer\s+[A-Za-z0-9._~+\/-]+/gi, "Bearer [REDACTED]");
  output = output.replace(/\/v1\/images\/files\/[a-f0-9]{64}/gi, "/v1/images/files/[REDACTED]");
  return output;
}

function addUniqueImage(target, value, index = null, url = false) {
  if (typeof value !== "string" || !value.trim()) return;
  const normalized = value.trim();
  if (target.some((item) => item.value === normalized)) return;
  target.push({ value: normalized, index, url });
}

function boundedText(value, max = 500) {
  return String(value).slice(0, max);
}

function parseArgs(argv, env) {
  const options = {
    baseUrl: env.CC_SWITCH_BASE_URL || env.SERVER_URL || "http://127.0.0.1:15721",
    routeKey: env.CC_SWITCH_CODEX_ROUTE_KEY || "",
    token: env.CC_SWITCH_INFERENCE_TOKEN || "",
    mode: env.CC_SWITCH_IMAGE_SMOKE_MODE || "all",
    prompt:
      env.CC_SWITCH_IMAGE_SMOKE_PROMPT ||
      "A precise black square centered on a plain white background, no text, no watermark.",
    imageModel: env.CC_SWITCH_IMAGE_MODEL || "gpt-image-2",
    responsesModel: env.CC_SWITCH_IMAGE_RESPONSES_MODEL || "gpt-5.4-mini",
    size: env.CC_SWITCH_IMAGE_SIZE || "3840x2160",
    quality: env.CC_SWITCH_IMAGE_QUALITY || "high",
    timeoutMs: numberOption(env.CC_SWITCH_IMAGE_TIMEOUT_MS, DEFAULT_TIMEOUT_MS),
    maxSilenceMs: numberOption(env.CC_SWITCH_IMAGE_MAX_SILENCE_MS, DEFAULT_MAX_SILENCE_MS),
    maxBytes: numberOption(env.CC_SWITCH_IMAGE_MAX_BYTES, DEFAULT_MAX_BYTES),
    outputDir: env.CC_SWITCH_IMAGE_OUTPUT_DIR || "",
    billableConfirmed: env.CC_SWITCH_CODEX_IMAGES_SMOKE === "1",
  };

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const next = () => {
      index += 1;
      if (index >= argv.length) throw new Error(`${arg} requires a value`);
      return argv[index];
    };
    if (arg === "--base-url") options.baseUrl = next();
    else if (arg === "--route-key") options.routeKey = next();
    else if (arg === "--token") options.token = next();
    else if (arg === "--mode") options.mode = next();
    else if (arg === "--prompt") options.prompt = next();
    else if (arg === "--image-model") options.imageModel = next();
    else if (arg === "--responses-model") options.responsesModel = next();
    else if (arg === "--size") options.size = next();
    else if (arg === "--quality") options.quality = next();
    else if (arg === "--timeout-ms") options.timeoutMs = numberOption(next(), 0);
    else if (arg === "--max-silence-ms") options.maxSilenceMs = numberOption(next(), 0);
    else if (arg === "--max-bytes") options.maxBytes = numberOption(next(), 0);
    else if (arg === "--output-dir") options.outputDir = next();
    else if (arg === "--help" || arg === "-h") options.help = true;
    else throw new Error(`unknown argument: ${arg}`);
  }
  return options;
}

function numberOption(raw, fallback) {
  if (raw === undefined || raw === null || raw === "") return fallback;
  const value = Number(raw);
  if (!Number.isFinite(value) || value <= 0) throw new Error(`invalid positive number: ${raw}`);
  return Math.trunc(value);
}

function printHelp() {
  process.stdout.write(`Usage: node scripts/smoke/codex-images-real.mjs [options]\n\n`);
  process.stdout.write(`  --base-url URL\n  --route-key KEY\n  --token TOKEN\n`);
  process.stdout.write(`  --mode stream|json|url|responses|all\n`);
  process.stdout.write(`  --size WIDTHxHEIGHT\n  --timeout-ms N\n  --max-silence-ms N\n`);
  process.stdout.write(`  --output-dir DIR\n\n`);
  process.stdout.write(`Requires CC_SWITCH_CODEX_IMAGES_SMOKE=1 because the default run makes four billable 4K requests.\n`);
}

function endpointUrl(options, endpoint) {
  const base = options.baseUrl.trim().replace(/\/+$/, "");
  const route = options.routeKey ? `/r/${encodeURIComponent(options.routeKey.trim())}` : "";
  return `${base}${route}/v1/${endpoint}`;
}

function requestHeaders(options, accept) {
  return {
    Authorization: `Bearer ${options.token}`,
    "Content-Type": "application/json",
    Accept: accept,
  };
}

async function timedPost(options, endpoint, payload, label) {
  const started = Date.now();
  const controller = new AbortController();
  let lastChunkAt = started;
  let silenceTimer;
  const totalTimer = setTimeout(() => controller.abort(new Error(`${label} total timeout`)), options.timeoutMs);
  const armSilence = () => {
    clearTimeout(silenceTimer);
    silenceTimer = setTimeout(
      () => controller.abort(new Error(`${label} exceeded max silence`)),
      options.maxSilenceMs,
    );
  };
  armSilence();

  const timing = {
    firstChunkMs: null,
    maxSilenceMs: 0,
    chunks: 0,
    bytes: 0,
    durationMs: 0,
  };
  try {
    const response = await fetch(endpointUrl(options, endpoint), {
      method: "POST",
      headers: requestHeaders(options, payload.stream ? "text/event-stream" : "application/json"),
      body: JSON.stringify(payload),
      signal: controller.signal,
    });
    if (!response.body) throw new Error(`${label} returned no readable body`);
    const chunks = [];
    const reader = response.body.getReader();
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      if (!value?.byteLength) continue;
      const now = Date.now();
      timing.maxSilenceMs = Math.max(timing.maxSilenceMs, now - lastChunkAt);
      lastChunkAt = now;
      timing.firstChunkMs ??= now - started;
      timing.chunks += 1;
      timing.bytes += value.byteLength;
      if (timing.bytes > options.maxBytes) {
        await reader.cancel();
        throw new Error(`${label} exceeded ${options.maxBytes} response bytes`);
      }
      chunks.push(Buffer.from(value));
      armSilence();
    }
    timing.durationMs = Date.now() - started;
    if (!response.ok) {
      throw new Error(`${label} returned HTTP ${response.status}: ${boundedText(Buffer.concat(chunks))}`);
    }
    return { response, body: Buffer.concat(chunks), timing };
  } finally {
    clearTimeout(totalTimer);
    clearTimeout(silenceTimer);
  }
}

async function probeSse(options, endpoint, payload, label) {
  const { response, body, timing } = await timedPost(options, endpoint, payload, label);
  const contentType = response.headers.get("content-type") || "";
  if (!contentType.toLowerCase().includes("text/event-stream")) {
    throw new Error(`${label} returned unexpected content-type ${contentType}`);
  }
  const decoder = new SseDecoder();
  const result = { partial: [], final: [], completed: false };
  for (const frame of decoder.finish(body.toString("utf8"))) collectImageEvent(frame, result);
  if (!result.completed || result.final.length === 0) {
    throw new Error(`${label} ended without a completed final image`);
  }
  const outputs = await validateAndSaveImages(options, result, label);
  return { label, timing, partialImages: result.partial.length, finalImages: result.final.length, outputs };
}

async function probeJson(options, responseFormat) {
  const label = `images-${responseFormat}`;
  const { body, timing } = await timedPost(
    options,
    "images/generations",
    {
      model: options.imageModel,
      prompt: options.prompt,
      size: options.size,
      quality: options.quality,
      n: 1,
      response_format: responseFormat,
      stream: false,
    },
    label,
  );
  const value = JSON.parse(body.toString("utf8"));
  if (value.error) throw new Error(`${label} failed: ${boundedText(value.error.message || value.error)}`);
  const data = Array.isArray(value.data) ? value.data : [];
  if (data.length === 0) throw new Error(`${label} response has no data items`);
  const result = { partial: [], final: [], completed: true };
  for (const item of data) addUniqueImage(result.final, item?.b64_json ?? item?.url, null, Boolean(item?.url));
  if (result.final.length === 0) throw new Error(`${label} response has no image output`);
  const outputs = await validateAndSaveImages(options, result, label);
  return { label, timing, partialImages: 0, finalImages: result.final.length, outputs };
}

async function validateAndSaveImages(options, result, label) {
  const outputs = [];
  for (const [kind, images] of [["partial", result.partial], ["final", result.final]]) {
    for (let index = 0; index < images.length; index += 1) {
      const item = images[index];
      const bytes = item.url ? await downloadCapability(options, item.value) : decodeBase64Image(item.value);
      const detected = detectImage(bytes);
      const sha256 = createHash("sha256").update(bytes).digest("hex");
      let path = null;
      if (options.outputDir) {
        await mkdir(options.outputDir, { recursive: true });
        path = resolve(options.outputDir, `${safeName(label)}-${kind}-${String(index + 1).padStart(2, "0")}.${detected.format}`);
        await writeFile(path, bytes, { mode: 0o600 });
      }
      outputs.push({ kind, bytes: bytes.length, format: detected.format, sha256, path });
    }
  }
  return outputs;
}

async function downloadCapability(options, rawUrl) {
  const url = new URL(rawUrl);
  if (!/^https?:$/.test(url.protocol)) throw new Error("capability URL must use HTTP(S)");
  if (!/^\/v1\/images\/files\/[a-f0-9]{64}$/.test(url.pathname) || url.search || url.hash) {
    throw new Error("capability URL has an unexpected path, query, or fragment");
  }
  const headController = new AbortController();
  const headTimer = setTimeout(
    () => headController.abort(new Error("capability HEAD timeout")),
    options.maxSilenceMs,
  );
  let head;
  let headMetadata;
  try {
    head = await fetch(url, {
      method: "HEAD",
      signal: headController.signal,
      redirect: "error",
    });
    if (!head.ok) throw new Error(`capability HEAD returned HTTP ${head.status}`);
    headMetadata = validateCapabilityHeaders(head.headers, "capability HEAD");
    if (headMetadata.byteLength > options.maxBytes) {
      throw new Error("capability HEAD Content-Length exceeds the response bound");
    }
  } finally {
    clearTimeout(headTimer);
  }

  const expected = headMetadata.byteLength;
  const expectedMimeType = headMetadata.mimeType;

  const getController = new AbortController();
  let getTimer;
  const armGetTimer = () => {
    clearTimeout(getTimer);
    getTimer = setTimeout(
      () => getController.abort(new Error("capability GET exceeded max silence")),
      options.maxSilenceMs,
    );
  };
  armGetTimer();
  try {
    const response = await fetch(url, { signal: getController.signal, redirect: "error" });
    if (!response.ok) throw new Error(`capability GET returned HTTP ${response.status}`);
    const metadata = validateCapabilityHeaders(response.headers, "capability GET");
    if (metadata.byteLength !== expected) {
      throw new Error(
        `capability HEAD/GET Content-Length mismatch: expected ${expected}, got ${metadata.byteLength}`,
      );
    }
    if (metadata.mimeType !== expectedMimeType) {
      throw new Error(
        `capability HEAD/GET Content-Type mismatch: expected ${expectedMimeType}, got ${metadata.mimeType}`,
      );
    }
    if (!response.body) throw new Error("capability GET returned no readable body");
    const chunks = [];
    let bytesRead = 0;
    const reader = response.body.getReader();
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      if (!value?.byteLength) continue;
      bytesRead += value.byteLength;
      if (bytesRead > options.maxBytes) {
        await reader.cancel();
        throw new Error("capability image exceeds the response bound");
      }
      chunks.push(Buffer.from(value));
      armGetTimer();
    }
    const bytes = Buffer.concat(chunks);
    if (expected !== bytes.length) {
      throw new Error(`capability Content-Length mismatch: expected ${expected}, got ${bytes.length}`);
    }
    const detected = detectImage(bytes);
    if (detected.mimeType !== expectedMimeType) {
      throw new Error(
        `capability signature does not match Content-Type: expected ${expectedMimeType}, got ${detected.mimeType}`,
      );
    }
    return bytes;
  } finally {
    clearTimeout(getTimer);
  }
}

function safeName(value) {
  return basename(value).replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^-+|-+$/g, "") || "image";
}

async function run(options) {
  if (!options.billableConfirmed) {
    throw new Error(
      "set CC_SWITCH_CODEX_IMAGES_SMOKE=1 to confirm the billable 4K image smoke",
    );
  }
  if (!options.token.trim()) throw new Error("CC_SWITCH_INFERENCE_TOKEN or --token is required");
  if (!options.prompt.trim()) throw new Error("image prompt must not be empty");
  const modes = options.mode === "all" ? ["stream", "json", "url", "responses"] : [options.mode];
  for (const mode of modes) {
    if (!["stream", "json", "url", "responses"].includes(mode)) throw new Error(`unsupported mode: ${mode}`);
  }

  const summaries = [];
  for (const mode of modes) {
    if (mode === "stream") {
      summaries.push(
        await probeSse(
          options,
          "images/generations",
          {
            model: options.imageModel,
            prompt: options.prompt,
            size: options.size,
            quality: options.quality,
            n: 1,
            response_format: "b64_json",
            stream: true,
            partial_images: 1,
          },
          "images-stream",
        ),
      );
    } else if (mode === "json") {
      summaries.push(await probeJson(options, "b64_json"));
    } else if (mode === "url") {
      summaries.push(await probeJson(options, "url"));
    } else {
      summaries.push(
        await probeSse(
          options,
          "responses",
          {
            model: options.responsesModel,
            input: [{ role: "user", content: [{ type: "input_text", text: options.prompt }] }],
            tools: [
              {
                type: "image_generation",
                model: options.imageModel,
                size: options.size,
                quality: options.quality,
                partial_images: 1,
              },
            ],
            tool_choice: { type: "image_generation" },
            stream: true,
            store: false,
          },
          "responses-image-stream",
        ),
      );
    }
  }
  return summaries;
}

async function main() {
  const options = parseArgs(process.argv.slice(2), process.env);
  if (options.help) {
    printHelp();
    return;
  }
  process.stderr.write(
    "[INFO] Running opt-in billable image acceptance; mode=all issues four 4K generation requests.\n",
  );
  const summaries = await run(options);
  process.stdout.write(`${JSON.stringify({ ok: true, summaries }, null, 2)}\n`);
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : "";
if (invokedPath === import.meta.url) {
  main().catch((error) => {
    const secrets = [
      process.env.CC_SWITCH_INFERENCE_TOKEN || "",
      process.env.CC_SWITCH_CODEX_ROUTE_KEY || "",
    ];
    process.stderr.write(`[FAIL] ${redact(error instanceof Error ? error.message : error, secrets)}\n`);
    process.exitCode = 1;
  });
}
