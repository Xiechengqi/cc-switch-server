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
const script = path.join(repoRoot, "scripts/smoke/claude-real-receipt.mjs");
const contract = JSON.parse(
  fs.readFileSync(
    path.join(repoRoot, "assets/contract/claude-reference-delta.json"),
    "utf8",
  ),
);
const targetCommit = execFileSync("git", ["rev-parse", "HEAD"], {
  cwd: repoRoot,
  encoding: "utf8",
}).trim();
const serverSecret = "claude-server-secret-for-harness";
const routerSecret = "claude-router-secret-for-harness";

const specs = Object.freeze({
  oauth_inference: Object.freeze({
    accountEnv: "CLAUDE_OAUTH_TEST_ACCOUNT",
    providerEnv: "CC_SWITCH_CLAUDE_OAUTH_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CLAUDE_OAUTH_SHARE_ID",
    modelEnv: "CC_SWITCH_CLAUDE_OAUTH_MODEL",
    receiptEnv: "CLAUDE_OAUTH_INFERENCE_REAL_RECEIPT_FILE",
    model: "claude-sonnet-4-6",
    plan: null,
  }),
  max_5x_plan: Object.freeze({
    accountEnv: "CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT",
    providerEnv: "CC_SWITCH_CLAUDE_MAX_5X_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CLAUDE_MAX_5X_SHARE_ID",
    modelEnv: "CC_SWITCH_CLAUDE_MAX_5X_MODEL",
    receiptEnv: "CLAUDE_MAX_5X_REAL_RECEIPT_FILE",
    model: "claude-sonnet-4-6",
    plan: Object.freeze({ planType: "claude_max_5x", planLabel: "Claude Max 5x" }),
  }),
  max_20x_plan: Object.freeze({
    accountEnv: "CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT",
    providerEnv: "CC_SWITCH_CLAUDE_MAX_20X_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CLAUDE_MAX_20X_SHARE_ID",
    modelEnv: "CC_SWITCH_CLAUDE_MAX_20X_MODEL",
    receiptEnv: "CLAUDE_MAX_20X_REAL_RECEIPT_FILE",
    model: "claude-sonnet-4-6",
    plan: Object.freeze({ planType: "claude_max_20x", planLabel: "Claude Max 20x" }),
  }),
  fable_5_1: Object.freeze({
    accountEnv: "CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT",
    providerEnv: "CC_SWITCH_CLAUDE_FABLE_5_1_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CLAUDE_FABLE_5_1_SHARE_ID",
    modelEnv: "CC_SWITCH_CLAUDE_FABLE_5_1_MODEL",
    receiptEnv: "CLAUDE_FABLE_5_1_REAL_RECEIPT_FILE",
    model: "claude-fable-5-1",
    plan: Object.freeze({ planType: "claude_max_20x", planLabel: "Claude Max 20x" }),
  }),
  opus_5_5: Object.freeze({
    accountEnv: "CLAUDE_OAUTH_TEST_ACCOUNT",
    providerEnv: "CC_SWITCH_CLAUDE_OPUS_5_5_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CLAUDE_OPUS_5_5_SHARE_ID",
    modelEnv: "CC_SWITCH_CLAUDE_OPUS_5_5_MODEL",
    receiptEnv: "CLAUDE_OPUS_5_5_REAL_RECEIPT_FILE",
    model: "claude-opus-5-5",
    plan: null,
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
  const observedAt = Date.now();
  const plan = spec.plan
    ? {
        ...spec.plan,
        source: "fixture_live_usage",
        observedAt,
        stale: false,
        conflict: false,
      }
    : null;
  return {
    operation,
    spec,
    accountId: `claude-${operation}-account`,
    providerId: `claude-${operation}-provider`,
    shareId: `claude-${operation}-share`,
    authIdentityGeneration: index + 5,
    tokenRefreshGeneration: index + 2,
    providerRevision: 3,
    shareRevision: 5,
    runtimeFingerprint: `claude-${operation}-runtime`,
    plan,
  };
}

function expectedDecisions(operation) {
  const decisions = {
    crossAccountFallback: "disabled",
    crossProviderFallback: "disabled",
    crossShareFallback: "disabled",
  };
  if (operation === "oauth_inference") {
    Object.assign(decisions, {
      sameAccount401: "replayed_once",
      second401: "terminal",
      rateLimitScope: "bounded_exact_scope",
      terminalLateDisconnect: "complete",
    });
  } else if (operation === "fable_5_1") {
    Object.assign(decisions, {
      quotaRefresh: "forced",
      rateLimitScope: "fable_pool",
      modelFallback: "disabled",
    });
  } else if (operation === "opus_5_5") {
    Object.assign(decisions, {
      modelFallback: "disabled",
      context1mProbe: "bounded_small_request",
    });
  } else {
    decisions.quotaRefresh = "forced";
  }
  return decisions;
}

function scopeDigest(value) {
  return digest("cc-switch-server:claude-real-scope:v1", {
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
    model: value.spec.model,
    plan: value.plan,
    harnessRevision: 1,
  });
}

function receipt(value, overrides = {}) {
  const operationContract = [
    ...contract.realAcceptance.operations,
    ...(contract.realAcceptanceExtensions || []),
  ].find(
    (candidate) => candidate.operation === value.operation,
  );
  return {
    schemaVersion: 1,
    providerFamily: "claude",
    operation: value.operation,
    verificationState: "contract_verified",
    liveState: "live_pending",
    targetCommit,
    harnessRevision: 1,
    recordedAt: new Date().toISOString(),
    scopeDigest: scopeDigest(value),
    model: value.spec.model,
    checks: Object.fromEntries(
      operationContract.requiredChecks.map((check) => [check, "pass"]),
    ),
    bodyHashes: Object.fromEntries(
      operationContract.requiredBodyHashes.map((name) => [
        name,
        digest(`claude-fixture:${value.operation}`, name),
      ]),
    ),
    generations: {
      authIdentityGeneration: value.authIdentityGeneration,
      tokenRefreshGeneration: value.tokenRefreshGeneration,
      providerRevision: value.providerRevision,
      shareRevision: value.shareRevision,
      providerBindingDigest: digest("cc-switch-server:claude-provider-binding:v1", {
        providerId: value.providerId,
        runtimeFingerprint: value.runtimeFingerprint,
        accountId: value.accountId,
        authIdentityGeneration: value.authIdentityGeneration,
      }),
    },
    plan: value.plan,
    decisions: expectedDecisions(value.operation),
    decoyRequests: { otherAccount: 0, otherProvider: 0, otherShare: 0 },
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
    CLAUDE_OAUTH_TEST_ACCOUNT: "",
    CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT: "",
    CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT: "",
    CLAUDE_REAL_RECEIPT_FILE: "",
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
        CC_SWITCH_CLAUDE_HARNESS_MODE: "fixture",
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
  { providerGeneration = value.authIdentityGeneration, planConflict = false, failProvidersWith = "" } = {},
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
            providerType: "claude_oauth",
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
            app: "claude",
            providerType: "claude_oauth",
            providerTypeId: "claude_oauth",
            providerRevision: value.providerRevision,
            provider: { id: value.providerId },
            runtime: {
              driverId: "oauth.claude_messages",
              configurationState: "ready",
              runtimeFingerprint: value.runtimeFingerprint,
              authRef: {
                kind: "managed_account",
                accountId: value.accountId,
                expectedProviderType: "claude_oauth",
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
            app: "claude",
            providerId: value.providerId,
            providerType: "claude_oauth",
            configRevision: value.shareRevision,
            bindings: [],
          },
        ],
      });
      return;
    }
    if (request.method === "GET" && url.pathname === "/v1/models") {
      assert.equal(url.searchParams.get("app"), "claude");
      assert.equal(url.searchParams.get("providerId"), value.providerId);
      sendJson(response, 200, {
        object: "list",
        data: [{ id: value.spec.model, object: "model", owned_by: "anthropic" }],
      });
      return;
    }
    if (
      request.method === "GET" &&
      url.pathname === `/api/accounts/${encodeURIComponent(value.accountId)}/quota`
    ) {
      assert.equal(url.searchParams.get("refresh"), "true");
      assert.equal(url.searchParams.get("force"), "true");
      assert.ok(value.plan);
      sendJson(response, 200, {
        ok: true,
        refreshed: true,
        account: {
          id: value.accountId,
          providerType: "claude_oauth",
          authIdentityGeneration: value.authIdentityGeneration,
          subscriptionLevel: value.plan.planLabel,
        },
        quota: {
          success: true,
          credentialMessage: value.plan.planLabel,
          extraUsage: {
            subscription: {
              planType: value.plan.planType,
              planLabel: value.plan.planLabel,
              planSource: value.plan.source,
              planStale: false,
              planObservedAt: value.plan.observedAt,
            },
            subscriptionEvidence: {
              source: value.plan.source,
              stale: false,
              observedAt: value.plan.observedAt,
              conflict: planConflict,
            },
          },
        },
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
    [value.spec.accountEnv]: value.accountId,
    [value.spec.providerEnv]: value.providerId,
    [value.spec.shareEnv]: value.shareId,
    [value.spec.modelEnv]: value.spec.model,
    [value.spec.receiptEnv]: receiptFile,
  };
}

test("Claude real harness validates all five operation receipts independently", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `claude-${operation}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, value);
      const mock = await startMockServer(value);
      try {
        const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
        assert.equal(result.signal, null);
        assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
        assert.match(
          result.stdout,
          new RegExp(`Claude ${operation} private receipt accepted`),
        );
        assert.match(result.stdout, /verificationState=contract_verified, liveState=live_pending/);
        assert.equal(
          mock.seen.filter((entry) => entry.path.endsWith("/quota")).length,
          value.plan ? 1 : 0,
        );
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Claude real harness reports missing inputs without promoting live state", async () => {
  const result = await runScript("oauth_inference", { RUN_REAL: "0" });
  assert.equal(result.code, 0, result.stderr);
  const output = JSON.parse(result.stdout);
  assert.equal(output.operation, "oauth_inference");
  assert.equal(output.verificationState, "blocked_inputs");
  assert.equal(output.liveState, "live_pending");
  assert.ok(output.missingInputs.includes("RUN_REAL=1"));
  assert.ok(output.missingInputs.includes("CLAUDE_OAUTH_INFERENCE_REAL_RECEIPT_FILE"));
  assert.doesNotMatch(result.stdout, /live_verified/);
});

test("Claude real harness rejects cross-operation and fixture live-state promotion", async (t) => {
  for (const [name, override] of [
    ["cross-operation", { operation: "max_5x_plan" }],
    ["live-state", { verificationState: "live_verified", liveState: "live_verified" }],
  ]) {
    await t.test(name, async () => {
      const value = fixture("oauth_inference");
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `claude-${name}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, value, override);
      const mock = await startMockServer(value);
      try {
        const result = await runScript(
          "oauth_inference",
          fullEnv(value, mock.url, receiptFile),
        );
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /^\[FAIL\] Claude acceptance failed \(details redacted\)$/m);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Claude real harness fails closed on Provider generation drift before Share calls", async () => {
  const value = fixture("oauth_inference");
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "claude-generation-drift-"));
  const receiptFile = path.join(directory, "receipt.json");
  writeReceipt(receiptFile, value);
  const mock = await startMockServer(value, {
    providerGeneration: value.authIdentityGeneration + 1,
  });
  try {
    const result = await runScript(
      value.operation,
      fullEnv(value, mock.url, receiptFile),
    );
    assert.equal(result.code, 1, result.stdout);
    assert.equal(mock.seen.filter((entry) => entry.path === "/api/shares").length, 0);
    assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 0);
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("Claude real harness rejects conflicting fresh plan evidence", async () => {
  const value = fixture("max_20x_plan");
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "claude-plan-conflict-"));
  const receiptFile = path.join(directory, "receipt.json");
  writeReceipt(receiptFile, value);
  const mock = await startMockServer(value, { planConflict: true });
  try {
    const result = await runScript(
      value.operation,
      fullEnv(value, mock.url, receiptFile),
    );
    assert.equal(result.code, 1, result.stdout);
    assert.equal(mock.seen.filter((entry) => entry.path.endsWith("/quota")).length, 1);
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("Claude real harness never prints secrets or a raw failure body", async () => {
  const value = fixture("oauth_inference");
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "claude-redaction-"));
  const receiptFile = path.join(directory, "receipt.json");
  writeReceipt(receiptFile, value);
  const leaked = "sk-ant-this-must-never-be-printed";
  const mock = await startMockServer(value, {
    failProvidersWith: `Bearer ${serverSecret}; ${routerSecret}; ${leaked}; raw body`,
  });
  try {
    const result = await runScript(
      value.operation,
      fullEnv(value, mock.url, receiptFile),
    );
    assert.equal(result.code, 1, result.stdout);
    const output = `${result.stdout}\n${result.stderr}`;
    assert.doesNotMatch(output, new RegExp(serverSecret, "g"));
    assert.doesNotMatch(output, new RegExp(routerSecret, "g"));
    assert.doesNotMatch(output, /sk-ant-/);
    assert.doesNotMatch(output, /raw body/);
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
