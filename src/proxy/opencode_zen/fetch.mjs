import { once } from "node:events";

const MAX_HEADER = 16384;
const MAX_BODY = 128 * 1024 * 1024;
const paths = new Set([
  "/inference/openai/v1/chat/completions", "/inference/openai/v1/responses",
  "/inference/anthropic/v1/messages", "/inference/v1/chat/completions",
  "/inference/v1/responses", "/inference/v1/messages",
]);

async function write(bytes) {
  if (!process.stdout.write(bytes)) await once(process.stdout, "drain");
}

try {
  let buffered = Buffer.alloc(0);
  let metadata;
  let body;
  let offset = 0;
  for await (const chunk of process.stdin) {
    if (!metadata) {
      buffered = Buffer.concat([buffered, chunk]);
      if (buffered.length < 4) continue;
      const length = buffered.readUInt32BE(0);
      if (length < 2 || length > MAX_HEADER) throw new Error("metadata_limit");
      if (buffered.length < length + 4) continue;
      metadata = JSON.parse(buffered.subarray(4, length + 4).toString("utf8"));
      if (!Number.isSafeInteger(metadata.bodyLength) || metadata.bodyLength < 0 || metadata.bodyLength > MAX_BODY) throw new Error("body_limit");
      body = Buffer.allocUnsafe(metadata.bodyLength);
      const tail = buffered.subarray(length + 4);
      if (tail.length > body.length) throw new Error("body_length");
      tail.copy(body);
      offset = tail.length;
      buffered = undefined;
    } else {
      if (chunk.length > body.length - offset) throw new Error("body_length");
      chunk.copy(body, offset);
      offset += chunk.length;
    }
  }
  if (!metadata || offset !== body.length) throw new Error("body_length");
  const url = new URL(metadata.url);
  const loopback = process.argv.includes("--loopback-test") && url.protocol === "http:" && ["127.0.0.1", "[::1]"].includes(url.hostname);
  if ((!loopback && url.origin !== "https://opencode.ai") || !paths.has(url.pathname) || url.search || url.hash || url.username || url.password) throw new Error("target_rejected");
  if (!Number.isSafeInteger(metadata.timeoutMs) || metadata.timeoutMs < 1 || metadata.timeoutMs > 86400000) throw new Error("deadline_rejected");
  const response = await fetch(url, {
    method: "POST",
    headers: metadata.headers,
    body,
    proxy: metadata.proxy || null,
    redirect: "manual",
    signal: AbortSignal.timeout(metadata.timeoutMs),
  });
  body = undefined;
  metadata = undefined;
  const headers = Object.fromEntries([...response.headers].filter(([key]) => ["content-type", "retry-after", "cf-ray", "x-request-id", "request-id"].includes(key)));
  const encoded = Buffer.from(JSON.stringify({ status: response.status, headers }));
  if (encoded.length > MAX_HEADER) throw new Error("response_header_limit");
  const prefix = Buffer.alloc(4);
  prefix.writeUInt32BE(encoded.length);
  await write(prefix);
  await write(encoded);
  if (response.body) {
    const reader = response.body.getReader();
    try {
      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        await write(Buffer.from(value));
      }
    } finally {
      await reader.cancel();
    }
  }
  process.stdout.end();
} catch {
  // Never expose fetch exceptions: proxy credentials and request data are private.
  process.exitCode = 1;
  process.stdout.destroy();
}
