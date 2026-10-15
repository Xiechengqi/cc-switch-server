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
    source: "upstream",
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
    catalogDrift = "",
    failProvidersWith = "",
    drift = "",
    shareFault = "",
    scopeFault = "",
    accountFault = "",
    finalShareDelayMs = 0,
    oversizedControl = "",
    onFirstControl = null,
  } = {},
) {
  const seen = [];
  let firstControlSeen = false;
  let accountReads = 0;
  let providerReads = 0;
  let shareReads = 0;
  let catalogReads = 0;
  const accountBoolean = (field) => {
    if (accountFault === `missing_${field}`) return undefined;
    if (accountFault === `null_${field}`) return null;
    if (accountFault === `string_${field}`) return "false";
    return false;
  };
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
      sendJson(response, 200, {
        ok: true,
        accounts: [
          {
            id: value.accountId,
            email: `${value.operation}@example.test`,
            providerType: "grok_oauth",
            authIdentityGeneration: value.authIdentityGeneration,
            tokenRefreshGeneration: value.tokenRefreshGeneration + (drift === "account" && accountReads > 1 ? 1 : 0),
            hasAccessToken: true,
            hasRefreshToken: true,
            hasApiKey: accountBoolean("has_api_key"),
            needsRelogin: accountBoolean("needs_relogin"),
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
            app: "codex",
            providerType: "grok_oauth",
            providerTypeId: "grok_oauth",
            revision: ["missing_view_revision", "missing_both_revisions", "legacy_top_level_only"].includes(scopeFault)
              ? undefined : value.providerRevision + (scopeFault === "mismatched_revisions" ? 1 : 0)
                + (drift === "provider" && providerReads > 1 ? 1 : 0),
            ...(scopeFault === "legacy_top_level_only" ? { providerRevision: value.providerRevision } : {}),
            provider: { id: value.providerId },
            runtime: {
              providerRevision: ["missing_runtime_revision", "missing_both_revisions", "legacy_top_level_only"].includes(scopeFault)
                ? undefined : value.providerRevision + (drift === "provider" && providerReads > 1 ? 1 : 0),
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
      shareReads += 1;
      const payload = {
        ok: true,
        shares: [
          {
            id: value.shareId,
            app: "codex",
            providerId: shareFault === "primary_conflict" ? "grok-other-provider" : value.providerId,
            providerType: "grok_oauth",
            enabled: shareFault !== "disabled",
            status: shareFault === "paused" ? "paused" : "active",
            configRevision: scopeFault === "missing_share_revision" ? undefined
              : value.shareRevision + (drift === "share" && shareReads > 1 ? 1 : 0),
            bindings: [
              { app: "codex", providerId: value.providerId, providerType: "grok_oauth" },
              ...(shareFault === "duplicate_conflict"
                ? [{ app: "codex", providerId: "grok-other-provider", providerType: "grok_oauth" }]
                : []),
            ],
          },
        ],
      };
      if (finalShareDelayMs && shareReads > 1) {
        setTimeout(() => sendJson(response, 200, payload), finalShareDelayMs);
      } else {
        sendJson(response, 200, payload);
      }
      return;
    }
    if (request.method === "GET" && url.pathname === "/v1/models") {
      catalogReads += 1;
      assert.equal(url.searchParams.get("app"), "codex");
      assert.equal(url.searchParams.get("providerId"), value.providerId);
      const finalCatalog = catalogReads > 1;
      sendJson(response, 200, {
        object: "list",
        data: finalCatalog && catalogDrift === "model_removed" ? []
          : finalCatalog && catalogDrift === "model_added"
            ? [...value.modelData, { id: "grok-extra", object: "model", owned_by: "xai" }]
            : finalCatalog && catalogDrift === "row_reordered"
              ? [...value.modelData].reverse()
              : finalCatalog && catalogDrift === "capability_changed"
                ? value.modelData.map((entry, index) => index === 0
                  ? { ...entry, capabilityManifestVersion: 2 } : entry)
            : value.modelData,
        source: finalCatalog && catalogDrift === "wrong_source" ? "static_registry"
          : finalCatalog && catalogDrift === "allowed_source" ? "cache_fresh"
            : value.catalog.source,
        fetchedAtMs: finalCatalog && catalogDrift === "future"
          ? Date.now() + 10 * 60_000 : value.catalog.fetchedAtMs,
        stale: catalogStale || (finalCatalog && catalogDrift === "stale"),
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
        for (const path of ["/api/accounts", "/api/providers", "/api/shares"]) {
          assert.equal(mock.seen.filter((entry) => entry.path === path).length, 2);
        }
        const catalogRequest = mock.seen.find((entry) => entry.path === "/v1/models");
        assert.ok(catalogRequest);
        assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 2);
        const catalogQuery = new URLSearchParams(catalogRequest.search);
        assert.equal(catalogQuery.get("expectedShareId"), value.shareId);
        assert.equal(catalogQuery.get("expectedProviderId"), value.providerId);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Grok operation receipts recheck current catalog content and authority", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const catalogDrift of ["model_removed", "model_added", "capability_changed", "stale", "wrong_source", "future"]) {
      await t.test(`${operation}/${catalogDrift}`, async () => {
        const value = fixture(operation);
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-catalog-drift-${operation}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, value);
        const mock = await startMockServer(value, { catalogDrift });
        try {
          const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Grok acceptance failed (details redacted)");
          assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 2);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
    await t.test(`${operation}/allowed_source`, async () => {
      const value = fixture(operation);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-catalog-source-${operation}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, value);
      const mock = await startMockServer(value, { catalogDrift: "allowed_source" });
      try {
        const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
        assert.equal(result.code, 0, result.stderr);
        assert.match(result.stdout, /contract_verified.*live_pending/);
        assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 2);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Grok operations reject ambiguous or malformed nonselected catalog IDs", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const kind of ["duplicate", "missing"]) {
      await t.test(`${operation}/${kind}`, async () => {
        const value = fixture(operation);
        value.modelData.push({ id: "grok-extra", object: "model", owned_by: "xai" });
        value.modelData.push(kind === "duplicate"
          ? { id: "grok-extra", object: "model", owned_by: "xai" }
          : { object: "model", owned_by: "xai" });
        value.catalog.digest = digest("cc-switch-server:grok-catalog:v1", value.modelData);
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-catalog-shape-${operation}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, value);
        const mock = await startMockServer(value);
        try {
          const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
          assert.equal(result.code, 1, `${result.stdout}\n${result.stderr}`);
          assert.equal(result.stderr.trim(), "[FAIL] Grok acceptance failed (details redacted)");
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Grok operations allow a pure authoritative catalog row reorder", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      value.modelData.push({ id: "grok-z", object: "model", owned_by: "xai" });
      value.catalog.digest = digest("cc-switch-server:grok-catalog:v1", value.modelData);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-catalog-order-${operation}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, value);
      const mock = await startMockServer(value, { catalogDrift: "row_reordered" });
      try {
        const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
        assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
        assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 2);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Grok catalog digest ignores reorder of distinct canonically equivalent IDs", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      value.modelData = [
        { id: "cafe\u0301", object: "model", owned_by: "xai" },
        { id: "caf\u00e9", object: "model", owned_by: "xai" },
        ...value.modelData,
      ];
      assert.notEqual(value.modelData[0].id, value.modelData[1].id);
      assert.equal(value.modelData[0].id.localeCompare(value.modelData[1].id), 0);
      value.catalog.digest = digest("cc-switch-server:grok-catalog:v1", value.modelData);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-catalog-unicode-order-${operation}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, value);
      const mock = await startMockServer(value, { catalogDrift: "row_reordered" });
      try {
        const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
        assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
        assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 2);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Grok private receipts cannot be hard-linked from inside the repository", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      const insideDirectory = fs.mkdtempSync(path.join(repoRoot, ".grok-receipt-hardlink-"));
      const outsideDirectory = fs.mkdtempSync(path.join(path.dirname(repoRoot), ".grok-receipt-outside-"));
      const insideReceipt = path.join(insideDirectory, "receipt.json");
      const outsideReceipt = path.join(outsideDirectory, "receipt.json");
      writeReceipt(insideReceipt, value);
      fs.linkSync(insideReceipt, outsideReceipt);
      const mock = await startMockServer(value);
      try {
        const result = await runScript(operation, fullEnv(value, mock.url, outsideReceipt));
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /details redacted/);
        assert.deepEqual(mock.seen, []);
      } finally {
        await mock.close();
        fs.rmSync(outsideDirectory, { recursive: true, force: true });
        fs.rmSync(insideDirectory, { recursive: true, force: true });
      }
    });
  }
});

test("Grok receipt cannot be replaced during control-plane validation", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-receipt-swap-${operation}-`));
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
        assert.match(result.stderr, /details redacted/);
        assert.ok(mock.seen.some((entry) => entry.path === "/api/accounts"));
        assert.notEqual(fs.statSync(receiptFile).ino, originalInode);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Grok private receipt must remain the same file through final acceptance", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const change of ["replacement", "in_place"]) {
      await t.test(`${operation}/${change}`, async () => {
        const value = fixture(operation);
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-receipt-final-${operation}-`));
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
          assert.equal(result.stderr.trim(), "[FAIL] Grok acceptance failed (details redacted)");
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

test("Grok final acceptance rejects changed bytes despite unchanged reported metadata", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-receipt-bytes-${operation}-`));
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
        assert.equal(result.stderr.trim(), "[FAIL] Grok acceptance failed (details redacted)");
        assert.ok(mock.seen.some((entry) => entry.path === "/api/accounts"));
        assert.match(fs.readFileSync(receiptFile, "utf8"), /^ +$/);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Grok receipt rejects an in-place rewrite during its snapshot", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-receipt-inplace-${operation}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, value);
      const preload = [
        'import fs from "node:fs";',
        'const originalRead = fs.readSync;',
        `const target = process.env[${JSON.stringify(specs[operation].receiptEnv)}];`,
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
        assert.match(result.stderr, /details redacted/);
        assert.deepEqual(mock.seen, []);
        assert.match(fs.readFileSync(receiptFile, "utf8"), /^ +$/);
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

test("Grok OAuth Account requires explicit false credential and relogin booleans", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const accountFault of [
      "missing_has_api_key",
      "null_has_api_key",
      "string_has_api_key",
      "missing_needs_relogin",
      "null_needs_relogin",
      "string_needs_relogin",
    ]) {
      await t.test(`${operation}/${accountFault}`, async () => {
        const value = fixture(operation);
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-account-${operation}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, value);
        const mock = await startMockServer(value, { accountFault });
        try {
          const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Grok acceptance failed (details redacted)");
          assert.equal(mock.seen.filter((entry) => entry.path === "/api/providers").length, 0);
          assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 0);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
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

test("Grok receipt rejects a catalog that expires before acceptance for every operation", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      value.catalog.fetchedAtMs = Date.now() - 2 * 60 * 60_000 + 1_000;
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-catalog-expiry-${operation}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, value);
      const mock = await startMockServer(value, { finalShareDelayMs: 1_500 });
      try {
        const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
        assert.equal(mock.seen.filter((entry) => entry.path === "/api/shares").length, 2);
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /details redacted/);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Grok operation receipts expiring during final binding validation are rejected", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-receipt-expiry-${operation}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, value, {
        recordedAt: new Date(Date.now() - 24 * 60 * 60_000 + 1_200).toISOString(),
      });
      const mock = await startMockServer(value, { finalShareDelayMs: 2_000 });
      try {
        const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
        assert.equal(mock.seen.filter((entry) => entry.path === "/api/shares").length, 2);
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /details redacted/);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
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

test("Grok receipt rejects binding drift for each operation", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const drift of ["account", "provider", "share"]) {
      await t.test(`${operation}/${drift}`, async () => {
        const value = fixture(operation);
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-${operation}-${drift}-`));
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

test("Grok Share rejects ambiguous or inactive bindings before catalog access", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const shareFault of ["duplicate_conflict", "primary_conflict", "disabled", "paused"]) {
      await t.test(`${operation}/${shareFault}`, async () => {
        const value = fixture(operation);
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-share-${operation}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, value);
        const mock = await startMockServer(value, { shareFault });
        try {
          const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.match(result.stderr, /details redacted/);
          assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 0);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Grok operations reject incomplete or inconsistent Provider and Share scope before catalogs", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const scopeFault of [
      "missing_runtime_revision",
      "missing_view_revision",
      "mismatched_revisions",
      "missing_both_revisions",
      "legacy_top_level_only",
      "missing_share_revision",
      "blank_runtime_fingerprint",
    ]) {
      await t.test(`${operation}/${scopeFault}`, async () => {
        const value = fixture(operation);
        if (scopeFault !== "legacy_top_level_only") value.providerRevision = 0;
        if (scopeFault === "missing_share_revision") value.shareRevision = 0;
        if (scopeFault === "blank_runtime_fingerprint") value.runtimeFingerprint = "   ";
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-scope-${operation}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, value);
        const mock = await startMockServer(value, { scopeFault });
        try {
          const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Grok acceptance failed (details redacted)");
          assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 0);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Grok receipt rejects old future and non-authoritative catalog evidence", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const invalid of ["old", "future", "wrong_source"]) {
      await t.test(`${operation}/${invalid}`, async () => {
        const value = fixture(operation);
        if (invalid === "old") value.catalog.fetchedAtMs -= 3 * 60 * 60 * 1_000;
        if (invalid === "future") value.catalog.fetchedAtMs += 10 * 60 * 1_000;
        if (invalid === "wrong_source") value.catalog.source = "static_registry";
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-${operation}-${invalid}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, value);
        const mock = await startMockServer(value);
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

test("Grok receipt accepts current bound cache and 304 catalog sources only as fixture evidence", async (t) => {
  for (const source of ["cache_fresh", "upstream_not_modified"]) {
    await t.test(source, async () => {
      const value = fixture("inference");
      value.catalog.source = source;
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-${source}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, value);
      const mock = await startMockServer(value);
      try {
        const result = await runScript(value.operation, fullEnv(value, mock.url, receiptFile));
        assert.equal(result.code, 0, result.stderr);
        assert.match(result.stdout, /contract_verified.*live_pending/);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Grok control-plane response bound rejects unfinished oversized bodies", async (t) => {
  for (const [operation, oversizedControl, expectedPath] of [
    ["inference", "declared", "/api/accounts"],
    ["media", "chunked", "/api/providers"],
  ]) {
    await t.test(`${operation}/${oversizedControl}`, async () => {
      const value = fixture(operation);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `grok-oversized-${operation}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, value);
      const mock = await startMockServer(value, { oversizedControl });
      try {
        const startedAt = Date.now();
        const result = await runScript(operation, fullEnv(value, mock.url, receiptFile));
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /details redacted/);
        assert.deepEqual(
          mock.seen.map((entry) => entry.path),
          expectedPath === "/api/accounts"
            ? ["/api/accounts"]
            : ["/api/accounts", "/api/providers"],
        );
        assert.ok(Date.now() - startedAt < 3_000, "oversized response waited for timeout");
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});
