import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import test from "node:test";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const script = path.join(repoRoot, "scripts/smoke/cursor-real.mjs");
const contract = JSON.parse(
  fs.readFileSync(
    path.join(repoRoot, "assets/contract/cursor-reference-delta.json"),
    "utf8",
  ),
);
const targetCommit = execFileSync("git", ["rev-parse", "HEAD"], {
  cwd: repoRoot,
  encoding: "utf8",
}).trim();
const serverSecret = "cursor-server-secret-for-harness";
const routerSecret = "cursor-router-secret-for-harness";
const model = "composer-2.5-fast";
const app = "codex";
const specs = Object.freeze({
  oauth: Object.freeze({
    providerType: "cursor_oauth",
    providerEnv: "CC_SWITCH_CURSOR_OAUTH_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CURSOR_OAUTH_SHARE_ID",
    modelEnv: "CC_SWITCH_CURSOR_OAUTH_MODEL",
    receiptEnv: "CURSOR_OAUTH_REAL_RECEIPT_FILE",
    accountEnv: "CURSOR_OAUTH_TEST_ACCOUNT",
    authKind: "managed_account",
    generation: 7,
    tokenRefreshGeneration: 2,
  }),
  api_key: Object.freeze({
    providerType: "cursor_apikey",
    providerEnv: "CC_SWITCH_CURSOR_API_KEY_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CURSOR_API_KEY_SHARE_ID",
    modelEnv: "CC_SWITCH_CURSOR_API_KEY_MODEL",
    receiptEnv: "CURSOR_API_KEY_REAL_RECEIPT_FILE",
    accountEnv: null,
    authKind: "static_credential",
    generation: 11,
    tokenRefreshGeneration: null,
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

function ids(rail) {
  return {
    providerId: `cursor-${rail}-provider`,
    shareId: `cursor-${rail}-share`,
    accountId: rail === "oauth" ? "cursor-oauth-account" : null,
  };
}

function binding(rail) {
  const spec = specs[rail];
  return {
    providerRevision: 3,
    runtimeFingerprint: `cursor-${rail}-runtime`,
    shareRevision: 5,
    authIdentityGeneration: rail === "oauth" ? spec.generation : null,
    tokenRefreshGeneration:
      rail === "oauth" ? spec.tokenRefreshGeneration : null,
    credentialGeneration: rail === "api_key" ? spec.generation : null,
  };
}

function scopeDigest(rail) {
  const value = ids(rail);
  const current = binding(rail);
  return digest("cc-switch-server:cursor-real-scope:v2", {
    rail,
    targetCommit,
    harnessRevision: 2,
    app,
    providerId: value.providerId,
    providerRevision: current.providerRevision,
    runtimeFingerprint: current.runtimeFingerprint,
    shareId: value.shareId,
    shareRevision: current.shareRevision,
    accountId: value.accountId,
    authIdentityGeneration: current.authIdentityGeneration,
    tokenRefreshGeneration: current.tokenRefreshGeneration,
    credentialGeneration: current.credentialGeneration,
    model,
  });
}

function providerBindingDigest(rail) {
  const value = ids(rail);
  const current = binding(rail);
  return digest("cc-switch-server:cursor-provider-binding:v2", {
    rail,
    app,
    providerId: value.providerId,
    runtimeFingerprint: current.runtimeFingerprint,
    accountId: value.accountId,
    authIdentityGeneration: current.authIdentityGeneration,
    credentialGeneration: current.credentialGeneration,
  });
}

function expectedDecisions() {
  return {
    sameIdentity401: "replayed_once",
    second401: "terminal",
    identityGenerationDrift: "terminal",
    crossRailFallback: "disabled",
    crossAccountFallback: "disabled",
    crossProviderFallback: "disabled",
    crossShareFallback: "disabled",
    postCommitReplay: "disabled",
    staleCatalogWireAuthorization: "disabled",
  };
}

function buildReceipt(rail) {
  const spec = specs[rail];
  const current = binding(rail);
  const railContract = contract.realAcceptance.rails.find(
    (candidate) => candidate.rail === rail,
  );
  assert.ok(railContract);
  return {
    schemaVersion: 2,
    providerFamily: "cursor",
    rail,
    verificationState: "contract_verified",
    liveState: "live_pending",
    targetCommit,
    harnessRevision: 2,
    recordedAt: new Date().toISOString(),
    scopeDigest: scopeDigest(rail),
    app,
    model,
    checks: Object.fromEntries(
      railContract.requiredChecks.map((check) => [check, "pass"]),
    ),
    bodyHashes: Object.fromEntries(
      railContract.requiredBodyHashes.map((name) => [
        name,
        digest("cc-switch-server:cursor-fixture-body:v1", { rail, name }),
      ]),
    ),
    generations: {
      ...(rail === "oauth"
        ? {
            authIdentityGeneration: spec.generation,
            tokenRefreshGeneration: spec.tokenRefreshGeneration,
          }
        : { credentialGeneration: spec.generation }),
      providerRevision: current.providerRevision,
      shareRevision: current.shareRevision,
      providerBindingDigest: providerBindingDigest(rail),
    },
    measurements: {
      surfaceRuns: 6,
      catalogRuns: 3,
      toolRuns: 1,
      imageRuns: 1,
      parkResumeRuns: 1,
    },
    decisions: expectedDecisions(),
    decoyRequests: {
      otherRail: 0,
      otherAccount: 0,
      otherProvider: 0,
      otherShare: 0,
    },
    sensitiveScan: { status: "pass", matches: 0 },
  };
}

function writeReceipt(file, rail, mutate = () => {}) {
  const receipt = buildReceipt(rail);
  mutate(receipt);
  fs.writeFileSync(file, `${JSON.stringify(receipt, null, 2)}\n`, {
    mode: 0o600,
  });
}

function runScript(rail, overrides = {}) {
  const cleared = {
    CC_SWITCH_CURSOR_OAUTH_PROVIDER_ID: "",
    CC_SWITCH_CURSOR_OAUTH_SHARE_ID: "",
    CC_SWITCH_CURSOR_OAUTH_MODEL: "",
    CURSOR_OAUTH_REAL_RECEIPT_FILE: "",
    CURSOR_OAUTH_PROVIDER_ID: "",
    CURSOR_OAUTH_SHARE_ID: "",
    CURSOR_OAUTH_TEST_ACCOUNT: "",
    CC_SWITCH_CURSOR_API_KEY_PROVIDER_ID: "",
    CC_SWITCH_CURSOR_API_KEY_SHARE_ID: "",
    CC_SWITCH_CURSOR_API_KEY_MODEL: "",
    CURSOR_API_KEY_REAL_RECEIPT_FILE: "",
    CURSOR_API_KEY_PROVIDER_ID: "",
    CURSOR_API_KEY_SHARE_ID: "",
    CURSOR_REAL_FAST_MODEL: "",
    CURSOR_REAL_RECEIPT_FILE: "",
  };
  const args = rail === null ? [] : ["--rail", rail];
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [script, ...args], {
      cwd: repoRoot,
      env: {
        ...process.env,
        ...cleared,
        RUN_REAL: "1",
        CC_SWITCH_CURSOR_HARNESS_MODE: "fixture",
        SERVER_URL: "",
        CC_SWITCH_SERVER_TOKEN: "",
        CC_SWITCH_SHARE_URL: "",
        ROUTER_API_TOKEN: "",
        ROUTER_API_TOKEN_HEADER: "Authorization",
        CURSOR_REAL_PROVIDER_APP: app,
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
    child.once("close", (code, signal) =>
      resolve({ code, signal, stdout, stderr }),
    );
  });
}

function sendJson(response, status, value) {
  response.writeHead(status, { "content-type": "application/json" });
  response.end(JSON.stringify(value));
}

async function startMockServer(
  rail,
  { providerRail = rail, failProvidersWith = "" } = {},
) {
  const spec = specs[providerRail];
  const value = ids(rail);
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
            email: "cursor-fixture@example.test",
            providerType: "cursor_oauth",
            authIdentityGeneration: specs.oauth.generation,
            tokenRefreshGeneration: specs.oauth.tokenRefreshGeneration,
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
      const authRef =
        providerRail === "oauth"
          ? {
              kind: spec.authKind,
              accountId: value.accountId,
              expectedProviderType: spec.providerType,
              authIdentityGeneration: spec.generation,
            }
          : {
              kind: spec.authKind,
              authScheme: "bearer",
              slots: ["apiKey"],
              credentialGeneration: spec.generation,
            };
      sendJson(response, 200, {
        ok: true,
        providers: [
          {
            app,
            providerType: spec.providerType,
            providerTypeId: spec.providerType,
            revision: 3,
            provider: { id: value.providerId },
            runtime: {
              providerRevision: 3,
              driverId: "special.cursor",
              configurationState: "ready",
              runtimeFingerprint: `cursor-${providerRail}-runtime`,
              authRef,
            },
          },
        ],
      });
      return;
    }
    if (request.method === "GET" && url.pathname === "/api/shares") {
      sendJson(response, 200, {
        ok: true,
        shares: [
          {
            id: value.shareId,
            app,
            providerId: value.providerId,
            providerType: spec.providerType,
            configRevision: 5,
            bindings: [],
          },
        ],
      });
      return;
    }
    if (request.method === "GET" && url.pathname === "/v1/models") {
      assert.equal(url.searchParams.get("app"), app);
      assert.equal(url.searchParams.get("providerId"), value.providerId);
      sendJson(response, 200, {
        object: "list",
        data: [{ id: model, object: "model", owned_by: "cursor" }],
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
  const value = ids(rail);
  return {
    SERVER_URL: url,
    CC_SWITCH_SERVER_TOKEN: serverSecret,
    CC_SWITCH_SHARE_URL: url,
    ROUTER_API_TOKEN: routerSecret,
    [spec.providerEnv]: value.providerId,
    [spec.shareEnv]: value.shareId,
    [spec.modelEnv]: model,
    [spec.receiptEnv]: receiptFile,
    ...(spec.accountEnv ? { [spec.accountEnv]: value.accountId } : {}),
  };
}

test("missing Cursor rail stays blocked without network access", async () => {
  const result = await runScript(null, { RUN_REAL: "0" });
  assert.equal(result.code, 0, result.stderr);
  const output = JSON.parse(result.stdout);
  assert.equal(output.verificationState, "blocked_inputs");
  assert.equal(output.liveState, "live_pending");
  assert.deepEqual(output.missingInputs, ["--rail oauth|api_key"]);
  assert.equal(result.stderr, "");
});

test("Cursor rail inputs stay blocked before any network access", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const result = await runScript(rail, { RUN_REAL: "0" });
      assert.equal(result.code, 0, result.stderr);
      const output = JSON.parse(result.stdout);
      assert.equal(output.rail, rail);
      assert.equal(output.verificationState, "blocked_inputs");
      assert.equal(output.liveState, "live_pending");
      assert.ok(output.missingInputs.includes("RUN_REAL=1"));
      assert.ok(output.missingInputs.includes(specs[rail].receiptEnv));
      assert.doesNotMatch(result.stdout, /live_verified/);
      assert.equal(result.stderr, "");
    });
  }
});

test("Cursor OAuth and API-key receipts remain independently live_pending", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const directory = fs.mkdtempSync(
        path.join(os.tmpdir(), `cursor-real-${rail}-`),
      );
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, rail);
      const mock = await startMockServer(rail);
      try {
        const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
        assert.equal(result.signal, null);
        assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
        assert.match(
          result.stdout,
          /verificationState=contract_verified, liveState=live_pending/,
        );
        assert.equal(result.stderr, "");
        assert.equal(
          mock.seen.filter((entry) => entry.path === "/api/accounts").length,
          rail === "oauth" ? 1 : 0,
        );
        assert.equal(
          mock.seen.filter((entry) => entry.path === "/api/providers").length,
          1,
        );
        assert.equal(
          mock.seen.filter((entry) => entry.path === "/api/shares").length,
          1,
        );
        assert.equal(
          mock.seen.filter((entry) => entry.path === "/v1/models").length,
          1,
        );
        const output = `${result.stdout}\n${result.stderr}`;
        assert.doesNotMatch(output, new RegExp(serverSecret, "g"));
        assert.doesNotMatch(output, new RegExp(routerSecret, "g"));
        assert.doesNotMatch(output, new RegExp(ids(rail).providerId, "g"));
        assert.doesNotMatch(
          output,
          new RegExp(ids(rail).accountId || "impossible-account", "g"),
        );
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Cursor receipt identity, generations, checks, and decisions fail closed", async (t) => {
  const mutations = [
    ["rail", (receipt) => (receipt.rail = "api_key")],
    ["scope", (receipt) => (receipt.scopeDigest = "0".repeat(64))],
    ["target", (receipt) => (receipt.targetCommit = "0".repeat(40))],
    [
      "generation",
      (receipt) => (receipt.generations.authIdentityGeneration += 1),
    ],
    ["check", (receipt) => delete receipt.checks.terminal_usage],
    ["body-hash", (receipt) => (receipt.bodyHashes.catalog = "invalid")],
    ["measurement", (receipt) => (receipt.measurements.surfaceRuns = 5)],
    [
      "decision",
      (receipt) => (receipt.decisions.crossRailFallback = "enabled"),
    ],
  ];
  for (const [name, mutate] of mutations) {
    await t.test(name, async () => {
      const directory = fs.mkdtempSync(
        path.join(os.tmpdir(), `cursor-real-${name}-`),
      );
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, "oauth", mutate);
      const mock = await startMockServer("oauth");
      try {
        const result = await runScript(
          "oauth",
          fullEnv("oauth", mock.url, receiptFile),
        );
        assert.equal(result.code, 1, result.stdout);
        assert.match(
          result.stderr,
          /^\[FAIL\] Cursor acceptance failed \(details redacted\)$/m,
        );
        assert.doesNotMatch(result.stderr, /scope|rail|account|provider|generation/i);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Cursor Provider rail mismatch fails before Share and catalog calls", async () => {
  const directory = fs.mkdtempSync(
    path.join(os.tmpdir(), "cursor-real-provider-rail-"),
  );
  const receiptFile = path.join(directory, "receipt.json");
  writeReceipt(receiptFile, "oauth");
  const mock = await startMockServer("oauth", { providerRail: "api_key" });
  try {
    const result = await runScript(
      "oauth",
      fullEnv("oauth", mock.url, receiptFile),
    );
    assert.equal(result.code, 1, result.stdout);
    assert.equal(
      mock.seen.filter((entry) => entry.path === "/api/shares").length,
      0,
    );
    assert.equal(
      mock.seen.filter((entry) => entry.path === "/v1/models").length,
      0,
    );
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("Cursor fixture mode rejects forged live state and stale receipts", async (t) => {
  for (const [name, mutate] of [
    [
      "live-state",
      (receipt) => {
        receipt.verificationState = "live_verified";
        receipt.liveState = "live_verified";
      },
    ],
    ["stale", (receipt) => (receipt.recordedAt = "2026-09-01T00:00:00Z")],
  ]) {
    await t.test(name, async () => {
      const directory = fs.mkdtempSync(
        path.join(os.tmpdir(), `cursor-real-${name}-`),
      );
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, "api_key", mutate);
      const mock = await startMockServer("api_key");
      try {
        const result = await runScript(
          "api_key",
          fullEnv("api_key", mock.url, receiptFile),
        );
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /^\[FAIL\]/m);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Cursor control-plane failures never print tokens or raw bodies", async () => {
  const directory = fs.mkdtempSync(
    path.join(os.tmpdir(), "cursor-real-redaction-"),
  );
  const receiptFile = path.join(directory, "receipt.json");
  writeReceipt(receiptFile, "api_key");
  const leaked = "key-this-must-never-be-printed";
  const mock = await startMockServer("api_key", {
    failProvidersWith: `Bearer ${serverSecret}; ${routerSecret}; ${leaked}; raw body`,
  });
  try {
    const result = await runScript(
      "api_key",
      fullEnv("api_key", mock.url, receiptFile),
    );
    assert.equal(result.code, 1, result.stdout);
    const output = `${result.stdout}\n${result.stderr}`;
    assert.match(result.stderr, /^\[FAIL\]/m);
    assert.doesNotMatch(output, new RegExp(serverSecret, "g"));
    assert.doesNotMatch(output, new RegExp(routerSecret, "g"));
    assert.doesNotMatch(output, new RegExp(leaked, "g"));
    assert.doesNotMatch(output, /raw body/);
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
