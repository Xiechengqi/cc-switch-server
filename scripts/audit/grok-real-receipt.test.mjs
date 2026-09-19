import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import test from "node:test";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const script = path.join(repoRoot, "scripts/smoke/grok-real-receipt.mjs");
const contract = JSON.parse(
  fs.readFileSync(
    path.join(repoRoot, "assets/contract/grok-reference-delta.json"),
    "utf8",
  ),
);
const targetCommit = execFileSync("git", ["rev-parse", "HEAD"], {
  cwd: repoRoot,
  encoding: "utf8",
}).trim();
const serverSecret = "grok-server-secret-for-harness";
const routerSecret = "grok-router-secret-for-harness";

const specs = Object.freeze({
  inference: Object.freeze({
    providerEnv: "CC_SWITCH_GROK_INFERENCE_PROVIDER_ID",
    shareEnv: "CC_SWITCH_GROK_INFERENCE_SHARE_ID",
    modelEnv: "CC_SWITCH_GROK_INFERENCE_MODEL",
    receiptEnv: "GROK_INFERENCE_REAL_RECEIPT_FILE",
    model: "grok-4.6",
  }),
  media: Object.freeze({
    providerEnv: "CC_SWITCH_GROK_MEDIA_PROVIDER_ID",
    shareEnv: "CC_SWITCH_GROK_MEDIA_SHARE_ID",
    modelEnv: "CC_SWITCH_GROK_MEDIA_MODEL",
    receiptEnv: "GROK_MEDIA_REAL_RECEIPT_FILE",
    model: "grok-4.6",
  }),
  remote_compaction: Object.freeze({
    providerEnv: "CC_SWITCH_GROK_COMPACTION_PROVIDER_ID",
    shareEnv: "CC_SWITCH_GROK_COMPACTION_SHARE_ID",
    modelEnv: "CC_SWITCH_GROK_COMPACTION_MODEL",
    receiptEnv: "GROK_REMOTE_COMPACTION_REAL_RECEIPT_FILE",
    model: "grok-4.6",
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

function fixture(operation) {
  const spec = specs[operation];
  const index = Object.keys(specs).indexOf(operation) + 1;
  const modelData = [{ id: spec.model, object: "model", owned_by: "xai" }];
  const catalog = {
    source: "grok_upstream_fetch_available_models",
    fetchedAtMs: Date.now(),
    stale: false,
    digest: digest("cc-switch-server:grok-catalog:v1", modelData),
  };
  return {
    operation,
    spec,
    accountId: `grok-${operation}-account`,
    providerId: `grok-${operation}-provider`,
    shareId: `grok-${operation}-share`,
    signedUser: `signed-${operation}@example.test`,
    sessionId: `grok-${operation}-session`,
    turnIndex: String(index + 6),
    authIdentityGeneration: index + 5,
    tokenRefreshGeneration: index + 2,
    providerRevision: 3,
    shareRevision: 5,
    runtimeFingerprint: `grok-${operation}-runtime`,
    modelData,
    catalog,
  };
}

function expectedDecisions(operation) {
  const decisions = {
    crossAccountFallback: "disabled",
    crossProviderFallback: "disabled",
    crossShareFallback: "disabled",
    crossRailFallback: "disabled",
    postCommitReplay: "disabled",
    webCookie: "disabled",
    poolRouting: "disabled",
  };
  if (operation === "inference") {
    Object.assign(decisions, {
      sameAccount401: "replayed_once",
      second401: "terminal",
      reasoningRejection: "replayed_once_precommit",
      rateLimitScope: "bound_account",
      catalogStale: "same_identity_only",
      sessionTurnDrift: "terminal",
    });
  } else if (operation === "media") {
    Object.assign(decisions, {
      sameAccount401: "replayed_once",
      second401: "terminal",
      taskBinding: "sticky",
      rateLimitScope: "bound_account",
      invalidInput: "zero_upstream",
    });
  } else {
    Object.assign(decisions, {
      runtimeEnabled: "disabled",
      receiptAutoEnablesRuntime: "false",
      designReview: "required",
      accountCache: "isolated",
    });
  }
  return decisions;
}

function userNamespaceDigest(value) {
  return digest(
    "cc-switch-server:grok-signed-user:v1",
    value.signedUser.toLowerCase(),
  );
}

function sessionDigest(value) {
  return digest("cc-switch-server:grok-session:v1", value.sessionId);
}

function scopeDigest(value) {
  return digest("cc-switch-server:grok-real-scope:v1", {
    operation: value.operation,
    targetCommit,
    accountId: value.accountId,
    authIdentityGeneration: value.authIdentityGeneration,
    tokenRefreshGeneration: value.tokenRefreshGeneration,
    providerId: value.providerId,
    providerRevision: value.providerRevision,
    runtimeFingerprint: value.runtimeFingerprint,
    shareId: value.shareId,
    shareRevision: value.shareRevision,
    userNamespaceDigest: userNamespaceDigest(value),
    sessionDigest: sessionDigest(value),
    turnIndex: value.turnIndex,
    model: value.spec.model,
    catalog: value.catalog,
    harnessRevision: 1,
  });
}

function receipt(value, overrides = {}) {
  const operationContract = contract.realAcceptance.operations.find(
    (candidate) => candidate.operation === value.operation,
  );
  return {
    schemaVersion: 1,
    providerFamily: "grok",
    operation: value.operation,
    verificationState: "contract_verified",
    liveState: "live_pending",
    targetCommit,
    harnessRevision: 1,
    recordedAt: new Date().toISOString(),
    scopeDigest: scopeDigest(value),
    userNamespaceDigest: userNamespaceDigest(value),
    sessionDigest: sessionDigest(value),
    turnIndex: value.turnIndex,
    model: value.spec.model,
    catalog: value.catalog,
    checks: Object.fromEntries(
      operationContract.requiredChecks.map((check) => [check, "pass"]),
    ),
    bodyHashes: Object.fromEntries(
      operationContract.requiredBodyHashes.map((name) => [
        name,
        digest(`grok-fixture:${value.operation}`, name),
      ]),
    ),
    measurements: Object.fromEntries(
      operationContract.requiredMeasurements.map((name) => [name, 1]),
    ),
    generations: {
      authIdentityGeneration: value.authIdentityGeneration,
      tokenRefreshGeneration: value.tokenRefreshGeneration,
      providerRevision: value.providerRevision,
      shareRevision: value.shareRevision,
      credentialIdentityDigest: digest("cc-switch-server:grok-credential:v1", {
        accountId: value.accountId,
        authIdentityGeneration: value.authIdentityGeneration,
        tokenRefreshGeneration: value.tokenRefreshGeneration,
      }),
      providerBindingDigest: digest("cc-switch-server:grok-provider-binding:v1", {
        providerId: value.providerId,
        runtimeFingerprint: value.runtimeFingerprint,
        accountId: value.accountId,
        authIdentityGeneration: value.authIdentityGeneration,
      }),
      shareBindingDigest: digest("cc-switch-server:grok-share-binding:v1", {
        shareId: value.shareId,
        shareRevision: value.shareRevision,
        providerId: value.providerId,
        userNamespaceDigest: userNamespaceDigest(value),
      }),
    },
    decisions: expectedDecisions(value.operation),
    decoyRequests: {
      otherAccount: 0,
      otherProvider: 0,
      otherShare: 0,
      otherRail: 0,
      otherSession: 0,
      webCookie: 0,
      poolRouter: 0,
    },
    sensitiveScan: { status: "pass", matches: 0 },
    ...overrides,
  };
}

function writeReceipt(file, value, overrides = {}) {
  fs.writeFileSync(file, `${JSON.stringify(receipt(value, overrides), null, 2)}\n`, {
    mode: 0o600,
  });
}

function runScript(operation, overrides = {}) {
  const cleared = {
    GROK_OAUTH_TEST_ACCOUNT: "",
    GROK_REAL_RECEIPT_FILE: "",
    CC_SWITCH_GROK_SIGNED_USER: "",
    CC_SWITCH_GROK_SESSION_ID: "",
    CC_SWITCH_GROK_TURN_INDEX: "",
  };
  for (const spec of Object.values(specs)) {
    cleared[spec.providerEnv] = "";
    cleared[spec.shareEnv] = "";
    cleared[spec.modelEnv] = "";
    cleared[spec.receiptEnv] = "";
  }
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [script, "--operation", operation], {
      cwd: repoRoot,
      env: {
        ...process.env,
        ...cleared,
        RUN_REAL: "1",
        CC_SWITCH_GROK_HARNESS_MODE: "fixture",
        SERVER_URL: "",
        CC_SWITCH_SERVER_TOKEN: "",
        CC_SWITCH_SHARE_URL: "",
        ROUTER_API_TOKEN: "",
        ROUTER_API_TOKEN_HEADER: "Authorization",
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

async function startMockServer(
  value,
  {
    providerGeneration = value.authIdentityGeneration,
    catalogStale = false,
    failProvidersWith = "",
  } = {},
) {
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
            email: `${value.operation}@example.test`,
            providerType: "grok_oauth",
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
        providers: [
          {
            app: "codex",
            providerType: "grok_oauth",
            providerTypeId: "grok_oauth",
            providerRevision: value.providerRevision,
            provider: { id: value.providerId },
            runtime: {
              driverId: "oauth.grok_responses",
              configurationState: "ready",
              runtimeFingerprint: value.runtimeFingerprint,
              authRef: {
                kind: "managed_account",
                accountId: value.accountId,
                expectedProviderType: "grok_oauth",
                authIdentityGeneration: providerGeneration,
              },
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
            app: "codex",
            providerId: value.providerId,
            providerType: "grok_oauth",
            configRevision: value.shareRevision,
            bindings: [],
          },
        ],
      });
      return;
    }
    if (request.method === "GET" && url.pathname === "/v1/models") {
      assert.equal(url.searchParams.get("app"), "codex");
      assert.equal(url.searchParams.get("providerId"), value.providerId);
      sendJson(response, 200, {
        object: "list",
        data: value.modelData,
        source: value.catalog.source,
        fetchedAtMs: value.catalog.fetchedAtMs,
        stale: catalogStale,
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

function fullEnv(value, url, receiptFile) {
  return {
    SERVER_URL: url,
    CC_SWITCH_SERVER_TOKEN: serverSecret,
    CC_SWITCH_SHARE_URL: url,
    ROUTER_API_TOKEN: routerSecret,
    GROK_OAUTH_TEST_ACCOUNT: value.accountId,
    CC_SWITCH_GROK_SIGNED_USER: value.signedUser,
    CC_SWITCH_GROK_SESSION_ID: value.sessionId,
    CC_SWITCH_GROK_TURN_INDEX: value.turnIndex,
    [value.spec.providerEnv]: value.providerId,
    [value.spec.shareEnv]: value.shareId,
    [value.spec.modelEnv]: value.spec.model,
    [value.spec.receiptEnv]: receiptFile,
  };
}

test("Grok receipt harness validates all three operations independently", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-${operation}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, value);
      const mock = await startMockServer(value);
      try {
        const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
        assert.equal(result.signal, null);
        assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
        assert.match(
          result.stdout,
          new RegExp(`Grok ${operation} private receipt accepted`),
        );
        assert.match(result.stdout, /verificationState=contract_verified, liveState=live_pending/);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Grok receipt harness reports missing operation inputs without promotion", async () => {
  const result = await runScript("inference", { RUN_REAL: "0" });
  assert.equal(result.code, 0, result.stderr);
  const output = JSON.parse(result.stdout);
  assert.equal(output.operation, "inference");
  assert.equal(output.verificationState, "blocked_inputs");
  assert.equal(output.liveState, "live_pending");
  assert.ok(output.missingInputs.includes("RUN_REAL=1"));
  assert.ok(output.missingInputs.includes("GROK_INFERENCE_REAL_RECEIPT_FILE"));
  assert.doesNotMatch(result.stdout, /live_verified/);
});

test("Grok receipt harness rejects cross-operation and fixture promotion", async (t) => {
  for (const [name, override] of [
    ["cross-operation", { operation: "media" }],
    ["live-state", { verificationState: "live_verified", liveState: "live_verified" }],
  ]) {
    await t.test(name, async () => {
      const value = fixture("inference");
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-${name}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, value, override);
      const mock = await startMockServer(value);
      try {
        const result = await runScript(
          "inference",
          fullEnv(value, mock.url, receiptFile),
        );
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /^\[FAIL\] Grok acceptance failed \(details redacted\)$/m);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Grok receipt harness fails closed on Account generation drift", async () => {
  const value = fixture("inference");
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "grok-generation-drift-"));
  const receiptFile = path.join(directory, "receipt.json");
  writeReceipt(receiptFile, value);
  const mock = await startMockServer(value, {
    providerGeneration: value.authIdentityGeneration + 1,
  });
  try {
    const result = await runScript(value.operation, fullEnv(value, mock.url, receiptFile));
    assert.equal(result.code, 1, result.stdout);
    assert.equal(mock.seen.filter((entry) => entry.path === "/api/shares").length, 0);
    assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 0);
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("Grok receipt harness rejects a stale current catalog", async () => {
  const value = fixture("media");
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "grok-stale-catalog-"));
  const receiptFile = path.join(directory, "receipt.json");
  writeReceipt(receiptFile, value);
  const mock = await startMockServer(value, { catalogStale: true });
  try {
    const result = await runScript(value.operation, fullEnv(value, mock.url, receiptFile));
    assert.equal(result.code, 1, result.stdout);
    assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 1);
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("Grok receipt harness never prints secrets or raw failure bodies", async () => {
  const value = fixture("inference");
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "grok-redaction-"));
  const receiptFile = path.join(directory, "receipt.json");
  writeReceipt(receiptFile, value);
  const leaked = "xai-this-must-never-be-printed";
  const mock = await startMockServer(value, {
    failProvidersWith: `Bearer ${serverSecret}; ${routerSecret}; ${leaked}; raw body`,
  });
  try {
    const result = await runScript(value.operation, fullEnv(value, mock.url, receiptFile));
    assert.equal(result.code, 1, result.stdout);
    const output = `${result.stdout}\n${result.stderr}`;
    assert.doesNotMatch(output, new RegExp(serverSecret, "g"));
    assert.doesNotMatch(output, new RegExp(routerSecret, "g"));
    assert.doesNotMatch(output, /xai-this/);
    assert.doesNotMatch(output, /raw body/);
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
