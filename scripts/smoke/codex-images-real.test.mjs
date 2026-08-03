import assert from "node:assert/strict";
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
