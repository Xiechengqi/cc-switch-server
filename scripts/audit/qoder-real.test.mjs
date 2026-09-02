import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import test from "node:test";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const script = path.join(repoRoot, "scripts/smoke/qoder-real.mjs");
const oracle = JSON.parse(
  fs.readFileSync(path.join(repoRoot, "assets/contract/qoder-cli-oracle.json"), "utf8"),
);
const model = "glm-5.3";
const authIdentityGeneration = 7;
const tokenRefreshGeneration = 11;
const serverSecret = "qoder-server-secret-for-harness";
const routerSecret = "qoder-router-secret-for-harness";

const specs = Object.freeze({
  global_oauth: Object.freeze({
    site: "global",
    accountEnv: "QODER_GLOBAL_OAUTH_TEST_ACCOUNT",
    modelEnv: "CC_SWITCH_QODER_GLOBAL_OAUTH_MODEL",
    providerEnvs: Object.freeze({
      claude: "CC_SWITCH_QODER_GLOBAL_OAUTH_CLAUDE_PROVIDER_ID",
      codex: "CC_SWITCH_QODER_GLOBAL_OAUTH_CODEX_PROVIDER_ID",
      gemini: "CC_SWITCH_QODER_GLOBAL_OAUTH_GEMINI_PROVIDER_ID",
    }),
  }),
  global_pat: Object.freeze({
    site: "global",
    accountEnv: "QODER_GLOBAL_PAT_TEST_ACCOUNT",
    modelEnv: "CC_SWITCH_QODER_GLOBAL_PAT_MODEL",
    providerEnvs: Object.freeze({
      claude: "CC_SWITCH_QODER_GLOBAL_PAT_CLAUDE_PROVIDER_ID",
      codex: "CC_SWITCH_QODER_GLOBAL_PAT_CODEX_PROVIDER_ID",
      gemini: "CC_SWITCH_QODER_GLOBAL_PAT_GEMINI_PROVIDER_ID",
    }),
  }),
  cn_oauth: Object.freeze({
    site: "cn",
    accountEnv: "QODER_CN_OAUTH_TEST_ACCOUNT",
    modelEnv: "CC_SWITCH_QODER_CN_OAUTH_MODEL",
    providerEnvs: Object.freeze({
      claude: "CC_SWITCH_QODER_CN_OAUTH_CLAUDE_PROVIDER_ID",
      codex: "CC_SWITCH_QODER_CN_OAUTH_CODEX_PROVIDER_ID",
      gemini: "CC_SWITCH_QODER_CN_OAUTH_GEMINI_PROVIDER_ID",
    }),
  }),
});

function runScript(rail, overrides = {}) {
  const cleared = {};
  for (const spec of Object.values(specs)) {
    cleared[spec.accountEnv] = "";
    cleared[spec.modelEnv] = "";
    for (const name of Object.values(spec.providerEnvs)) cleared[name] = "";
  }
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [script, "--rail", rail], {
      cwd: repoRoot,
      env: {
        ...process.env,
        ...cleared,
        RUN_REAL: "1",
        CC_SWITCH_QODER_HARNESS_MODE: "fixture",
        SERVER_URL: "",
        CC_SWITCH_SERVER_TOKEN: "",
        CC_SWITCH_SHARE_URL: "",
        ROUTER_API_TOKEN: "",
        ROUTER_API_TOKEN_HEADER: "Authorization",
        QODER_REAL_RECEIPT_FILE: "",
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

function sendJson(response, status, value) {
  response.writeHead(status, { "content-type": "application/json" });
  response.end(JSON.stringify(value));
}

function sendSse(response, values) {
  response.writeHead(200, {
    "content-type": "text/event-stream; charset=utf-8",
    "cache-control": "no-cache",
  });
  for (const value of values) response.write(`data: ${JSON.stringify(value)}\n\n`);
  response.end();
}

function accountFor(rail) {
  const spec = specs[rail];
  const oauth = rail !== "global_pat";
  return {
    id: `qoder-${spec.site}-${rail.replaceAll("_", "-")}`,
    providerType: "qoder_cosy",
    authIdentityGeneration,
    tokenRefreshGeneration,
    hasAccessToken: oauth,
    hasRefreshToken: oauth,
    hasApiKey: !oauth,
    needsRelogin: false,
  };
}

function providerIdsFor(rail) {
  return Object.fromEntries(
    ["claude", "codex", "gemini"].map((app) => [app, `${rail}-${app}-provider`]),
  );
}

function providerView(rail, app, { bindingMismatch = false } = {}) {
  const account = accountFor(rail);
  return {
    app,
    providerType: "qoder_cosy",
    providerTypeId: "qoder_cosy",
    provider: { id: providerIdsFor(rail)[app] },
    runtime: {
      driverId: "special.qoder_cosy",
      runtimeFingerprint: `runtime-${rail}-${app}`,
      configurationState: "ready",
      authRef: {
        kind: "managed_account",
        accountId: bindingMismatch ? "qoder-global-decoy" : account.id,
        expectedProviderType: "qoder_cosy",
        authIdentityGeneration,
      },
    },
  };
}

async function startMockServer(rail, { bindingMismatch = false, failAccountsWith = "" } = {}) {
  const account = accountFor(rail);
  const providerIds = providerIdsFor(rail);
  const seen = [];
  const server = http.createServer(async (request, response) => {
    try {
      const url = new URL(request.url, "http://127.0.0.1");
      const body = request.method === "POST" ? await readJson(request) : null;
      seen.push({
        method: request.method,
        path: url.pathname,
        search: url.search,
        authorization: request.headers.authorization || "",
        body,
      });

      if (request.method === "GET" && url.pathname === "/api/accounts") {
        if (failAccountsWith) {
          response.writeHead(500, { "content-type": "text/plain" });
          response.end(failAccountsWith);
          return;
        }
        sendJson(response, 200, {
          ok: true,
          accounts: [
            account,
            {
              ...account,
              id: `qoder-${specs[rail].site}-decoy-account`,
              authIdentityGeneration: 99,
            },
            {
              ...account,
              id: `qoder-${specs[rail].site === "cn" ? "global" : "cn"}-other-site`,
              authIdentityGeneration: 101,
            },
          ],
        });
        return;
      }
      if (request.method === "GET" && url.pathname === "/api/providers") {
        sendJson(response, 200, {
          ok: true,
          providers: [
            providerView(rail, "claude", { bindingMismatch }),
            providerView(rail, "codex", { bindingMismatch }),
            providerView(rail, "gemini", { bindingMismatch }),
            {
              ...providerView(rail, "claude"),
              app: "claude",
              provider: { id: "decoy-provider" },
            },
          ],
        });
        return;
      }
      if (request.method === "GET" && url.pathname === "/v1/models") {
        const app = url.searchParams.get("app");
        assert.equal(url.searchParams.get("providerId"), providerIds[app]);
        sendJson(response, 200, {
          object: "list",
          source: "qoder_live_model_catalog",
          stale: false,
          fetchedAtMs: Date.now(),
          data: [{ id: model, object: "model", owned_by: "qoder" }],
        });
        return;
      }
      if (
        request.method === "GET" &&
        url.pathname === `/api/accounts/${account.id}/quota` &&
        url.search === "?refresh=true&force=true"
      ) {
        sendJson(response, 200, {
          ok: true,
          account,
          quota: {
            success: true,
            tiers: [{ name: "qoder_user", unit: "credits", utilization: 0.2 }],
            extraUsage: { qoderQuota: { availability: "available" } },
          },
        });
        return;
      }
      if (request.method === "POST" && url.pathname === "/v1/messages") {
        assert.equal(body.model, model);
        if (body.stream) {
          sendSse(response, [
            { type: "message_start", message: { usage: { input_tokens: 3, output_tokens: 0 } } },
            {
              type: "content_block_start",
              index: 0,
              content_block: { type: "tool_use", id: "tool-claude", name: "lookup", input: {} },
            },
            { type: "message_delta", delta: { stop_reason: "tool_use" }, usage: { output_tokens: 1 } },
            { type: "message_stop" },
          ]);
        } else {
          sendJson(response, 200, {
            type: "message",
            role: "assistant",
            content: [
              {
                type: "tool_use",
                id: "tool-claude",
                name: "lookup",
                input: { key: "qoder-acceptance" },
              },
            ],
            usage: { input_tokens: 3, output_tokens: 1 },
          });
        }
        return;
      }
      if (request.method === "POST" && url.pathname === "/v1/responses") {
        assert.equal(body.model, model);
        const completed = {
          object: "response",
          status: "completed",
          output: [
            {
              type: "function_call",
              call_id: "call-codex",
              name: "lookup",
              arguments: "{\"key\":\"qoder-acceptance\"}",
            },
          ],
          usage: { input_tokens: 3, output_tokens: 1, total_tokens: 4 },
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
      const gemini = /^\/v1beta\/models\/([^/:]+):(generateContent|streamGenerateContent)$/.exec(
        url.pathname,
      );
      if (request.method === "POST" && gemini) {
        assert.equal(decodeURIComponent(gemini[1]), model);
        const payload = {
          candidates: [
            {
              content: {
                role: "model",
                parts: [
                  { functionCall: { name: "lookup", args: { key: "qoder-acceptance" } } },
                ],
              },
              finishReason: "STOP",
            },
          ],
          usageMetadata: { promptTokenCount: 3, candidatesTokenCount: 1, totalTokenCount: 4 },
        };
        if (gemini[2] === "streamGenerateContent") {
          assert.equal(url.search, "?alt=sse");
          sendSse(response, [payload]);
        } else {
          sendJson(response, 200, payload);
        }
        return;
      }
      sendJson(response, 404, { error: "unhandled harness route" });
    } catch (error) {
      sendJson(response, 500, { error: error instanceof Error ? error.message : "harness error" });
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
    close: () =>
      new Promise((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      ),
  };
}

function fullEnv(rail, url, receiptFile) {
  const spec = specs[rail];
  const providerIds = providerIdsFor(rail);
  return {
    SERVER_URL: url,
    CC_SWITCH_SERVER_TOKEN: serverSecret,
    CC_SWITCH_SHARE_URL: url,
    ROUTER_API_TOKEN: routerSecret,
    QODER_REAL_RECEIPT_FILE: receiptFile,
    [spec.accountEnv]: accountFor(rail).id,
    [spec.modelEnv]: model,
    ...Object.fromEntries(
      Object.entries(spec.providerEnvs).map(([app, name]) => [name, providerIds[app]]),
    ),
  };
}

function assertNoForbiddenReceiptFields(value) {
  if (Array.isArray(value)) return value.forEach(assertNoForbiddenReceiptFields);
  if (!value || typeof value !== "object") return;
  for (const [key, nested] of Object.entries(value)) {
    assert.ok(!oracle.receiptSchema.forbiddenFields.includes(key), `forbidden receipt field ${key}`);
    assert.doesNotMatch(key, /prompt|callback|raw(?:request|response|body)/i);
    assertNoForbiddenReceiptFields(nested);
  }
}

test("Qoder real harness keeps Global OAuth, Global PAT, and CN OAuth receipts independent", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `qoder-real-${rail}-`));
      const receiptFile = path.join(directory, "receipt.json");
      const mock = await startMockServer(rail);
      try {
        const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
        assert.equal(result.signal, null);
        assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
        assert.match(result.stdout, /verificationState=contract_verified, liveState=live_pending/);
        assert.equal(result.stderr, "");
        const receipt = JSON.parse(fs.readFileSync(receiptFile, "utf8"));
        assert.equal(receipt.verificationState, "contract_verified");
        assert.equal(receipt.liveState, "live_pending");
        assert.equal(receipt.site, specs[rail].site);
        assert.equal(
          receipt.credentialRail,
          rail === "global_pat" ? "pat_job_token" : rail,
        );
        assert.equal(receipt.otherAccountRequests, 0);
        assert.equal(receipt.otherProviderRequests, 0);
        assert.equal(receipt.otherSiteRequests, 0);
        assert.equal(receipt.sensitiveScan.status, "pass");
        assert.equal(receipt.sensitiveScan.matches, 0);
        assert.deepEqual(receipt.surfaceChecks, {
          claude: { nonstream: "pass", stream: "pass", tool: "pass" },
          codex: { nonstream: "pass", stream: "pass", tool: "pass" },
          gemini: { nonstream: "pass", stream: "pass", tool: "pass" },
        });
        assert.deepEqual(
          Object.fromEntries(
            Object.entries(receipt.terminalChecks).map(([app, check]) => [
              app,
              [check.terminalCount, check.eof],
            ]),
          ),
          { claude: [1, true], codex: [1, true], gemini: [1, true] },
        );
        for (const field of oracle.receiptSchema.requiredFields) assert.ok(field in receipt, field);
        assertNoForbiddenReceiptFields(receipt);
        const serialized = `${result.stdout}\n${result.stderr}\n${JSON.stringify(receipt)}`;
        assert.doesNotMatch(serialized, new RegExp(serverSecret, "g"));
        assert.doesNotMatch(serialized, new RegExp(routerSecret, "g"));
        assert.doesNotMatch(serialized, /Call lookup with key qoder-acceptance/);
        assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 3);
        assert.equal(mock.seen.filter((entry) => entry.path === "/v1/messages").length, 2);
        assert.equal(mock.seen.filter((entry) => entry.path === "/v1/responses").length, 2);
        assert.equal(
          mock.seen.filter((entry) => /generateContent/i.test(entry.path)).length,
          2,
        );
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Qoder real harness reports missing inputs as blocked_inputs/live_pending", async () => {
  const result = await runScript("global_oauth", { RUN_REAL: "0" });
  assert.equal(result.code, 0, result.stderr);
  const output = JSON.parse(result.stdout);
  assert.equal(output.verificationState, "blocked_inputs");
  assert.equal(output.liveState, "live_pending");
  assert.ok(output.missingInputs.includes("RUN_REAL=1"));
  assert.ok(output.missingInputs.includes("QODER_REAL_RECEIPT_FILE"));
  assert.doesNotMatch(result.stdout, /live_verified/);
  assert.equal(result.stderr, "");
});

test("Qoder real harness fails closed on a mismatched bound Account before data-plane calls", async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "qoder-real-binding-"));
  const receiptFile = path.join(directory, "receipt.json");
  const mock = await startMockServer("cn_oauth", { bindingMismatch: true });
  try {
    const result = await runScript(
      "cn_oauth",
      fullEnv("cn_oauth", mock.url, receiptFile),
    );
    assert.equal(result.code, 1, result.stdout);
    assert.match(result.stderr, /not fixed to the selected Account generation/);
    assert.equal(mock.seen.filter((entry) => entry.path.startsWith("/v1/")).length, 0);
    assert.equal(fs.existsSync(receiptFile), false);
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("Qoder real harness never prints a token or raw failure body", async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "qoder-real-redaction-"));
  const receiptFile = path.join(directory, "receipt.json");
  const leaked = "pt-this-must-never-be-printed";
  const mock = await startMockServer("global_pat", {
    failAccountsWith: `Bearer ${serverSecret}; ${leaked}; raw body`,
  });
  try {
    const result = await runScript(
      "global_pat",
      fullEnv("global_pat", mock.url, receiptFile),
    );
    assert.equal(result.code, 1, result.stdout);
    assert.match(result.stderr, /^\[FAIL\]/m);
    const output = `${result.stdout}\n${result.stderr}`;
    assert.doesNotMatch(output, new RegExp(serverSecret, "g"));
    assert.doesNotMatch(output, new RegExp(leaked, "g"));
    assert.doesNotMatch(output, /raw body/);
    assert.equal(fs.existsSync(receiptFile), false);
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
