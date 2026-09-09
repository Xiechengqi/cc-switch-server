import assert from "node:assert/strict";
import http from "node:http";
import test from "node:test";

import { parseSseFrame, runProbe, SseEventDecoder } from "./stream-probe.mjs";

test("SseEventDecoder handles every split and legal line ending", () => {
  for (const input of [
    "event: message\ndata: one\n\n",
    "event: message\r\ndata: one\r\n\r\n",
    "event: message\rdata: one\r\r",
    "event: message\r\ndata: one\n\r",
  ]) {
    for (let split = 0; split <= input.length; split += 1) {
      const decoder = new SseEventDecoder();
      const events = [
        ...decoder.push(input.slice(0, split)),
        ...decoder.push(input.slice(split)),
        ...decoder.finish(),
      ];
      assert.deepEqual(events, [input], `split=${split} input=${JSON.stringify(input)}`);
    }
  }
});

test("parseSseFrame joins multiline data and preserves the event name", () => {
  assert.deepEqual(
    parseSseFrame("event: chunk\nid: 7\ndata: {\"a\":\ndata: 1}\n\n"),
    { eventName: "chunk", data: '{"a":\n1}' },
  );
});

test("OpenAI Chat probe validates fragmented created, finish, usage, and DONE", async () => {
  const stream = [
    chatChunk(
      73,
      { index: 0, delta: { role: "assistant" }, finish_reason: null },
      null,
    ),
    chatChunk(73, { index: 0, delta: { content: "ok" }, finish_reason: null }),
    chatChunk(73, { index: 0, delta: {}, finish_reason: "stop" }),
    chatChunk(73, null, { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 }),
    "data: [DONE]\r\n\r\n",
  ].join("");
  const summary = await probeStream(stream, { requireUsage: true });

  assert.equal(summary.ok, true);
  assert.equal(summary.protocol, "openai-chat");
  assert.equal(summary.chatChunks, 4);
  assert.equal(summary.doneCount, 1);
  assert.equal(summary.createdValid, true);
  assert.equal(summary.createdStable, true);
  assert.equal(summary.finishReasonSeen, true);
  assert.equal(summary.usageSeen, true);
});

test("OpenAI Chat probe rejects missing and drifting created values", async () => {
  const missing = await probeStream(
    `${chatChunk(undefined, { index: 0, delta: {}, finish_reason: "stop" })}data: [DONE]\n\n`,
  );
  assert.equal(missing.ok, false);
  assert.match(missing.error, /created must be a positive integer/);

  const drifting = await probeStream(
    `${chatChunk(73, { index: 0, delta: { role: "assistant" }, finish_reason: null })}${chatChunk(74, { index: 0, delta: {}, finish_reason: "stop" })}data: [DONE]\n\n`,
  );
  assert.equal(drifting.ok, false);
  assert.match(drifting.error, /created changed within one stream/);
});

test("OpenAI Chat probe requires exactly one terminal DONE", async () => {
  const chunk = chatChunk(73, { index: 0, delta: {}, finish_reason: "stop" });
  const missing = await probeStream(chunk);
  assert.equal(missing.ok, false);
  assert.match(missing.error, /exactly one \[DONE\]/);

  const duplicate = await probeStream(`${chunk}data: [DONE]\n\ndata: [DONE]\n\n`);
  assert.equal(duplicate.ok, false);
  assert.match(duplicate.error, /duplicate \[DONE\]/);
});

test("OpenAI Chat probe inspects decoder output flushed only at EOF", async () => {
  const valid = `${chatChunk(73, {
    index: 0,
    delta: {},
    finish_reason: "stop",
  })}data: [DONE]\n\n`;
  const trailingInvalidUtf8 = Buffer.concat([
    Buffer.from(valid),
    Buffer.from("data: "),
    Buffer.from([0xe2]),
  ]);
  const summary = await probeStream(trailingInvalidUtf8);

  assert.equal(summary.ok, false);
  assert.match(summary.error, /after \[DONE\]/);
});

function chatChunk(created, choice, usage) {
  const value = {
    id: "chatcmpl_fixture",
    object: "chat.completion.chunk",
    model: "grok-4.6-build",
    choices: choice ? [choice] : [],
  };
  if (created !== undefined) value.created = created;
  if (usage !== undefined) value.usage = usage;
  return `data: ${JSON.stringify(value)}\r\n\r\n`;
}

async function probeStream(stream, { requireUsage = false } = {}) {
  const server = http.createServer(async (request, response) => {
    for await (const _chunk of request) {
      // Drain the request before responding, matching a normal HTTP handler.
    }
    response.writeHead(200, { "content-type": "text/event-stream; charset=utf-8" });
    const cutPoints = [1, 7, 19, 43, 89, stream.length - 2]
      .filter((value) => value > 0 && value < stream.length)
      .sort((left, right) => left - right);
    let offset = 0;
    for (const end of [...new Set(cutPoints), stream.length]) {
      response.write(stream.slice(offset, end));
      offset = end;
      await new Promise((resolve) => setImmediate(resolve));
    }
    response.end();
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  try {
    const address = server.address();
    return await runProbe({
      url: `http://127.0.0.1:${address.port}/v1/chat/completions`,
      body: '{"stream":true}',
      protocol: "openai-chat",
      requireDone: true,
      requireUsage,
      timeoutMs: 5_000,
      maxBytes: 1024 * 1024,
    });
  } finally {
    await new Promise((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
}
