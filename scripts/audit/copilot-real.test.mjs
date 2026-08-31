import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import http from "node:http";
import path from "node:path";
import test from "node:test";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const script = path.join(repoRoot, "scripts/smoke/copilot-real.mjs");

const accountId = "copilot-account-1";
const authIdentityGeneration = 7;
const modelId = "gpt-4.1";
const providerIds = Object.freeze({
  claude: "copilot-claude-provider",
  codex: "copilot-codex-provider",
  gemini: "copilot-gemini-provider",
});

function runScript(overrides = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [script], {
      cwd: repoRoot,
      env: {
        ...process.env,
        RUN_REAL: "1",
        CC_SWITCH_SHARE_URL: "",
        ROUTER_API_TOKEN: "",
        ROUTER_API_TOKEN_HEADER: "Authorization",
        SERVER_URL: "",
        CC_SWITCH_SERVER_TOKEN: "",
        GITHUB_COPILOT_TEST_ACCOUNT: "",
        GITHUB_COPILOT_GITHUB_DOMAIN: "",
        GITHUB_COPILOT_TOKEN_FIXTURE: "",
        CC_SWITCH_COPILOT_CLAUDE_PROVIDER_ID: "",
        CC_SWITCH_COPILOT_CODEX_PROVIDER_ID: "",
        CC_SWITCH_COPILOT_GEMINI_PROVIDER_ID: "",
        CC_SWITCH_COPILOT_MODEL: "",
        EVIDENCE_FILE: "",
        CC_SWITCH_REAL_TIMEOUT_MS: "5000",
        ...overrides,
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
    child.once("close", (code, signal) => resolve({ code, signal, stdout, stderr }));
  });
}

async function readJson(request) {
  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  const body = Buffer.concat(chunks).toString("utf8");
  return body ? JSON.parse(body) : null;
}

function sendJson(response, status, body) {
  response.writeHead(status, { "content-type": "application/json" });
  response.end(JSON.stringify(body));
}

function sendSse(response, payloads) {
  response.writeHead(200, {
    "content-type": "text/event-stream; charset=utf-8",
    "cache-control": "no-cache",
  });
  for (const payload of payloads) response.write(`data: ${JSON.stringify(payload)}\n\n`);
  response.end();
}

function providerView(app) {
  return {
    app,
    providerType: "github_copilot",
    providerTypeId: "github_copilot",
    provider: { id: providerIds[app] },
    runtime: {
      driverId: "special.copilot",
      configurationState: "ready",
      authRef: {
        kind: "managed_account",
        accountId,
        expectedProviderType: "github_copilot",
        authIdentityGeneration,
      },
    },
  };
}

function catalog(app) {
  return {
    ok: true,
    outcome: "success",
    providerId: providerIds[app],
    app,
    providerType: "github_copilot",
    driverId: "special.copilot",
    source: "copilot_models_api",
    stale: false,
    fetchedAtMs: Date.now(),
    models: [
      {
        id: modelId,
        raw: {
          entitlementSource: "copilot_models_api",
          githubDomain: "github.example.test",
          apiOrigin: "https://api.githubcopilot.example.test",
          modelPickerEnabled: true,
          policyState: "enabled",
          preview: false,
          supportedEndpoints: ["chat/completions"],
          limits: {
            maxContextWindowTokens: 128000,
            maxOutputTokens: 16384,
          },
          capabilities: { tools: true, vision: true, reasoning: true },
        },
      },
    ],
  };
}

async function startMockServer({ failAccountsWith = "" } = {}) {
  const seen = [];
  const server = http.createServer(async (request, response) => {
    try {
      const url = new URL(request.url, "http://127.0.0.1");
      const body = request.method === "POST" ? await readJson(request) : null;
      seen.push({ method: request.method, path: url.pathname, search: url.search, body });

      if (request.method === "GET" && url.pathname === "/api/accounts") {
        if (failAccountsWith) {
          response.writeHead(500, { "content-type": "text/plain" });
          response.end(failAccountsWith);
          return;
        }
        sendJson(response, 200, {
          ok: true,
          accounts: [
            {
              id: accountId,
              email: "copilot@example.test",
              providerType: "github_copilot",
              authIdentityGeneration,
            },
            {
              id: "negative-account",
              email: "other@example.test",
              providerType: "github_copilot",
              authIdentityGeneration: 99,
            },
          ],
        });
        return;
      }
      if (request.method === "GET" && url.pathname === "/api/providers") {
        sendJson(response, 200, {
          ok: true,
          providers: [providerView("claude"), providerView("codex"), providerView("gemini")],
        });
        return;
      }
      const catalogMatch = /^\/api\/providers\/([^/]+)\/fetch-models$/.exec(url.pathname);
      if (request.method === "POST" && catalogMatch) {
        const app = Object.entries(providerIds).find(([, id]) => id === catalogMatch[1])?.[0];
        assert.ok(app, `unexpected Provider ID ${catalogMatch[1]}`);
        assert.deepEqual(body, { app, merge: false, timeoutMs: 5000 });
        sendJson(response, 200, catalog(app));
        return;
      }
      if (
        request.method === "GET" &&
        url.pathname === `/api/accounts/${accountId}/quota` &&
        url.search === "?refresh=true&force=true"
      ) {
        sendJson(response, 200, {
          ok: true,
          account: {
            id: accountId,
            providerType: "github_copilot",
            authIdentityGeneration,
          },
          quota: {
            success: true,
            credentialMessage: "copilot_business",
            tiers: [
              {
                name: "premium",
                unit: "premium_interactions",
                utilization: 0.25,
              },
            ],
          },
        });
        return;
      }
      if (request.method === "POST" && url.pathname === "/v1/messages") {
        assert.equal(body.model, modelId);
        if (body.stream) {
          sendSse(response, [
            { type: "message_start", message: { usage: { input_tokens: 4, output_tokens: 0 } } },
            {
              type: "content_block_start",
              index: 0,
              content_block: { type: "tool_use", id: "tool-claude", name: "lookup", input: {} },
            },
            { type: "message_delta", delta: { stop_reason: "tool_use" }, usage: { output_tokens: 3 } },
            { type: "message_stop" },
          ]);
        } else {
          sendJson(response, 200, {
            type: "message",
            role: "assistant",
            content: [{ type: "tool_use", id: "tool-claude", name: "lookup", input: { key: "copilot-claude" } }],
            usage: { input_tokens: 4, output_tokens: 3 },
          });
        }
        return;
      }
      if (request.method === "POST" && url.pathname === "/v1/responses") {
        assert.equal(body.model, modelId);
        const completed = {
          object: "response",
          status: "completed",
          output: [{ type: "function_call", call_id: "call-codex", name: "lookup", arguments: "{\"key\":\"copilot-codex\"}" }],
          usage: { input_tokens: 5, output_tokens: 4 },
        };
        if (body.stream) {
          sendSse(response, [
            { type: "response.output_item.added", item: completed.output[0] },
            { type: "response.completed", response: completed },
          ]);
        } else {
          sendJson(response, 200, completed);
        }
        return;
      }
      const geminiMatch = /^\/v1beta\/models\/([^/:]+):(generateContent|streamGenerateContent)$/.exec(
        url.pathname,
      );
      if (request.method === "POST" && geminiMatch) {
        assert.equal(decodeURIComponent(geminiMatch[1]), modelId);
        const payload = {
          candidates: [
            {
              content: { role: "model", parts: [{ functionCall: { name: "lookup", args: { key: "copilot-gemini" } } }] },
              finishReason: "STOP",
            },
          ],
          usageMetadata: { promptTokenCount: 5, candidatesTokenCount: 3, totalTokenCount: 8 },
        };
        if (geminiMatch[2] === "streamGenerateContent") {
          assert.equal(url.search, "?alt=sse");
          sendSse(response, [payload]);
        } else {
          sendJson(response, 200, payload);
        }
        return;
      }
      sendJson(response, 404, { error: `unhandled ${request.method} ${url.pathname}${url.search}` });
    } catch (error) {
      sendJson(response, 500, { error: error instanceof Error ? error.message : String(error) });
    }
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  assert.ok(address && typeof address === "object");
  return {
    url: `http://127.0.0.1:${address.port}`,
    seen,
    close: () => new Promise((resolve, reject) => server.close((error) => (error ? reject(error) : resolve()))),
  };
}

function fullEnv(url) {
  return {
    CC_SWITCH_SHARE_URL: url,
    ROUTER_API_TOKEN: "router-secret-for-test",
    SERVER_URL: url,
    CC_SWITCH_SERVER_TOKEN: "server-secret-for-test",
    GITHUB_COPILOT_TEST_ACCOUNT: "copilot@example.test",
    GITHUB_COPILOT_GITHUB_DOMAIN: "github.example.test",
    CC_SWITCH_COPILOT_CLAUDE_PROVIDER_ID: providerIds.claude,
    CC_SWITCH_COPILOT_CODEX_PROVIDER_ID: providerIds.codex,
    CC_SWITCH_COPILOT_GEMINI_PROVIDER_ID: providerIds.gemini,
  };
}

test("Copilot real gate verifies one Account generation across three protocol surfaces", async () => {
  const mock = await startMockServer();
  try {
    const result = await runScript(fullEnv(mock.url));
    assert.equal(result.signal, null);
    assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
    assert.match(result.stdout, /Copilot Claude\/Codex\/Gemini Providers bind one Account generation/);
    assert.match(result.stdout, /Copilot model entitlement across three Providers/);
    assert.match(result.stdout, /Copilot premium quota/);
    assert.match(result.stdout, /Copilot Claude non-stream\/stream tool, usage, terminal/);
    assert.match(result.stdout, /Copilot Codex non-stream\/stream tool, usage, terminal/);
    assert.match(result.stdout, /Copilot Gemini non-stream\/stream tool, usage, terminal/);
    assert.match(result.stdout, /GitHub Copilot real-account gate complete/);
    assert.equal(result.stderr, "");
    assert.equal(mock.seen.filter((entry) => entry.path.endsWith("/fetch-models")).length, 3);
    assert.equal(mock.seen.filter((entry) => entry.path === "/v1/messages").length, 2);
    assert.equal(mock.seen.filter((entry) => entry.path === "/v1/responses").length, 2);
    assert.equal(mock.seen.filter((entry) => /generateContent/i.test(entry.path)).length, 2);
  } finally {
    await mock.close();
  }
});

test("Copilot real gate reports missing real inputs as a non-passing skip", async () => {
  const result = await runScript({ RUN_REAL: "0" });
  assert.equal(result.code, 0, result.stderr);
  assert.match(result.stdout, /^\[SKIP\].*RUN_REAL=1.*CC_SWITCH_SHARE_URL/m);
  assert.doesNotMatch(result.stdout, /\[PASS\]/);
  assert.equal(result.stderr, "");
});

test("Copilot real gate redacts bearer and configured token values on failure", async () => {
  const secret = "server-secret-that-must-never-appear";
  const mock = await startMockServer({
    failAccountsWith: `upstream rejected Bearer ${secret}; access_token=\"${secret}\"`,
  });
  try {
    const result = await runScript({
      ...fullEnv(mock.url),
      CC_SWITCH_SERVER_TOKEN: secret,
    });
    assert.equal(result.code, 1, result.stdout);
    assert.match(result.stderr, /\[FAIL\]/);
    assert.match(result.stderr, /\[REDACTED\]/);
    assert.doesNotMatch(`${result.stdout}\n${result.stderr}`, new RegExp(secret, "g"));
  } finally {
    await mock.close();
  }
});
