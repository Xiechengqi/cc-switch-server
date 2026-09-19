import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import http from "node:http";
import path from "node:path";
import test from "node:test";

import {
  collectImageEvent,
  decodeBase64Image,
  detectImage,
  parseSseFrame,
  redact,
  SseDecoder,
  validateCapabilityHeaders,
} from "./codex-images-real.mjs";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const script = path.join(repoRoot, "scripts/smoke/codex-images-real.mjs");

test("SseDecoder preserves image events split across chunks", () => {
  const decoder = new SseDecoder();
  assert.deepEqual(decoder.push("event: image_generation.partial_image\n"), []);
  const frames = decoder.push(
    'data: {"type":"image_generation.partial_image","partial_image_index":0,"b64_json":"cGFydGlhbA=="}\n\n',
  );
  assert.equal(frames.length, 1);
  const result = { partial: [], final: [], completed: false };
  collectImageEvent(frames[0], result);
  assert.equal(result.partial[0].value, "cGFydGlhbA==");
});

test("collectImageEvent supports Responses image output", () => {
  const frame = parseSseFrame(
    'data: {"type":"response.output_item.done","item":{"type":"image_generation_call","result":"ZmluYWw="}}',
  );
  const completed = parseSseFrame(
    'data: {"type":"response.completed","response":{"output":[]}}',
  );
  const result = { partial: [], final: [], completed: false };
  collectImageEvent(frame, result);
  collectImageEvent(completed, result);
  assert.equal(result.final[0].value, "ZmluYWw=");
  assert.equal(result.completed, true);
});

test("detectImage accepts supported signatures and rejects arbitrary bytes", () => {
  assert.equal(detectImage(Buffer.from("89504e470d0a1a0a", "hex")).format, "png");
  assert.equal(detectImage(Buffer.from("ffd8ff00", "hex")).format, "jpeg");
  assert.throws(() => detectImage(Buffer.from("not-an-image")), /unsupported/);
});

test("decodeBase64Image rejects Node's permissive base64 edge cases", () => {
  assert.deepEqual(decodeBase64Image("iVBORw0KGgo="), Buffer.from("89504e470d0a1a0a", "hex"));
  assert.throws(() => decodeBase64Image("not base64!!"), /strict standard base64/);
  assert.throws(() => decodeBase64Image("aGVsbG8"), /strict standard base64/);
});

test("validateCapabilityHeaders enforces download integrity headers", () => {
  const metadata = validateCapabilityHeaders(
    new Headers({
      "cache-control": "private, no-store, max-age=0",
      "content-length": "8",
      "content-type": "image/png",
      "x-content-type-options": "nosniff",
    }),
  );
  assert.deepEqual(metadata, { mimeType: "image/png", byteLength: 8 });
  assert.throws(
    () =>
      validateCapabilityHeaders(
        new Headers({
          "cache-control": "private, no-store",
          "content-length": "8",
          "content-type": "image/png",
        }),
      ),
    /nosniff/,
  );
});

test("redact removes bearer values and capability tokens", () => {
  const token = "secret-token-value";
  const capability = "a".repeat(64);
  const output = redact(
    `Bearer ${token} https://example.test/v1/images/files/${capability}`,
    [token],
  );
  assert.equal(output.includes(token), false);
  assert.equal(output.includes(capability), false);
});

test("probe failures never print tokens, prompts, or raw upstream bodies", async () => {
  const token = "codex-probe-router-token-secret";
  const prompt = "codex-probe-private-prompt-secret";
  const rawFailure = "codex-probe-raw-upstream-body-secret";
  const server = http.createServer((request, response) => {
    request.resume();
    response.writeHead(502, { "content-type": "text/plain" });
    response.end(`${rawFailure}:${prompt}:${request.headers.authorization || ""}`);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  assert.ok(address && typeof address === "object");

  try {
    const result = await new Promise((resolve, reject) => {
      const child = spawn(process.execPath, [script, "--mode", "json", "--prompt", prompt], {
        cwd: repoRoot,
        env: {
          ...process.env,
          CC_SWITCH_CODEX_IMAGES_SMOKE: "1",
          CC_SWITCH_SHARE_URL: `http://127.0.0.1:${address.port}`,
          ROUTER_API_TOKEN: token,
          CC_SWITCH_IMAGE_MODEL: "gpt-image-2",
          CC_SWITCH_IMAGE_SIZE: "1024x1024",
          CC_SWITCH_IMAGE_QUALITY: "low",
          CC_SWITCH_IMAGE_TIMEOUT_MS: "5000",
          CC_SWITCH_IMAGE_MAX_SILENCE_MS: "5000",
        },
        stdio: ["ignore", "pipe", "pipe"],
      });
      let stdout = "";
      let stderr = "";
      child.stdout.setEncoding("utf8");
      child.stderr.setEncoding("utf8");
      child.stdout.on("data", (chunk) => {
        stdout += chunk;
      });
      child.stderr.on("data", (chunk) => {
        stderr += chunk;
      });
      child.once("error", reject);
      child.once("close", (code) => resolve({ code, stdout, stderr }));
    });
    assert.equal(result.code, 1);
    const output = `${result.stdout}\n${result.stderr}`;
    assert.equal(output.includes(token), false);
    assert.equal(output.includes(prompt), false);
    assert.equal(output.includes(rawFailure), false);
    assert.match(output, /returned HTTP 502/);
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});
