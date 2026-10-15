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
    shareEnv: "CC_SWITCH_QODER_GLOBAL_OAUTH_SHARE_ID",
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
    shareEnv: "CC_SWITCH_QODER_GLOBAL_PAT_SHARE_ID",
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
    shareEnv: "CC_SWITCH_QODER_CN_OAUTH_SHARE_ID",
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
    cleared[spec.shareEnv] = "";
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
  for (const value of values) {
    response.write(`data: ${value === "[DONE]" ? value : JSON.stringify(value)}\n\n`);
  }
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

function shareIdFor(rail) {
  return `${rail}-share`;
}

function providerView(rail, app, { bindingMismatch = false, scopeFault = "", revisionOffset = 0 } = {}) {
  const account = accountFor(rail);
  const fault = scopeFault.startsWith(`${app}_`) ? scopeFault.slice(app.length + 1) : "";
  const revision = { claude: 17, codex: 19, gemini: 23 }[app] + revisionOffset;
  return {
    app,
    revision: ["missing_view_revision", "legacy_top_level_only"].includes(fault)
      ? undefined : revision + (fault === "mismatched_revisions" ? 1 : 0),
    ...(fault === "legacy_top_level_only" ? { providerRevision: revision } : {}),
    providerType: "qoder_cosy",
    providerTypeId: "qoder_cosy",
    provider: { id: providerIdsFor(rail)[app] },
    runtime: {
      providerRevision: ["missing_runtime_revision", "legacy_top_level_only"].includes(fault)
        ? undefined : revision,
      driverId: "special.qoder_cosy",
      runtimeFingerprint: fault === "blank_runtime_fingerprint" ? "   " : `runtime-${rail}-${app}`,
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

async function startMockServer(
  rail,
  {
    bindingMismatch = false,
    failAccountsWith = "",
    postTerminalErrorApp = "",
    earlyDoneApp = "",
    finalDoneApp = "",
    invalidToolApp = "",
    invalidToolMode = "",
    fragmentedClaudeTool = false,
    driftAccountTokenGeneration = false,
    driftProviderRevision = false,
    driftShareRevision = false,
    shareFault = "",
    scopeFault = "",
    oversizedAccounts = false,
    accountSite = specs[rail].site,
    accountCredentialRail = rail === "global_pat" ? "pat_job_token" : rail,
    accountFault = null,
    omitAccountScope = false,
    expiringCatalogApp = "",
    expiringFetchedAtMs = 0,
    catalogDataByApp = null,
    finalCatalogFault = null,
    finalShareDelayMs = 0,
  } = {},
) {
  const account = accountFor(rail);
  const selectedAccount = { ...account };
  if (accountFault) {
    if (accountFault.variant === "missing") {
      delete selectedAccount[accountFault.field];
    } else {
      selectedAccount[accountFault.field] =
        accountFault.variant === "null" ? null : "false";
    }
  }
  const providerIds = providerIdsFor(rail);
  const seen = [];
  const reads = { accounts: 0, providers: 0, shares: 0 };
  const catalogReads = { claude: 0, codex: 0, gemini: 0 };
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
        reads.accounts += 1;
        if (oversizedAccounts) {
          response.writeHead(200, {
            "content-type": "application/json",
            ...(oversizedAccounts === "declared"
              ? { "content-length": String(8 * 1024 * 1024 + 1) }
              : {}),
          });
          response.write(
            oversizedAccounts === "declared" ? "{" : "x".repeat(8 * 1024 * 1024 + 1),
          );
          return;
        }
        if (failAccountsWith) {
          response.writeHead(500, { "content-type": "text/plain" });
          response.end(failAccountsWith);
          return;
        }
        sendJson(response, 200, {
          ok: true,
          accounts: [
            {
              ...selectedAccount,
              ...(!omitAccountScope && {
                qoderCredentialScope: {
                  site: accountSite,
                  credentialRail: accountCredentialRail,
                },
              }),
              tokenRefreshGeneration:
                driftAccountTokenGeneration && reads.accounts > 1
                  ? tokenRefreshGeneration + 1
                  : tokenRefreshGeneration,
            },
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
        reads.providers += 1;
        sendJson(response, 200, {
          ok: true,
          providers: [
            providerView(rail, "claude", { bindingMismatch, scopeFault }),
            providerView(rail, "codex", {
              bindingMismatch,
              scopeFault,
              revisionOffset: driftProviderRevision && reads.providers > 1 ? 1 : 0,
            }),
            providerView(rail, "gemini", { bindingMismatch, scopeFault }),
            {
              ...providerView(rail, "claude"),
              app: "claude",
              provider: { id: "decoy-provider" },
            },
          ],
        });
        return;
      }
      if (request.method === "GET" && url.pathname === "/api/shares") {
        reads.shares += 1;
        const payload = {
          ok: true,
          shares: [
            {
              id: shareIdFor(rail),
              enabled: shareFault === "disabled" ? false : shareFault === "missing_enabled" ? undefined : true,
              status: shareFault === "paused" ? "paused" : shareFault === "missing_status" ? undefined : "active",
              configRevision: scopeFault === "missing_share_revision" ? undefined
                : driftShareRevision && reads.shares > 1 ? 30 : 29,
              app: "claude",
              providerId: shareFault === "primary_conflict" ? "qoder-other-provider" : providerIds.claude,
              providerType: "qoder_cosy",
              bindings: [
                {
                  app: "claude",
                  providerId: providerIds.claude,
                  providerType: "qoder_cosy",
                },
                {
                  app: "codex",
                  providerId: providerIds.codex,
                  providerType: "qoder_cosy",
                },
                {
                  app: "gemini",
                  providerId: providerIds.gemini,
                  providerType: "qoder_cosy",
                },
                ...(["duplicate_claude", "duplicate_codex", "duplicate_gemini"].includes(shareFault)
                  ? [{
                      app: shareFault.slice("duplicate_".length),
                      providerId: "qoder-other-provider",
                      providerType: "qoder_cosy",
                    }]
                  : []),
              ],
            },
          ],
        };
        if (reads.shares > 1 && finalShareDelayMs > 0) {
          setTimeout(() => sendJson(response, 200, payload), finalShareDelayMs);
        } else {
          sendJson(response, 200, payload);
        }
        return;
      }
      if (request.method === "GET" && url.pathname === "/v1/models") {
        const app = url.searchParams.get("app");
        assert.equal(url.searchParams.get("providerId"), providerIds[app]);
        catalogReads[app] += 1;
        const catalogData = catalogDataByApp?.[app] ?? [
          { id: model, object: "model", owned_by: "qoder" },
        ];
        const fault = finalCatalogFault?.app === app && catalogReads[app] > 1
          ? finalCatalogFault.kind : "";
        let responseData = catalogData;
        if (fault === "remove") responseData = [];
        if (fault === "add") {
          responseData = [...catalogData, { id: "unexpected-model", object: "model", owned_by: "qoder" }];
        }
        if (fault === "reorder") responseData = [...catalogData].reverse();
        if (fault === "capability") {
          responseData = catalogData.map((entry) => entry.id === model
            ? { ...entry, capabilities: { tools: false } }
            : entry);
        }
        sendJson(response, 200, {
          object: "list",
          source: fault === "source" ? "untrusted_catalog" : "qoder_live_model_catalog",
          stale: fault === "stale",
          fetchedAtMs: fault === "future" ? Date.now() + 6 * 60_000
            : app === expiringCatalogApp ? expiringFetchedAtMs : Date.now(),
          data: responseData,
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
        const key = invalidToolApp === "claude" && invalidToolMode === (body.stream ? "stream" : "nonstream")
          ? "wrong-key"
          : "qoder-acceptance";
        if (body.stream) {
          sendSse(response, [
            { type: "message_start", message: { usage: { input_tokens: 3, output_tokens: 0 } } },
            {
              type: "content_block_start",
              index: 0,
              content_block: {
                type: "tool_use",
                id: "tool-claude",
                name: "lookup",
                input: fragmentedClaudeTool ? {} : { key },
              },
            },
            ...(fragmentedClaudeTool ? [
              { type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: '{"key":' } },
              { type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: JSON.stringify(key) + "}" } },
            ] : []),
            { type: "content_block_stop", index: 0 },
            { type: "message_delta", delta: { stop_reason: "tool_use" }, usage: { output_tokens: 1 } },
            { type: "message_stop" },
            ...(postTerminalErrorApp === "claude" ? [{ type: "error", error: { type: "api_error" } }] : []),
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
                input: { key },
              },
            ],
            usage: { input_tokens: 3, output_tokens: 1 },
          });
        }
        return;
      }
      if (request.method === "POST" && url.pathname === "/v1/responses") {
        assert.equal(body.model, model);
        const key = invalidToolApp === "codex" && invalidToolMode === (body.stream ? "stream" : "nonstream")
          ? "wrong-key"
          : "qoder-acceptance";
        const completed = {
          object: "response",
          status: "completed",
          output: [
            {
              type: "function_call",
              call_id: "call-codex",
              name: "lookup",
              arguments: JSON.stringify({ key }),
            },
          ],
          usage: { input_tokens: 3, output_tokens: 1, total_tokens: 4 },
        };
        if (body.stream) {
          sendSse(response, [
            { type: "response.output_item.added", item: completed.output[0] },
            ...(earlyDoneApp === "codex" ? ["[DONE]"] : []),
            { type: "response.completed", response: completed },
            ...(postTerminalErrorApp === "codex" ? [{ type: "response.failed" }] : []),
            ...(finalDoneApp === "codex" ? ["[DONE]"] : []),
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
        const key = invalidToolApp === "gemini" && invalidToolMode === (gemini[2] === "streamGenerateContent" ? "stream" : "nonstream")
          ? "wrong-key"
          : "qoder-acceptance";
        const payload = {
          candidates: [
            {
              content: {
                role: "model",
                parts: [
                  { functionCall: { name: "lookup", args: { key } } },
                ],
              },
              finishReason: "STOP",
            },
          ],
          usageMetadata: { promptTokenCount: 3, candidatesTokenCount: 1, totalTokenCount: 4 },
        };
        if (gemini[2] === "streamGenerateContent") {
          assert.equal(url.search, "?alt=sse");
          sendSse(response, [
            payload,
            ...(postTerminalErrorApp === "gemini" ? [{ error: { code: 500 } }] : []),
          ]);
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
    reads,
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
    [spec.shareEnv]: shareIdFor(rail),
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
        assert.equal(fs.statSync(receiptFile).mode & 0o777, 0o600);
        assert.deepEqual(fs.readdirSync(directory), ["receipt.json"]);
        assert.equal(receipt.schemaVersion, 2);
        assert.equal(receipt.harnessRevision, 2);
        assert.equal(receipt.verificationState, "contract_verified");
        assert.equal(receipt.liveState, "live_pending");
        assert.equal(receipt.acceptanceChecks.fresh_catalog, "pass");
        assert.equal(receipt.acceptanceChecks.unique_terminal_then_eof, "pass");
        for (const pending of [
          "login_or_pat_import",
          "refresh_rotation_or_job_token_exchange",
          "authoritative_empty_catalog",
          "same_account_first_401_recovery",
          "second_401_terminal",
        ]) {
          assert.equal(receipt.acceptanceChecks[pending], "not_observed", pending);
        }
        assert.equal(receipt.site, specs[rail].site);
        assert.match(receipt.targetCommit, /^[a-f0-9]{40}$/);
        assert.equal(receipt.commit, receipt.targetCommit);
        assert.match(receipt.scopeDigest, /^[a-f0-9]{64}$/);
        assert.match(receipt.shareIdentityDigest, /^[a-f0-9]{64}$/);
        assert.match(receipt.shareBindingDigest, /^[a-f0-9]{64}$/);
        assert.equal(receipt.shareRevision, 29);
        assert.deepEqual(
          Object.fromEntries(
            Object.entries(receipt.surfaceBindings).map(([app, binding]) => [
              app,
              [binding.providerRevision, /^[a-f0-9]{64}$/.test(binding.runtimeFingerprintDigest)],
            ]),
          ),
          { claude: [17, true], codex: [19, true], gemini: [23, true] },
        );
        assert.deepEqual(Object.keys(receipt.catalogs).sort(), ["claude", "codex", "gemini"]);
        for (const snapshot of Object.values(receipt.catalogs)) {
          assert.equal(snapshot.source, "qoder_live_model_catalog");
          assert.equal(snapshot.stale, false);
          assert.ok(Number.isSafeInteger(snapshot.fetchedAtMs));
          assert.match(snapshot.digest, /^[a-f0-9]{64}$/);
        }
        assert.deepEqual(Object.keys(receipt.bodyHashes).sort(), [
          "claude_nonstream",
          "claude_stream",
          "codex_nonstream",
          "codex_stream",
          "gemini_nonstream",
          "gemini_stream",
        ]);
        for (const value of Object.values(receipt.bodyHashes)) {
          assert.match(value, /^[a-f0-9]{64}$/);
        }
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
        assert.deepEqual(mock.reads, { accounts: 2, providers: 2, shares: 2 });
        const catalogRequests = mock.seen.filter((entry) => entry.path === "/v1/models");
        assert.equal(catalogRequests.length, 6);
        for (const app of ["claude", "codex", "gemini"]) {
          const scoped = catalogRequests.filter(
            (entry) => new URLSearchParams(entry.search).get("app") === app,
          );
          assert.equal(scoped.length, 2, `${rail}/${app} catalog count`);
          const query = new URLSearchParams(scoped[0].search);
          assert.equal(query.get("providerId"), providerIdsFor(rail)[app]);
          assert.equal(query.get("expectedShareId"), shareIdFor(rail));
          assert.equal(query.get("expectedProviderId"), providerIdsFor(rail)[app]);
        }
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

test("Qoder rails reject final catalog changes before private receipt publication", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const app of ["claude", "codex", "gemini"]) {
      for (const kind of ["remove", "add", "capability", "stale", "source", "future"]) {
        await t.test(`${rail}/${app}/${kind}`, async () => {
          const directory = fs.mkdtempSync(path.join(os.tmpdir(), `qoder-final-catalog-${rail}-`));
          const receiptFile = path.join(directory, "receipt.json");
          const mock = await startMockServer(rail, {
            finalCatalogFault: { app, kind },
          });
          try {
            const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
            assert.equal(result.code, 1, `${result.stdout}\n${result.stderr}`);
            assert.match(result.stderr, /^\[FAIL\]/m);
            assert.equal(fs.existsSync(receiptFile), false);
            assert.deepEqual(mock.reads, { accounts: 2, providers: 2, shares: 2 });
          } finally {
            await mock.close();
            fs.rmSync(directory, { recursive: true, force: true });
          }
        });
      }
    }
  }
});

test("Qoder rails reject invalid IDs in each complete App catalog", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const app of ["claude", "codex", "gemini"]) {
      for (const kind of ["missing", "blank", "duplicate", "nonstring"]) {
        await t.test(`${rail}/${app}/${kind}`, async () => {
          const directory = fs.mkdtempSync(path.join(os.tmpdir(), `qoder-catalog-id-${rail}-`));
          const receiptFile = path.join(directory, "receipt.json");
          const data = [
            { id: model, object: "model", owned_by: "qoder" },
            { id: "qoder-other", object: "model", owned_by: "qoder" },
            kind === "missing" ? { object: "model", owned_by: "qoder" }
              : { id: kind === "blank" ? " " : kind === "duplicate" ? "qoder-other" : 42, object: "model", owned_by: "qoder" },
          ];
          const mock = await startMockServer(rail, { catalogDataByApp: { [app]: data } });
          try {
            const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
            assert.equal(result.code, 1, `${result.stdout}\n${result.stderr}`);
            assert.equal(fs.existsSync(receiptFile), false);
            assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length,
              ["claude", "codex", "gemini"].indexOf(app) + 1);
          } finally {
            await mock.close();
            fs.rmSync(directory, { recursive: true, force: true });
          }
        });
      }
    }
  }
});

test("Qoder rails accept reordered model rows with identical catalog content", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const app of ["claude", "codex", "gemini"]) {
      await t.test(`${rail}/${app}`, async () => {
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `qoder-catalog-reorder-${rail}-`));
        const receiptFile = path.join(directory, "receipt.json");
        const mock = await startMockServer(rail, {
          catalogDataByApp: { [app]: [
            { id: model, object: "model", owned_by: "qoder" },
            { id: "qoder-other", object: "model", owned_by: "qoder" },
          ] },
          finalCatalogFault: { app, kind: "reorder" },
        });
        try {
          const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
          assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
          assert.match(result.stdout, /verificationState=contract_verified, liveState=live_pending/);
          assert.equal(fs.existsSync(receiptFile), true);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Qoder rails reject each surface catalog if it expires before receipt publication", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const app of ["claude", "codex", "gemini"]) {
      await t.test(`${rail}/${app}`, async () => {
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `qoder-expiring-${rail}-${app}-`));
        const receiptFile = path.join(directory, "receipt.json");
        const mock = await startMockServer(rail, {
          expiringCatalogApp: app,
          expiringFetchedAtMs: Date.now() - 2 * 60 * 60 * 1_000 + 1_200,
          finalShareDelayMs: 2_000,
        });
        try {
          const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.match(result.stderr, /^\[FAIL\]/m);
          assert.equal(fs.existsSync(receiptFile), false);
          assert.deepEqual(mock.reads, { accounts: 2, providers: 2, shares: 2 });
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Qoder rails reject catalogs that expire while writing the private receipt", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `qoder-slow-write-${rail}-`));
      const receiptFile = path.join(directory, "receipt.json");
      const preload = [
        'import fs from "node:fs";',
        'const original = fs.writeFileSync;',
        'fs.writeFileSync = (file, ...args) => {',
        '  const result = original(file, ...args);',
        '  const pathname = typeof file === "number"',
        '    ? fs.readlinkSync(`/proc/self/fd/${file}`)',
        '    : String(file);',
        '  if (pathname.endsWith(`.tmp-${process.pid}`)) {',
        '    const deadline = Date.now() + 2_000;',
        '    while (Date.now() < deadline) {}',
        '  }',
        '  return result;',
        '};',
      ].join("\n");
      const mock = await startMockServer(rail, {
        expiringCatalogApp: "claude",
        expiringFetchedAtMs: Date.now() - 2 * 60 * 60 * 1_000 + 1_200,
      });
      try {
        const result = await runScript(rail, {
          ...fullEnv(rail, mock.url, receiptFile),
          NODE_OPTIONS: `--import=data:text/javascript,${encodeURIComponent(preload)}`,
        });
        assert.equal(result.code, 1, `${result.stdout}\n${result.stderr}`);
        assert.match(result.stderr, /Qoder model catalogs expired during acceptance/);
        assert.equal(fs.existsSync(receiptFile), false);
        assert.deepEqual(fs.readdirSync(directory), []);
        assert.deepEqual(mock.reads, { accounts: 2, providers: 2, shares: 2 });
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Qoder private receipt output cannot traverse an outside symlink into the repository", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const insideDirectory = fs.mkdtempSync(path.join(repoRoot, ".qoder-receipt-inside-"));
      const outsideDirectory = fs.mkdtempSync(path.join(path.dirname(repoRoot), ".qoder-receipt-outside-"));
      const linkedDirectory = path.join(outsideDirectory, "receipt-dir");
      fs.symlinkSync(insideDirectory, linkedDirectory, "dir");
      const insideReceipt = path.join(insideDirectory, "receipt.json");
      const receiptFile = path.join(linkedDirectory, "receipt.json");
      const mock = await startMockServer(rail);
      try {
        const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /\[FAIL\]/);
        assert.equal(fs.existsSync(insideReceipt), false);
        assert.deepEqual(mock.seen, []);
      } finally {
        await mock.close();
        fs.rmSync(outsideDirectory, { recursive: true, force: true });
        fs.rmSync(insideDirectory, { recursive: true, force: true });
      }
    });
  }
});

test("Qoder receipt publication rejects a parent symlink swapped during its write", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const insideDirectory = fs.mkdtempSync(path.join(repoRoot, ".qoder-receipt-swap-inside-"));
      const outsideDirectory = fs.mkdtempSync(path.join(path.dirname(repoRoot), ".qoder-receipt-swap-outside-"));
      const safeDirectory = path.join(outsideDirectory, "safe");
      fs.mkdirSync(safeDirectory);
      const linkedDirectory = path.join(outsideDirectory, "receipt-dir");
      fs.symlinkSync(safeDirectory, linkedDirectory, "dir");
      const receiptFile = path.join(linkedDirectory, "receipt.json");
      const preload = [
        'import fs from "node:fs";',
        'const original = fs.writeFileSync;',
        `const linkedDirectory = ${JSON.stringify(linkedDirectory)};`,
        `const insideDirectory = ${JSON.stringify(insideDirectory)};`,
        'let swapped = false;',
        'fs.writeFileSync = (file, ...args) => {',
        '  const pathname = typeof file === "number"',
        '    ? fs.readlinkSync(`/proc/self/fd/${file}`)',
        '    : String(file);',
        '  if (!swapped && pathname.endsWith(`.tmp-${process.pid}`)) {',
        '    swapped = true;',
        '    fs.unlinkSync(linkedDirectory);',
        '    fs.symlinkSync(insideDirectory, linkedDirectory, "dir");',
        '  }',
        '  return original(file, ...args);',
        '};',
      ].join("\n");
      const mock = await startMockServer(rail);
      try {
        const result = await runScript(rail, {
          ...fullEnv(rail, mock.url, receiptFile),
          NODE_OPTIONS: `--import=data:text/javascript,${encodeURIComponent(preload)}`,
        });
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /\[FAIL\]/);
        assert.deepEqual(fs.readdirSync(insideDirectory), []);
        assert.deepEqual(fs.readdirSync(safeDirectory), []);
      } finally {
        await mock.close();
        fs.rmSync(outsideDirectory, { recursive: true, force: true });
        fs.rmSync(insideDirectory, { recursive: true, force: true });
      }
    });
  }
});

test("Qoder receipt publication removes its own link if the parent changes just after linking", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const insideDirectory = fs.mkdtempSync(path.join(repoRoot, ".qoder-receipt-postlink-inside-"));
      const outsideDirectory = fs.mkdtempSync(path.join(path.dirname(repoRoot), ".qoder-receipt-postlink-outside-"));
      const safeDirectory = path.join(outsideDirectory, "safe");
      fs.mkdirSync(safeDirectory);
      const linkedDirectory = path.join(outsideDirectory, "receipt-dir");
      fs.symlinkSync(safeDirectory, linkedDirectory, "dir");
      const receiptFile = path.join(linkedDirectory, "receipt.json");
      const preload = [
        'import fs from "node:fs";',
        'const original = fs.linkSync;',
        `const linkedDirectory = ${JSON.stringify(linkedDirectory)};`,
        `const insideDirectory = ${JSON.stringify(insideDirectory)};`,
        'fs.linkSync = (source, target) => {',
        '  const result = original(source, target);',
        '  fs.unlinkSync(linkedDirectory);',
        '  fs.symlinkSync(insideDirectory, linkedDirectory, "dir");',
        '  return result;',
        '};',
      ].join("\n");
      const mock = await startMockServer(rail);
      try {
        const result = await runScript(rail, {
          ...fullEnv(rail, mock.url, receiptFile),
          NODE_OPTIONS: `--import=data:text/javascript,${encodeURIComponent(preload)}`,
        });
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /\[FAIL\]/);
        assert.deepEqual(fs.readdirSync(insideDirectory), []);
        assert.deepEqual(fs.readdirSync(safeDirectory), []);
      } finally {
        await mock.close();
        fs.rmSync(outsideDirectory, { recursive: true, force: true });
        fs.rmSync(insideDirectory, { recursive: true, force: true });
      }
    });
  }
});

test("Qoder receipt publication rejects content changed during or just after linking", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const change of ["temporary_in_place", "temporary_replacement", "published_in_place", "published_replacement"]) {
      await t.test(`${rail}/${change}`, async () => {
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `qoder-receipt-content-${rail}-`));
        const receiptFile = path.join(directory, "receipt.json");
        const sentinel = "another private receipt\n";
        const preload = change.startsWith("published_")
          ? [
              'import fs from "node:fs";',
              'const originalLink = fs.linkSync;',
              'const originalWrite = fs.writeFileSync;',
              'fs.linkSync = (source, target) => {',
              '  const result = originalLink(source, target);',
              ...(change === "published_in_place"
                ? [
                    '  originalWrite(target, " ".repeat(fs.statSync(target).size), { flag: "r+" });',
                    '  fs.utimesSync(target, new Date(0), new Date(0));',
                  ]
                : [
                    '  const replacement = `${target}.replacement`;',
                    `  originalWrite(replacement, ${JSON.stringify(sentinel)}, { mode: 0o600 });`,
                    '  fs.renameSync(replacement, target);',
                  ]),
              '  return result;',
              '};',
            ].join("\n")
          : [
              'import fs from "node:fs";',
              'const originalWrite = fs.writeFileSync;',
              'fs.writeFileSync = (file, ...args) => {',
              '  const result = originalWrite(file, ...args);',
              '  const pathname = typeof file === "number"',
              '    ? fs.readlinkSync(`/proc/self/fd/${file}`)',
              '    : String(file);',
              '  if (pathname.endsWith(`.tmp-${process.pid}`)) {',
              ...(change === "temporary_in_place"
                ? [
                    '    originalWrite(pathname, " ".repeat(fs.statSync(pathname).size), { flag: "r+" });',
                    '    fs.utimesSync(pathname, new Date(0), new Date(0));',
                  ]
                : [
                    '    const replacement = `${pathname}.replacement`;',
                    '    originalWrite(replacement, "not the accepted receipt\\n", { mode: 0o600 });',
                    '    fs.renameSync(replacement, pathname);',
                  ]),
              '  }',
              '  return result;',
              '};',
            ].join("\n");
        const mock = await startMockServer(rail);
        try {
          const result = await runScript(rail, {
            ...fullEnv(rail, mock.url, receiptFile),
            NODE_OPTIONS: `--import=data:text/javascript,${encodeURIComponent(preload)}`,
          });
          assert.equal(result.code, 1, result.stdout);
          assert.match(result.stderr, /^\[FAIL\]/);
          if (change === "published_replacement") {
            assert.equal(fs.readFileSync(receiptFile, "utf8"), sentinel);
          } else if (change === "temporary_replacement") {
            const names = fs.readdirSync(directory);
            assert.equal(names.length, 1);
            assert.match(names[0], /^receipt\.json\.tmp-\d+$/);
            const temporary = path.join(directory, names[0]);
            assert.equal(fs.readFileSync(temporary, "utf8"), "not the accepted receipt\n");
          } else {
            assert.equal(fs.existsSync(receiptFile), false);
          }
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Qoder receipt publication never replaces a file created after its existence check", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `qoder-receipt-race-${rail}-`));
      const receiptFile = path.join(directory, "receipt.json");
      const sentinel = "preexisting-private-receipt\n";
      const preload = [
        'import fs from "node:fs";',
        'const original = fs.writeFileSync;',
        `const target = ${JSON.stringify(receiptFile)};`,
        'fs.writeFileSync = (file, ...args) => {',
        '  const result = original(file, ...args);',
        '  const pathname = typeof file === "number"',
        '    ? fs.readlinkSync(`/proc/self/fd/${file}`)',
        '    : String(file);',
        '  if (pathname.endsWith(`.tmp-${process.pid}`)) {',
        `    original(target, ${JSON.stringify(sentinel)}, { flag: "wx", mode: 0o600 });`,
        '  }',
        '  return result;',
        '};',
      ].join("\n");
      const mock = await startMockServer(rail);
      try {
        const result = await runScript(rail, {
          ...fullEnv(rail, mock.url, receiptFile),
          NODE_OPTIONS: `--import=data:text/javascript,${encodeURIComponent(preload)}`,
        });
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /\[FAIL\]/);
        assert.equal(fs.readFileSync(receiptFile, "utf8"), sentinel);
        assert.deepEqual(fs.readdirSync(directory), ["receipt.json"]);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Qoder real harness refuses receipts when a selected control-plane binding changes during acceptance", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const [kind, option] of [
      ["account token generation", "driftAccountTokenGeneration"],
      ["provider revision", "driftProviderRevision"],
      ["share revision", "driftShareRevision"],
    ]) {
      await t.test(`${rail}/${kind}`, async () => {
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `qoder-drift-${rail}-`));
        const receiptFile = path.join(directory, "receipt.json");
        const mock = await startMockServer(rail, { [option]: true });
        try {
          const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
          assert.equal(result.code, 1, `${result.stdout}\n${result.stderr}`);
          assert.match(result.stderr, /drifted during acceptance/);
          assert.equal(fs.existsSync(receiptFile), false);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Qoder rails reject ambiguous or inactive Shares before catalog access", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const shareFault of [
      "duplicate_claude",
      "duplicate_codex",
      "duplicate_gemini",
      "primary_conflict",
      "disabled",
      "paused",
      "missing_enabled",
      "missing_status",
    ]) {
      await t.test(`${rail}/${shareFault}`, async () => {
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `qoder-share-${rail}-`));
        const receiptFile = path.join(directory, "receipt.json");
        const mock = await startMockServer(rail, { shareFault });
        try {
          const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.match(result.stderr, /\[FAIL\]/);
          assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 0);
          assert.equal(fs.existsSync(receiptFile), false);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Qoder rails reject incomplete or inconsistent three-Surface revision scope before catalogs", async (t) => {
  const providerFaults = [
    "missing_view_revision",
    "missing_runtime_revision",
    "mismatched_revisions",
    "legacy_top_level_only",
    "blank_runtime_fingerprint",
  ];
  for (const rail of Object.keys(specs)) {
    for (const scopeFault of [
      ...["claude", "codex", "gemini"].flatMap((app) => providerFaults.map((fault) => `${app}_${fault}`)),
      "missing_share_revision",
    ]) {
      await t.test(`${rail}/${scopeFault}`, async () => {
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `qoder-scope-${rail}-`));
        const receiptFile = path.join(directory, "receipt.json");
        const mock = await startMockServer(rail, { scopeFault });
        try {
          const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.match(result.stderr, /\[FAIL\]/);
          assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 0);
          assert.equal(fs.existsSync(receiptFile), false);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
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
  assert.ok(output.missingInputs.includes("CC_SWITCH_QODER_GLOBAL_OAUTH_SHARE_ID"));
  assert.doesNotMatch(result.stdout, /live_verified/);
  assert.equal(result.stderr, "");
  const source = fs.readFileSync(script, "utf8");
  assert.match(source, /const liveState = "live_pending"/);
  assert.doesNotMatch(source, /const liveState = .*live_verified/);
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

test("Qoder receipt rejects a different bound site or credential rail", async (t) => {
  for (const [name, rail, options] of [
    ["site", "cn_oauth", { accountSite: "global" }],
    ["credential_rail", "global_oauth", { accountCredentialRail: "pat_job_token" }],
    ["missing_scope", "global_pat", { omitAccountScope: true }],
  ]) {
    await t.test(name, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `qoder-real-scope-${name}-`));
      const receiptFile = path.join(directory, "receipt.json");
      const mock = await startMockServer(rail, options);
      try {
        const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /^\[FAIL\]/m);
        assert.equal(mock.seen.filter((entry) => entry.path.startsWith("/v1/")).length, 0);
        assert.equal(fs.existsSync(receiptFile), false);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Qoder rails require explicit Account credential and relogin booleans", async (t) => {
  for (const rail of Object.keys(specs)) {
    const falseFields = rail === "global_pat"
      ? ["hasAccessToken", "hasRefreshToken", "needsRelogin"]
      : ["hasApiKey", "needsRelogin"];
    for (const field of falseFields) {
      for (const variant of ["missing", "null", "string"]) {
        await t.test(`${rail}/${field}/${variant}`, async () => {
          const directory = fs.mkdtempSync(path.join(os.tmpdir(), `qoder-account-${rail}-`));
          const receiptFile = path.join(directory, "receipt.json");
          const mock = await startMockServer(rail, {
            accountFault: { field, variant },
          });
          try {
            const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
            assert.equal(result.code, 1, result.stdout);
            assert.match(result.stderr, /^\[FAIL\]/m);
            assert.equal(mock.seen.filter((entry) => entry.path === "/api/providers").length, 0);
            assert.equal(mock.seen.filter((entry) => entry.path.startsWith("/v1/")).length, 0);
            assert.equal(fs.existsSync(receiptFile), false);
          } finally {
            await mock.close();
            fs.rmSync(directory, { recursive: true, force: true });
          }
        });
      }
    }
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

test("Qoder control-plane gate rejects declared oversized unfinished responses on every rail", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `qoder-oversized-${rail}-`));
      const receiptFile = path.join(directory, "receipt.json");
      const mock = await startMockServer(rail, { oversizedAccounts: "declared" });
      try {
        const startedAt = Date.now();
        const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /response-size bound/);
        assert.deepEqual(mock.seen.map((entry) => entry.path), ["/api/accounts"]);
        assert.equal(fs.existsSync(receiptFile), false);
        assert.ok(Date.now() - startedAt < 3_000, "oversized response waited for timeout");
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Qoder control-plane gate cancels an unfinished chunked body at its byte limit", async () => {
  const rail = "global_oauth";
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "qoder-oversized-chunked-"));
  const receiptFile = path.join(directory, "receipt.json");
  const mock = await startMockServer(rail, { oversizedAccounts: "chunked" });
  try {
    const startedAt = Date.now();
    const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
    assert.equal(result.code, 1, result.stdout);
    assert.match(result.stderr, /response-size bound/);
    assert.deepEqual(mock.seen.map((entry) => entry.path), ["/api/accounts"]);
    assert.equal(fs.existsSync(receiptFile), false);
    assert.ok(Date.now() - startedAt < 3_000, "oversized chunked body waited for timeout");
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("Qoder real harness rejects an error after a success terminal on every surface", async (t) => {
  for (const app of ["claude", "codex", "gemini"]) {
    await t.test(app, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `qoder-terminal-${app}-`));
      const receiptFile = path.join(directory, "receipt.json");
      const mock = await startMockServer("global_oauth", { postTerminalErrorApp: app });
      try {
        const result = await runScript(
          "global_oauth",
          fullEnv("global_oauth", mock.url, receiptFile),
        );
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /stream reported an error/);
        assert.equal(fs.existsSync(receiptFile), false);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Qoder real harness rejects a premature DONE marker before terminal", async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "qoder-early-done-"));
  const receiptFile = path.join(directory, "receipt.json");
  const mock = await startMockServer("global_oauth", { earlyDoneApp: "codex" });
  try {
    const result = await runScript(
      "global_oauth",
      fullEnv("global_oauth", mock.url, receiptFile),
    );
    assert.equal(result.code, 1, result.stdout);
    assert.match(result.stderr, /premature DONE marker/);
    assert.equal(fs.existsSync(receiptFile), false);
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("Qoder real harness accepts one final DONE marker after Codex completion", async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "qoder-final-done-"));
  const receiptFile = path.join(directory, "receipt.json");
  const mock = await startMockServer("global_oauth", { finalDoneApp: "codex" });
  try {
    const result = await runScript(
      "global_oauth",
      fullEnv("global_oauth", mock.url, receiptFile),
    );
    assert.equal(result.code, 0, result.stderr);
    assert.equal(JSON.parse(fs.readFileSync(receiptFile, "utf8")).liveState, "live_pending");
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("Qoder real harness rejects wrong tool arguments on every surface and mode", async (t) => {
  for (const app of ["claude", "codex", "gemini"]) {
    for (const mode of ["nonstream", "stream"]) {
      await t.test(`${app}/${mode}`, async () => {
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `qoder-tool-${app}-${mode}-`));
        const receiptFile = path.join(directory, "receipt.json");
        const mock = await startMockServer("global_oauth", {
          invalidToolApp: app,
          invalidToolMode: mode,
        });
        try {
          const result = await runScript(
            "global_oauth",
            fullEnv("global_oauth", mock.url, receiptFile),
          );
          assert.equal(result.code, 1, result.stdout);
          assert.match(result.stderr, /tool.*arguments/i);
          assert.equal(fs.existsSync(receiptFile), false);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Qoder real harness accepts a fragmented Claude tool argument round-trip", async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "qoder-tool-fragmented-"));
  const receiptFile = path.join(directory, "receipt.json");
  const mock = await startMockServer("global_oauth", { fragmentedClaudeTool: true });
  try {
    const result = await runScript(
      "global_oauth",
      fullEnv("global_oauth", mock.url, receiptFile),
    );
    assert.equal(result.code, 0, result.stderr);
    assert.equal(JSON.parse(fs.readFileSync(receiptFile, "utf8")).liveState, "live_pending");
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
