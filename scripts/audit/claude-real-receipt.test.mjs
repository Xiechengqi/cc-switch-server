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
const catalogCapturedAtMs = 1_790_121_600_000;

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
  { providerGeneration = value.authIdentityGeneration, planConflict = false, failProvidersWith = "", duplicateShareBinding = false, primaryMismatch = false, shareState = "active", scopeFault = "", drift = "", oversizedControl = "", onFirstControl = null, credentialCapability = "refreshable_oauth", hasRefreshToken = true, accountFault = null, finalShareDelayMs = 0, catalogData = null, finalCatalogFault = "" } = {},
) {
  const seen = [];
  let firstControlSeen = false;
  let accountReads = 0;
  let providerReads = 0;
  let shareReads = 0;
  let catalogReads = 0;
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    seen.push({
      path: url.pathname,
      search: url.search,
      authorization: request.headers.authorization || "",
    });
    if (!firstControlSeen && url.pathname === "/api/accounts") {
      firstControlSeen = true;
      onFirstControl?.();
    }
    if (request.method === "GET" && url.pathname === "/api/accounts") {
      accountReads += 1;
      if (oversizedControl === "declared") {
        response.writeHead(200, {
          "content-type": "application/json",
          "content-length": String(4 * 1024 * 1024 + 1),
        });
        response.write("{");
        return;
      }
      const accountFlags = { hasApiKey: false, needsRelogin: false };
      if (accountFault) {
        if (accountFault.variant === "missing") delete accountFlags[accountFault.field];
        else accountFlags[accountFault.field] = accountFault.variant === "null" ? null : "false";
      }
      sendJson(response, 200, {
        ok: true,
        accounts: [
          {
            id: value.accountId,
            email: `${value.operation}@example.test`,
            providerType: "claude_oauth",
            authIdentityGeneration: value.authIdentityGeneration,
            tokenRefreshGeneration: value.tokenRefreshGeneration + (drift === "account" && accountReads > 1 ? 1 : 0),
            hasAccessToken: true,
            hasRefreshToken: hasRefreshToken && !(drift === "refresh_credential" && accountReads > 1),
            ...(credentialCapability !== null && {
              credentialCapability: drift === "credential_capability" && accountReads > 1
                ? "access_only_setup_token"
                : credentialCapability,
            }),
            ...accountFlags,
          },
        ],
      });
      return;
    }
    if (request.method === "GET" && url.pathname === "/api/providers") {
      providerReads += 1;
      if (oversizedControl === "chunked") {
        response.writeHead(200, { "content-type": "application/json" });
        response.write("x".repeat(4 * 1024 * 1024 + 1));
        return;
      }
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
            revision: ["missing_provider_revision", "legacy_top_level_only", "missing_view_revision"].includes(scopeFault)
              ? undefined : value.providerRevision + (scopeFault === "mismatched_revisions" ? 1 : 0)
                + (drift === "provider" && providerReads > 1 ? 1 : 0),
            ...(["legacy_top_level_only", "missing_view_revision", "missing_runtime_revision", "mismatched_revisions"].includes(scopeFault)
              ? { providerRevision: value.providerRevision + (drift === "provider" && providerReads > 1 ? 1 : 0) } : {}),
            provider: { id: value.providerId },
            runtime: {
              providerRevision: ["missing_provider_revision", "legacy_top_level_only", "missing_runtime_revision"].includes(scopeFault)
                ? undefined : value.providerRevision + (drift === "provider" && providerReads > 1 ? 1 : 0),
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
      shareReads += 1;
      const payload = {
        ok: true,
        shares: [
          {
            id: value.shareId,
            app: "claude",
            providerId: primaryMismatch ? "conflicting-primary-provider" : value.providerId,
            providerType: "claude_oauth",
            enabled: shareState !== "disabled",
            status: shareState === "paused" ? "paused" : "active",
            configRevision: scopeFault === "missing_share_revision" ? undefined
              : value.shareRevision + (drift === "share" && shareReads > 1 ? 1 : 0),
            bindings: [
              { app: "claude", providerId: value.providerId, providerType: "claude_oauth" },
              ...(duplicateShareBinding ? [{
                app: "claude",
                providerId: "conflicting-claude-provider",
                providerType: "claude_oauth",
              }] : []),
            ],
          },
        ],
      };
      if (shareReads > 1 && finalShareDelayMs > 0) {
        setTimeout(() => sendJson(response, 200, payload), finalShareDelayMs);
      } else {
        sendJson(response, 200, payload);
      }
      return;
    }
    if (request.method === "GET" && url.pathname === "/v1/models") {
      assert.equal(url.searchParams.get("app"), "claude");
      assert.equal(url.searchParams.get("providerId"), value.providerId);
      catalogReads += 1;
      const fault = catalogReads > 1 ? finalCatalogFault : "";
      const modelData = catalogData ?? [
        { id: value.spec.model, object: "model", owned_by: "anthropic" },
      ];
      let responseData = fault === "remove" ? [] : modelData;
      if (fault === "add") {
        responseData = [...modelData, { id: "unexpected-model", object: "model" }];
      }
      if (fault === "reorder") responseData = [...modelData].reverse();
      if (fault === "capability") {
        responseData = modelData.map((entry) => entry.id === value.spec.model
          ? { ...entry, capabilities: { tools: false } }
          : entry);
      }
      sendJson(response, 200, {
        object: "list",
        data: responseData,
        source: fault === "source" ? "untrusted_catalog" : "claude_code_wire_profile",
        stale: fault === "stale",
        fetchedAtMs: catalogCapturedAtMs + (fault === "capture_time" ? 1 : 0),
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
        const catalogRequests = mock.seen.filter((entry) => entry.path === "/v1/models");
        assert.equal(catalogRequests.length, 2);
        const catalogQuery = new URLSearchParams(catalogRequests[0].search);
        assert.equal(catalogQuery.get("app"), "claude");
        assert.equal(catalogQuery.get("providerId"), value.providerId);
        assert.equal(catalogQuery.get("expectedShareId"), value.shareId);
        assert.equal(catalogQuery.get("expectedProviderId"), value.providerId);
        assert.equal(
          mock.seen.filter((entry) => entry.path.endsWith("/quota")).length,
          value.plan ? 1 : 0,
        );
        for (const path of ["/api/accounts", "/api/providers", "/api/shares"]) {
          assert.equal(mock.seen.filter((entry) => entry.path === path).length, 2);
        }
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Claude operations reject final static-catalog drift before receipt acceptance", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const kind of ["remove", "add", "capability", "stale", "source", "capture_time"]) {
      await t.test(`${operation}/${kind}`, async () => {
        const value = fixture(operation);
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `claude-final-catalog-${operation}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, value);
        const mock = await startMockServer(value, { finalCatalogFault: kind });
        try {
          const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
          assert.equal(result.code, 1, `${result.stdout}\n${result.stderr}`);
          assert.equal(result.stderr.trim(), "[FAIL] Claude acceptance failed (details redacted)");
          assert.ok(fs.existsSync(receiptFile));
          assert.equal(mock.seen.filter((entry) => entry.path === "/api/shares").length, 2);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Claude operations reject invalid IDs in the full static catalog", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const kind of ["missing", "blank", "duplicate", "nonstring"]) {
      await t.test(`${operation}/${kind}`, async () => {
        const value = fixture(operation);
        const catalogData = [
          { id: value.spec.model, object: "model", owned_by: "anthropic" },
          { id: "zz-claude-other", object: "model", owned_by: "anthropic" },
          kind === "missing" ? { object: "model", owned_by: "anthropic" }
            : { id: kind === "blank" ? " " : kind === "duplicate" ? "zz-claude-other" : 42, object: "model", owned_by: "anthropic" },
        ];
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `claude-catalog-id-${operation}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, value);
        const mock = await startMockServer(value, { catalogData });
        try {
          const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
          assert.equal(result.code, 1, `${result.stdout}\n${result.stderr}`);
          assert.equal(result.stderr.trim(), "[FAIL] Claude acceptance failed (details redacted)");
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Claude operations accept static catalog row reordering without content drift", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      const catalogData = [
        { id: value.spec.model, object: "model", owned_by: "anthropic" },
        { id: "zz-claude-other", object: "model", owned_by: "anthropic" },
      ];
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `claude-catalog-reorder-${operation}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, value);
      const mock = await startMockServer(value, { catalogData, finalCatalogFault: "reorder" });
      try {
        const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
        assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
        assert.match(result.stdout, /contract_verified.*live_pending/);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Claude operations reject ambiguous or inactive Shares before catalog requests", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const [name, options] of [
      ["duplicate", { duplicateShareBinding: true }],
      ["primary_mismatch", { primaryMismatch: true }],
      ["disabled", { shareState: "disabled" }],
      ["paused", { shareState: "paused" }],
    ]) {
      await t.test(`${operation}/${name}`, async () => {
        const value = fixture(operation);
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `claude-share-${operation}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, value);
        const mock = await startMockServer(value, options);
        try {
          const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Claude acceptance failed (details redacted)");
          assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 0);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Claude operations reject receipts or plan evidence expiring during final binding validation", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const evidence of ["receipt", ...(specs[operation].plan ? ["plan"] : [])]) {
      await t.test(`${operation}/${evidence}`, async () => {
        const value = fixture(operation);
        if (evidence === "plan") {
          value.plan.observedAt = Date.now() - 15 * 60 * 1_000 + 1_200;
        }
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `claude-${operation}-${evidence}-expiry-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, value, evidence === "receipt"
          ? { recordedAt: new Date(Date.now() - 24 * 60 * 60 * 1_000 + 1_200).toISOString() }
          : {});
        const mock = await startMockServer(value, { finalShareDelayMs: 2_000 });
        try {
          const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.match(result.stderr, /details redacted/);
          assert.equal(mock.seen.filter((entry) => entry.path === "/api/shares").length, 2);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Claude private receipt cannot enter the repository through an external symlink", async () => {
  const value = fixture("oauth_inference");
  const insideDirectory = fs.mkdtempSync(path.join(repoRoot, ".claude-receipt-test-"));
  const outsideDirectory = fs.mkdtempSync(path.join(os.tmpdir(), "claude-receipt-link-"));
  const insideReceipt = path.join(insideDirectory, "receipt.json");
  const linkedReceipt = path.join(outsideDirectory, "receipt.json");
  const mock = await startMockServer(value);
  try {
    writeReceipt(insideReceipt, value);
    fs.symlinkSync(insideReceipt, linkedReceipt);
    const result = await runScript(value.operation, fullEnv(value, mock.url, linkedReceipt));
    assert.equal(result.code, 1, result.stdout);
    assert.equal(mock.seen.length, 0);
  } finally {
    await mock.close();
    fs.rmSync(outsideDirectory, { recursive: true, force: true });
    fs.rmSync(insideDirectory, { recursive: true, force: true });
  }
});

test("Claude private receipts cannot be hard-linked from inside the repository", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      const insideDirectory = fs.mkdtempSync(path.join(repoRoot, ".claude-receipt-hardlink-"));
      const outsideDirectory = fs.mkdtempSync(path.join(path.dirname(repoRoot), ".claude-receipt-outside-"));
      const insideReceipt = path.join(insideDirectory, "receipt.json");
      const outsideReceipt = path.join(outsideDirectory, "receipt.json");
      writeReceipt(insideReceipt, value);
      fs.linkSync(insideReceipt, outsideReceipt);
      const mock = await startMockServer(value);
      try {
        const result = await runScript(operation, fullEnv(value, mock.url, outsideReceipt));
        assert.equal(result.code, 1, result.stdout);
        assert.equal(result.stderr.trim(), "[FAIL] Claude acceptance failed (details redacted)");
        assert.deepEqual(mock.seen, []);
      } finally {
        await mock.close();
        fs.rmSync(outsideDirectory, { recursive: true, force: true });
        fs.rmSync(insideDirectory, { recursive: true, force: true });
      }
    });
  }
});

test("Claude private receipt cannot be replaced during control-plane validation", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `claude-receipt-swap-${operation}-`));
      const receiptFile = path.join(directory, "receipt.json");
      const replacementFile = path.join(directory, "replacement.json");
      writeReceipt(receiptFile, value, { model: "not-the-selected-model" });
      writeReceipt(replacementFile, value);
      const originalInode = fs.statSync(receiptFile).ino;
      const mock = await startMockServer(value, {
        onFirstControl: () => fs.renameSync(replacementFile, receiptFile),
      });
      try {
        const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
        assert.equal(result.code, 1, result.stdout);
        assert.equal(result.stderr.trim(), "[FAIL] Claude acceptance failed (details redacted)");
        assert.ok(mock.seen.some((entry) => entry.path === "/api/accounts"));
        assert.notEqual(fs.statSync(receiptFile).ino, originalInode);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Claude private receipt must remain the same file through final acceptance", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const change of ["replacement", "in_place"]) {
      await t.test(`${operation}/${change}`, async () => {
        const value = fixture(operation);
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `claude-receipt-final-${operation}-`));
        const receiptFile = path.join(directory, "receipt.json");
        const replacementFile = path.join(directory, "replacement.json");
        writeReceipt(receiptFile, value);
        if (change === "replacement") {
          fs.writeFileSync(replacementFile, "not the accepted receipt\n", { mode: 0o600 });
        }
        const originalInode = fs.statSync(receiptFile).ino;
        const mock = await startMockServer(value, {
          onFirstControl: () => {
            if (change === "replacement") {
              fs.renameSync(replacementFile, receiptFile);
            } else {
              fs.writeFileSync(receiptFile, " ".repeat(fs.statSync(receiptFile).size), { flag: "r+" });
              fs.utimesSync(receiptFile, new Date(0), new Date(0));
            }
          },
        });
        try {
          const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Claude acceptance failed (details redacted)");
          assert.ok(mock.seen.some((entry) => entry.path === "/api/accounts"));
          if (change === "replacement") {
            assert.notEqual(fs.statSync(receiptFile).ino, originalInode);
          } else {
            assert.equal(fs.statSync(receiptFile).ino, originalInode);
          }
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Claude final acceptance rejects changed bytes despite unchanged reported metadata", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `claude-receipt-bytes-${operation}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, value);
      const preload = [
        'import fs from "node:fs";',
        `const target = process.env[${JSON.stringify(specs[operation].receiptEnv)}];`,
        'const originalStat = fs.statSync;',
        'const originalFstat = fs.fstatSync;',
        'const originalFingerprint = originalStat(target, { bigint: true });',
        'let bigintFstatCount = 0;',
        'fs.statSync = (path, options) => {',
        '  if (path === target && options?.bigint) return originalFingerprint;',
        '  return originalStat(path, options);',
        '};',
        'fs.fstatSync = (fd, options) => {',
        '  if (options?.bigint && ++bigintFstatCount > 2) return originalFingerprint;',
        '  return originalFstat(fd, options);',
        '};',
      ].join("\n");
      const mock = await startMockServer(value, {
        onFirstControl: () => {
          fs.writeFileSync(receiptFile, " ".repeat(fs.statSync(receiptFile).size), { flag: "r+" });
        },
      });
      try {
        const result = await runScript(operation, {
          ...fullEnv(value, mock.url, receiptFile),
          NODE_OPTIONS: `--import=data:text/javascript,${encodeURIComponent(preload)}`,
        });
        assert.equal(result.code, 1, result.stdout);
        assert.equal(result.stderr.trim(), "[FAIL] Claude acceptance failed (details redacted)");
        assert.ok(mock.seen.some((entry) => entry.path === "/api/accounts"));
        assert.match(fs.readFileSync(receiptFile, "utf8"), /^ +$/);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Claude private receipt rejects an in-place rewrite during its snapshot", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `claude-receipt-inplace-${operation}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, value);
      const preload = [
        'import fs from "node:fs";',
        'const originalRead = fs.readSync;',
        `const target = process.env[${JSON.stringify(value.spec.receiptEnv)}];`,
        'let changed = false;',
        'fs.readSync = (fd, ...args) => {',
        '  const count = originalRead(fd, ...args);',
        '  if (!changed && count > 0 && fs.fstatSync(fd).ino === fs.statSync(target).ino) {',
        '    changed = true;',
        '    fs.writeFileSync(target, " ".repeat(fs.statSync(target).size), { flag: "r+" });',
        '    fs.utimesSync(target, new Date(0), new Date(0));',
        '  }',
        '  return count;',
        '};',
      ].join("\n");
      const mock = await startMockServer(value);
      try {
        const result = await runScript(operation, {
          ...fullEnv(value, mock.url, receiptFile),
          NODE_OPTIONS: `--import=data:text/javascript,${encodeURIComponent(preload)}`,
        });
        assert.equal(result.code, 1, result.stdout);
        assert.equal(result.stderr.trim(), "[FAIL] Claude acceptance failed (details redacted)");
        assert.deepEqual(mock.seen, []);
        assert.match(fs.readFileSync(receiptFile, "utf8"), /^ +$/);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Claude private receipt size is bounded before control-plane requests", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `claude-receipt-size-${operation}-`));
      const receiptFile = path.join(directory, "receipt.json");
      fs.writeFileSync(receiptFile, "x".repeat(1024 * 1024 + 1), { mode: 0o600 });
      const mock = await startMockServer(value);
      try {
        const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
        assert.equal(result.code, 1, result.stdout);
        assert.equal(result.stderr.trim(), "[FAIL] Claude acceptance failed (details redacted)");
        assert.deepEqual(mock.seen, []);
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

test("Claude receipt gate rejects binding drift before acceptance for every operation", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const drift of ["account", "provider", "share"]) {
      await t.test(`${operation}/${drift}`, async () => {
        const value = fixture(operation);
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `claude-${operation}-${drift}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, value);
        const mock = await startMockServer(value, { drift });
        try {
          const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.match(result.stderr, /details redacted/);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Claude operations reject incomplete revision or runtime fingerprint scope before catalogs", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const scopeFault of [
      "missing_provider_revision",
      "missing_share_revision",
      "whitespace_runtime_fingerprint",
    ]) {
      await t.test(`${operation}/${scopeFault}`, async () => {
        const value = fixture(operation);
        if (scopeFault === "missing_provider_revision") value.providerRevision = 0;
        if (scopeFault === "missing_share_revision") value.shareRevision = 0;
        if (scopeFault === "whitespace_runtime_fingerprint") value.runtimeFingerprint = "   ";
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `claude-scope-${operation}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, value);
        const mock = await startMockServer(value, { scopeFault });
        try {
          const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Claude acceptance failed (details redacted)");
          assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 0);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Claude operations reject legacy top-level revision masking real Provider scope", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const scopeFault of [
      "legacy_top_level_only",
      "missing_view_revision",
      "missing_runtime_revision",
      "mismatched_revisions",
    ]) {
      await t.test(`${operation}/${scopeFault}`, async () => {
        const value = fixture(operation);
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `claude-real-scope-${operation}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, value);
        const mock = await startMockServer(value, { scopeFault });
        try {
          const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Claude acceptance failed (details redacted)");
          assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 0);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Claude OAuth operations require explicit false Account booleans", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const field of ["hasApiKey", "needsRelogin"]) {
      for (const variant of ["missing", "null", "string"]) {
        await t.test(`${operation}/${field}/${variant}`, async () => {
          const value = fixture(operation);
          const directory = fs.mkdtempSync(path.join(os.tmpdir(), `claude-${operation}-account-`));
          const receiptFile = path.join(directory, "receipt.json");
          writeReceipt(receiptFile, value);
          const mock = await startMockServer(value, { accountFault: { field, variant } });
          try {
            const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
            assert.equal(result.code, 1, result.stdout);
            assert.equal(result.stderr.trim(), "[FAIL] Claude acceptance failed (details redacted)");
            assert.equal(mock.seen.filter((entry) => entry.path === "/api/providers").length, 0);
            assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 0);
          } finally {
            await mock.close();
            fs.rmSync(directory, { recursive: true, force: true });
          }
        });
      }
    }
  }
});

test("Claude refresh-dependent receipts reject access-only, missing, or drifting credentials", async (t) => {
  for (const operation of ["oauth_inference", "max_5x_plan", "max_20x_plan", "fable_5_1"]) {
    for (const [label, mockOptions] of [
      ["access_only", { credentialCapability: "access_only_setup_token", hasRefreshToken: false }],
      ["missing_capability", { credentialCapability: null }],
      ["missing_refresh", { hasRefreshToken: false }],
      ["capability_drift", { drift: "credential_capability" }],
      ["refresh_drift", { drift: "refresh_credential" }],
    ]) {
      await t.test(`${operation}/${label}`, async () => {
        const value = fixture(operation);
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `claude-${operation}-${label}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, value);
        const mock = await startMockServer(value, mockOptions);
        try {
          const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.match(result.stderr, /details redacted/);
          if (!["capability_drift", "refresh_drift"].includes(label)) {
            assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 0);
          }
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Claude Opus receipt does not require a refresh token from an access-only setup credential", async () => {
  const value = fixture("opus_5_5");
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "claude-opus-access-only-"));
  const receiptFile = path.join(directory, "receipt.json");
  writeReceipt(receiptFile, value);
  const mock = await startMockServer(value, {
    credentialCapability: "access_only_setup_token",
    hasRefreshToken: false,
  });
  try {
    const result = await runScript("opus_5_5", fullEnv(value, mock.url, receiptFile));
    assert.equal(result.code, 0, result.stderr);
    assert.match(result.stdout, /contract_verified.*live_pending/);
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("Claude control-plane response bound rejects unfinished oversized bodies", async (t) => {
  for (const [operation, oversizedControl, expectedPaths] of [
    ["oauth_inference", "declared", ["/api/accounts"]],
    ["opus_5_5", "chunked", ["/api/accounts", "/api/providers"]],
  ]) {
    await t.test(`${operation}/${oversizedControl}`, async () => {
      const value = fixture(operation);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `claude-oversized-${operation}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, value);
      const mock = await startMockServer(value, { oversizedControl });
      try {
        const startedAt = Date.now();
        const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /details redacted/);
        assert.deepEqual(mock.seen.map((entry) => entry.path), expectedPaths);
        assert.ok(Date.now() - startedAt < 3_000, "oversized response waited for timeout");
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});
