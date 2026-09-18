import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { execFileSync } from "node:child_process";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const script = path.join(repoRoot, "scripts/smoke/antigravity-real.mjs");
const contract = JSON.parse(
  fs.readFileSync(
    path.join(repoRoot, "assets/contract/antigravity-reference-delta.json"),
    "utf8",
  ),
);
const targetCommit = execFileSync("git", ["rev-parse", "HEAD"], {
  cwd: repoRoot,
  encoding: "utf8",
}).trim();
const serverSecret = "antigravity-server-secret-for-harness";
const routerSecret = "antigravity-router-secret-for-harness";

const specs = Object.freeze({
  antigravity_oauth: Object.freeze({
    providerType: "antigravity_oauth",
    driverId: "special.antigravity",
    accountEnv: "ANTIGRAVITY_OAUTH_TEST_ACCOUNT",
    shareEnv: "CC_SWITCH_ANTIGRAVITY_OAUTH_SHARE_ID",
    providerEnvs: Object.freeze({
      claude: "CC_SWITCH_ANTIGRAVITY_OAUTH_CLAUDE_PROVIDER_ID",
      gemini: "CC_SWITCH_ANTIGRAVITY_OAUTH_GEMINI_PROVIDER_ID",
    }),
    modelEnvs: Object.freeze({
      claude: "CC_SWITCH_ANTIGRAVITY_OAUTH_CLAUDE_MODEL",
      gemini: "CC_SWITCH_ANTIGRAVITY_OAUTH_GEMINI_MODEL",
    }),
  }),
  agy_oauth: Object.freeze({
    providerType: "agy_oauth",
    driverId: "special.agy",
    accountEnv: "AGY_OAUTH_TEST_ACCOUNT",
    shareEnv: "CC_SWITCH_AGY_OAUTH_SHARE_ID",
    providerEnvs: Object.freeze({
      claude: "CC_SWITCH_AGY_OAUTH_CLAUDE_PROVIDER_ID",
      gemini: "CC_SWITCH_AGY_OAUTH_GEMINI_PROVIDER_ID",
    }),
    modelEnvs: Object.freeze({
      claude: "CC_SWITCH_AGY_OAUTH_CLAUDE_MODEL",
      gemini: "CC_SWITCH_AGY_OAUTH_GEMINI_MODEL",
    }),
  }),
});

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, nested]) => [key, canonical(nested)]),
    );
  }
  return value;
}

function digest(domain, value) {
  return crypto
    .createHash("sha256")
    .update(`${domain}\0`)
    .update(JSON.stringify(canonical(value)))
    .digest("hex");
}

function values(rail) {
  return {
    accountId: `${rail}-account`,
    shareId: `${rail}-share`,
    providerIds: {
      claude: `${rail}-claude-provider`,
      gemini: `${rail}-gemini-provider`,
    },
    models: {
      claude: "claude-sonnet-4-6",
      gemini: "gemini-3.5-flash-medium",
    },
    authIdentityGeneration: rail === "antigravity_oauth" ? 7 : 11,
    tokenRefreshGeneration: rail === "antigravity_oauth" ? 3 : 5,
    shareRevision: 13,
  };
}

function bindings(rail) {
  const value = values(rail);
  return Object.entries(value.providerIds).map(([app, providerId], index) => ({
    app,
    providerId,
    providerRevision: index + 3,
    runtimeFingerprint: `${rail}-${app}-runtime`,
    authIdentityGeneration: value.authIdentityGeneration,
  }));
}

function catalogs(rail) {
  const value = values(rail);
  return Object.entries(value.providerIds).map(([app]) => ({
    app,
    model: value.models[app],
    source: "authenticated_fetch_available_models",
    stale: false,
  }));
}

function scopeDigest(rail) {
  const value = values(rail);
  return digest("cc-switch-server:antigravity-real-scope:v1", {
    rail,
    accountId: value.accountId,
    authIdentityGeneration: value.authIdentityGeneration,
    tokenRefreshGeneration: value.tokenRefreshGeneration,
    providerIds: value.providerIds,
    shareId: value.shareId,
    models: value.models,
    bindings: bindings(rail),
    shareRevision: value.shareRevision,
    catalogs: catalogs(rail),
    targetCommit,
    harnessRevision: 1,
  });
}

function receipt(rail, overrides = {}) {
  const value = values(rail);
  return {
    schemaVersion: 1,
    providerFamily: "antigravity",
    rail,
    verificationState: "contract_verified",
    liveState: "live_pending",
    targetCommit,
    harnessRevision: 1,
    recordedAt: "2026-09-18T00:00:00.000Z",
    scopeDigest: scopeDigest(rail),
    models: value.models,
    requestTypeDecision: {
      ordinary: "agent",
      webSearch: "web_search",
      mixedToolsPreserved: true,
    },
    checks: Object.fromEntries(
      contract.realAcceptance.requiredChecks.map((check) => [check, "pass"]),
    ),
    bodyHashes: Object.fromEntries(
      contract.realAcceptance.requiredBodyHashes.map((name) => [
        name,
        digest(`antigravity-fixture:${rail}`, name),
      ]),
    ),
    decisions: {
      sameAccount401: "replayed_once",
      second401: "terminal",
      structured429: "bounded_exact_scope",
      compaction: "runtime_disabled",
    },
    generations: {
      authIdentityGeneration: value.authIdentityGeneration,
      tokenRefreshGeneration: value.tokenRefreshGeneration,
      providerBindingDigest: digest(
        "cc-switch-server:antigravity-provider-bindings:v1",
        bindings(rail),
      ),
      shareRevision: value.shareRevision,
    },
    surfaceMatrix: {
      claude: {
        nonstream: "pass",
        stream: "pass",
        tool: "pass",
        usage: "pass",
        terminal: "pass",
      },
      gemini: {
        nonstream: "pass",
        stream: "pass",
        tool: "pass",
        usage: "pass",
        terminal: "pass",
      },
    },
    decoyRequests: { otherAccount: 0, otherProvider: 0, otherRail: 0 },
    sensitiveScan: { status: "pass", matches: 0 },
    ...overrides,
  };
}

function writeReceipt(file, rail, overrides = {}) {
  fs.writeFileSync(file, `${JSON.stringify(receipt(rail, overrides), null, 2)}\n`, {
    mode: 0o600,
  });
}

function runScript(rail, overrides = {}) {
  const cleared = {};
  for (const spec of Object.values(specs)) {
    cleared[spec.accountEnv] = "";
    cleared[spec.shareEnv] = "";
    for (const name of Object.values(spec.providerEnvs)) cleared[name] = "";
    for (const name of Object.values(spec.modelEnvs)) cleared[name] = "";
  }
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [script, "--rail", rail], {
      cwd: repoRoot,
      env: {
        ...process.env,
        ...cleared,
        RUN_REAL: "1",
        CC_SWITCH_ANTIGRAVITY_HARNESS_MODE: "fixture",
        SERVER_URL: "",
        CC_SWITCH_SERVER_TOKEN: "",
        CC_SWITCH_SHARE_URL: "",
        ROUTER_API_TOKEN: "",
        ROUTER_API_TOKEN_HEADER: "Authorization",
        ANTIGRAVITY_REAL_RECEIPT_FILE: "",
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

function sendJson(response, status, value) {
  response.writeHead(status, { "content-type": "application/json" });
  response.end(JSON.stringify(value));
}

async function startMockServer(rail, { providerRail = rail, failProvidersWith = "" } = {}) {
  const selected = specs[providerRail];
  const value = values(rail);
  const seen = [];
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    seen.push({
      path: url.pathname,
      search: url.search,
      authorization: request.headers.authorization || "",
    });
    if (request.method === "GET" && url.pathname === "/api/accounts") {
      sendJson(response, 200, {
        ok: true,
        accounts: [
          {
            id: value.accountId,
            email: `${rail}@example.test`,
            providerType: selected.providerType,
            authIdentityGeneration: value.authIdentityGeneration,
            tokenRefreshGeneration: value.tokenRefreshGeneration,
            hasAccessToken: true,
            hasRefreshToken: true,
            hasApiKey: false,
            needsRelogin: false,
          },
        ],
      });
      return;
    }
    if (request.method === "GET" && url.pathname === "/api/providers") {
      if (failProvidersWith) {
        response.writeHead(500, { "content-type": "text/plain" });
        response.end(failProvidersWith);
        return;
      }
      sendJson(response, 200, {
        ok: true,
        providers: Object.entries(value.providerIds).map(([app, providerId], index) => ({
          app,
          providerType: selected.providerType,
          providerTypeId: selected.providerType,
          providerRevision: index + 3,
          provider: { id: providerId },
          runtime: {
            driverId: selected.driverId,
            configurationState: "ready",
            runtimeFingerprint: `${rail}-${app}-runtime`,
            authRef: {
              kind: "managed_account",
              accountId: value.accountId,
              expectedProviderType: selected.providerType,
              authIdentityGeneration: value.authIdentityGeneration,
            },
          },
        })),
      });
      return;
    }
    if (request.method === "GET" && url.pathname === "/api/shares") {
      sendJson(response, 200, {
        ok: true,
        shares: [
          {
            id: value.shareId,
            app: "claude",
            providerId: value.providerIds.claude,
            providerType: selected.providerType,
            configRevision: value.shareRevision,
            bindings: [
              {
                app: "gemini",
                providerId: value.providerIds.gemini,
                providerType: selected.providerType,
              },
            ],
          },
        ],
      });
      return;
    }
    if (request.method === "GET" && url.pathname === "/v1/models") {
      const app = url.searchParams.get("app");
      assert.ok(app === "claude" || app === "gemini");
      assert.equal(url.searchParams.get("providerId"), value.providerIds[app]);
      sendJson(response, 200, {
        object: "list",
        source: "authenticated_fetch_available_models",
        stale: false,
        fetchedAtMs: 1_789_776_000_000,
        data: [{ id: value.models[app], object: "model", owned_by: "antigravity" }],
      });
      return;
    }
    sendJson(response, 404, { error: "unhandled fixture route" });
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
  const value = values(rail);
  const result = {
    SERVER_URL: url,
    CC_SWITCH_SERVER_TOKEN: serverSecret,
    CC_SWITCH_SHARE_URL: url,
    ROUTER_API_TOKEN: routerSecret,
    ANTIGRAVITY_REAL_RECEIPT_FILE: receiptFile,
    [spec.accountEnv]: value.accountId,
    [spec.shareEnv]: value.shareId,
  };
  for (const [app, name] of Object.entries(spec.providerEnvs)) {
    result[name] = value.providerIds[app];
  }
  for (const [app, name] of Object.entries(spec.modelEnvs)) {
    result[name] = value.models[app];
  }
  return result;
}

test("Antigravity real harness keeps Antigravity and Agy receipts independent", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `${rail}-real-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, rail);
      const mock = await startMockServer(rail);
      try {
        const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
        assert.equal(result.code, 0, result.stderr);
        assert.match(result.stdout, new RegExp(`Antigravity ${rail} private receipt accepted`));
        assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 2);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Antigravity real harness reports missing inputs without promoting live state", async () => {
  const result = await runScript("antigravity_oauth");
  assert.equal(result.code, 0, result.stderr);
  const output = JSON.parse(result.stdout);
  assert.equal(output.rail, "antigravity_oauth");
  assert.equal(output.verificationState, "blocked_inputs");
  assert.equal(output.liveState, "live_pending");
  assert.ok(output.missingInputs.includes("SERVER_URL"));
});

test("Antigravity real harness rejects a cross-rail receipt", async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "antigravity-cross-rail-"));
  const receiptFile = path.join(directory, "receipt.json");
  writeReceipt(receiptFile, "agy_oauth");
  const mock = await startMockServer("antigravity_oauth");
  try {
    const result = await runScript(
      "antigravity_oauth",
      fullEnv("antigravity_oauth", mock.url, receiptFile),
    );
    assert.equal(result.code, 1, result.stdout);
    assert.match(result.stderr, /^\[FAIL\] Antigravity acceptance failed \(details redacted\)$/m);
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("Antigravity real harness fails before Share calls on a Provider rail mismatch", async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "antigravity-provider-rail-"));
  const receiptFile = path.join(directory, "receipt.json");
  writeReceipt(receiptFile, "antigravity_oauth");
  const mock = await startMockServer("antigravity_oauth", { providerRail: "agy_oauth" });
  try {
    const result = await runScript(
      "antigravity_oauth",
      fullEnv("antigravity_oauth", mock.url, receiptFile),
    );
    assert.equal(result.code, 1, result.stdout);
    assert.equal(mock.seen.filter((entry) => entry.path === "/api/shares").length, 0);
    assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 0);
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("Antigravity real harness never prints secrets or a raw failure body", async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "antigravity-redaction-"));
  const receiptFile = path.join(directory, "receipt.json");
  writeReceipt(receiptFile, "agy_oauth");
  const leaked = "ya29.this-must-never-be-printed";
  const mock = await startMockServer("agy_oauth", {
    failProvidersWith: `Bearer ${serverSecret}; ${routerSecret}; ${leaked}; raw body`,
  });
  try {
    const result = await runScript("agy_oauth", fullEnv("agy_oauth", mock.url, receiptFile));
    assert.equal(result.code, 1, result.stdout);
    const output = `${result.stdout}\n${result.stderr}`;
    assert.doesNotMatch(output, new RegExp(serverSecret, "g"));
    assert.doesNotMatch(output, new RegExp(routerSecret, "g"));
    assert.doesNotMatch(output, /ya29\./);
    assert.doesNotMatch(output, /raw body/);
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
