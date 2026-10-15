import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import test from "node:test";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const script = path.join(repoRoot, "scripts/smoke/kiro-real-receipt.mjs");
const contract = JSON.parse(
  fs.readFileSync(
    path.join(repoRoot, "assets/contract/kiro-reference-delta.json"),
    "utf8",
  ),
);
const targetCommit = execFileSync("git", ["rev-parse", "HEAD"], {
  cwd: repoRoot,
  encoding: "utf8",
}).trim();
const serverSecret = "kiro-server-secret-for-harness";
const routerSecret = "kiro-router-secret-for-harness";
const authKinds = ["builder_id", "idc", "social", "api_key"];
const regions = ["us-east-1", "eu-central-1"];

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

function catalogDigest(app, data) {
  return digest(
    `cc-switch-server:kiro-${app}-catalog:v1`,
    [...data].sort((left, right) => left.id < right.id ? -1 : left.id > right.id ? 1 : 0),
  );
}

function scopeName(authKind, region) {
  return `${authKind}_${region}`.replaceAll("-", "_").toUpperCase();
}

function spec(authKind, region) {
  const scope = scopeName(authKind, region);
  return {
    scope,
    accountEnv: `KIRO_${scope}_TEST_ACCOUNT`,
    claudeProviderEnv: `CC_SWITCH_KIRO_${scope}_CLAUDE_PROVIDER_ID`,
    codexProviderEnv: `CC_SWITCH_KIRO_${scope}_CODEX_PROVIDER_ID`,
    shareEnv: `CC_SWITCH_KIRO_${scope}_SHARE_ID`,
    modelEnv: `CC_SWITCH_KIRO_${scope}_MODEL`,
    receiptEnv: `KIRO_${scope}_REAL_RECEIPT_FILE`,
  };
}

function fixture(authKind, region) {
  const scope = `${authKind}-${region}`;
  const value = {
    authKind,
    region,
    spec: spec(authKind, region),
    accountId: `kiro-${scope}-account`,
    claudeProviderId: `kiro-${scope}-claude-provider`,
    codexProviderId: `kiro-${scope}-codex-provider`,
    shareId: `kiro-${scope}-share`,
    signedUser: `signed-${authKind}@example.test`,
    sessionId: `kiro-${scope}-session`,
    model: "claude-sonnet-4-5",
    authIdentityGeneration: 7,
    tokenRefreshGeneration: authKind === "api_key" ? 0 : 4,
    claudeProviderRevision: 3,
    codexProviderRevision: 5,
    shareRevision: 9,
    claudeRuntimeFingerprint: `kiro-${scope}-claude-runtime`,
    codexRuntimeFingerprint: `kiro-${scope}-codex-runtime`,
    modelData: [{ id: "claude-sonnet-4-5", object: "model", owned_by: "kiro" }],
  };
  value.catalogs = {
    claude: {
      source: "kiro_list_available_models",
      fetchedAtMs: Date.now(),
      stale: false,
      digest: catalogDigest("claude", value.modelData),
    },
    codex: {
      source: "kiro_list_available_models",
      fetchedAtMs: Date.now(),
      stale: false,
      digest: catalogDigest("codex", value.modelData),
    },
  };
  return value;
}

function updateCatalogDigests(value, { raw = false } = {}) {
  for (const app of ["claude", "codex"]) {
    const data = value.modelDataByApp?.[app] ?? value.modelData;
    // Raw digests let malformed fixtures reach the catalog gate in the old validator.
    value.catalogs[app].digest = raw
      ? digest(`cc-switch-server:kiro-${app}-catalog:v1`, data)
      : catalogDigest(app, data);
  }
}

function userNamespaceDigest(value) {
  return digest(
    "cc-switch-server:kiro-signed-user:v1",
    value.signedUser.toLowerCase(),
  );
}

function sessionDigest(value) {
  return digest("cc-switch-server:kiro-session:v1", value.sessionId);
}

function generations(value) {
  const userDigest = userNamespaceDigest(value);
  return {
    authIdentityGeneration: value.authIdentityGeneration,
    tokenRefreshGeneration: value.tokenRefreshGeneration,
    claudeProviderRevision: value.claudeProviderRevision,
    codexProviderRevision: value.codexProviderRevision,
    shareRevision: value.shareRevision,
    credentialIdentityDigest: digest("cc-switch-server:kiro-credential:v1", {
      accountId: value.accountId,
      authKind: value.authKind,
      region: value.region,
      authIdentityGeneration: value.authIdentityGeneration,
      tokenRefreshGeneration: value.tokenRefreshGeneration,
    }),
    claudeBindingDigest: digest("cc-switch-server:kiro-provider-binding:v1", {
      app: "claude",
      providerId: value.claudeProviderId,
      runtimeFingerprint: value.claudeRuntimeFingerprint,
      accountId: value.accountId,
      authIdentityGeneration: value.authIdentityGeneration,
    }),
    codexBindingDigest: digest("cc-switch-server:kiro-provider-binding:v1", {
      app: "codex",
      providerId: value.codexProviderId,
      runtimeFingerprint: value.codexRuntimeFingerprint,
      accountId: value.accountId,
      authIdentityGeneration: value.authIdentityGeneration,
    }),
    shareBindingDigest: digest("cc-switch-server:kiro-share-binding:v1", {
      shareId: value.shareId,
      shareRevision: value.shareRevision,
      claudeProviderId: value.claudeProviderId,
      codexProviderId: value.codexProviderId,
      userNamespaceDigest: userDigest,
    }),
  };
}

function scopeDigest(value) {
  const scopedGenerations = generations(value);
  return digest("cc-switch-server:kiro-real-scope:v1", {
    authKind: value.authKind,
    region: value.region,
    targetCommit,
    accountId: value.accountId,
    generations: scopedGenerations,
    claudeProviderId: value.claudeProviderId,
    codexProviderId: value.codexProviderId,
    shareId: value.shareId,
    userNamespaceDigest: userNamespaceDigest(value),
    sessionDigest: sessionDigest(value),
    model: value.model,
    catalogs: value.catalogs,
    harnessRevision: 1,
  });
}

function expectedDecisions(value) {
  return {
    sameAccount401:
      value.authKind === "api_key"
        ? "terminal_nonrefreshable"
        : "replayed_once_precommit",
    second401: "terminal",
    identityGenerationDrift: "terminal",
    crossAuthKindFallback: "disabled",
    crossRegionFallback: "disabled",
    crossAccountFallback: "disabled",
    crossProviderFallback: "disabled",
    crossShareFallback: "disabled",
    postCommitReplay: "disabled",
    catalogStale: "same_identity_only",
    compactRuntime: "disabled",
    receiptAutoEnablesCompact: "false",
    sharedCacheRuntime: "disabled",
  };
}

function receipt(value, overrides = {}) {
  return {
    schemaVersion: 1,
    providerFamily: "kiro",
    authKind: value.authKind,
    region: value.region,
    verificationState: "contract_verified",
    liveState: "live_pending",
    targetCommit,
    harnessRevision: 1,
    recordedAt: new Date().toISOString(),
    scopeDigest: scopeDigest(value),
    userNamespaceDigest: userNamespaceDigest(value),
    sessionDigest: sessionDigest(value),
    model: value.model,
    catalogs: value.catalogs,
    checks: Object.fromEntries(
      contract.realAcceptance.requiredChecks.map((name) => [name, "pass"]),
    ),
    bodyHashes: Object.fromEntries(
      contract.realAcceptance.requiredBodyHashes.map((name) => [
        name,
        digest(`kiro-fixture:${value.authKind}:${value.region}`, name),
      ]),
    ),
    measurements: Object.fromEntries(
      contract.realAcceptance.requiredMeasurements.map((name) => [name, 1]),
    ),
    generations: generations(value),
    decisions: expectedDecisions(value),
    decoyRequests: {
      otherAccount: 0,
      otherProvider: 0,
      otherShare: 0,
      otherAuthKind: 0,
      otherRegion: 0,
      amazonQ: 0,
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

function clearedMatrixEnv() {
  const cleared = {
    KIRO_TEST_ACCOUNT: "",
    CC_SWITCH_KIRO_SIGNED_USER: "",
    CC_SWITCH_KIRO_SESSION_ID: "",
  };
  for (const authKind of authKinds) {
    for (const region of regions) {
      const current = spec(authKind, region);
      for (const name of [
        current.accountEnv,
        current.claudeProviderEnv,
        current.codexProviderEnv,
        current.shareEnv,
        current.modelEnv,
        current.receiptEnv,
      ]) {
        cleared[name] = "";
      }
    }
  }
  return cleared;
}

function runScript(value, overrides = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(
      process.execPath,
      [script, "--auth-kind", value.authKind, "--region", value.region],
      {
        cwd: repoRoot,
        env: {
          ...process.env,
          ...clearedMatrixEnv(),
          RUN_REAL: "1",
          CC_SWITCH_KIRO_HARNESS_MODE: "fixture",
          SERVER_URL: "",
          CC_SWITCH_SERVER_TOKEN: "",
          CC_SWITCH_SHARE_URL: "",
          ROUTER_API_TOKEN: "",
          ROUTER_API_TOKEN_HEADER: "Authorization",
          CC_SWITCH_REAL_TIMEOUT_MS: "5000",
          ...overrides,
        },
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
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
  { providerGeneration = value.authIdentityGeneration, catalogStale = false, catalogDrift = null, drift = "", shareFault = "", scopeFault = "", accountReloginFault = "", oversizedControl = "", onFirstControl = null, accountAuthKind = value.authKind, accountRegion = value.region, omitAccountScope = false, finalShareDelayMs = 0 } = {},
) {
  const seen = [];
  let firstControlSeen = false;
  let accountReads = 0;
  let providerReads = 0;
  let shareReads = 0;
  const catalogReads = { claude: 0, codex: 0 };
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
            email: `${value.authKind}@example.test`,
            providerType: "kiro_oauth",
            authIdentityGeneration: value.authIdentityGeneration,
            tokenRefreshGeneration: value.tokenRefreshGeneration + (drift === "account" && accountReads > 1 ? 1 : 0),
            tokenType: value.authKind === "api_key" ? "API_KEY" : "Bearer",
            hasAccessToken: true,
            hasRefreshToken: value.authKind !== "api_key",
            hasApiKey: value.authKind === "api_key",
            hasProfile: true,
            hasRaw: true,
            needsRelogin: accountReloginFault === "missing" ? undefined
              : accountReloginFault === "null" ? null
                : accountReloginFault === "string" ? "false" : false,
            ...(!omitAccountScope && {
              kiroCredentialScope: {
                authKind: accountAuthKind,
                runtimeRegion: accountRegion,
              },
            }),
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
      const provider = (app, id, revision, runtimeFingerprint) => {
        const fault = scopeFault.startsWith(`${app}_`) ? scopeFault.slice(app.length + 1) : "";
        const currentRevision = revision + (drift === `${app}_provider` && providerReads > 1 ? 1 : 0);
        return {
          app,
          provider: { id },
          providerType: "kiro_oauth",
          providerTypeId: "kiro_oauth",
          revision: ["missing_view_revision", "legacy_top_level_only"].includes(fault)
            ? undefined : currentRevision + (fault === "mismatched_revisions" ? 1 : 0),
          ...(fault === "legacy_top_level_only" ? { providerRevision: currentRevision } : {}),
          runtime: {
            providerRevision: ["missing_runtime_revision", "legacy_top_level_only"].includes(fault)
              ? undefined : currentRevision,
            driverId: "special.kiro",
            configurationState: "ready",
            runtimeFingerprint,
            authRef: {
              kind: "managed_account",
              expectedProviderType: "kiro_oauth",
              accountId: value.accountId,
              authIdentityGeneration: providerGeneration,
            },
          },
        };
      };
      sendJson(response, 200, {
        ok: true,
        providers: [
          provider(
            "claude",
            value.claudeProviderId,
            value.claudeProviderRevision,
            value.claudeRuntimeFingerprint,
          ),
          provider(
            "codex",
            value.codexProviderId,
            value.codexProviderRevision,
            value.codexRuntimeFingerprint,
          ),
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
            providerId: shareFault === "primary_conflict" ? "kiro-other-provider" : value.claudeProviderId,
            providerType: "kiro_oauth",
            enabled: shareFault !== "disabled",
            status: shareFault === "paused" ? "paused" : "active",
            configRevision: scopeFault === "missing_share_revision" ? undefined
              : value.shareRevision + (drift === "share" && shareReads > 1 ? 1 : 0),
            bindings: [
              {
                app: "claude",
                providerId: value.claudeProviderId,
                providerType: "kiro_oauth",
              },
              {
                app: "codex",
                providerId: value.codexProviderId,
                providerType: "kiro_oauth",
              },
              ...(shareFault === "duplicate_claude"
                ? [{ app: "claude", providerId: "kiro-other-provider", providerType: "kiro_oauth" }]
                : []),
              ...(shareFault === "duplicate_codex"
                ? [{ app: "codex", providerId: "kiro-other-provider", providerType: "kiro_oauth" }]
                : []),
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
      const app = url.searchParams.get("app");
      if (app !== "claude" && app !== "codex") {
        sendJson(response, 400, { error: "wrong app" });
        return;
      }
      catalogReads[app] += 1;
      const modelData = value.modelDataByApp?.[app] ?? value.modelData;
      const fault = catalogDrift?.app === app && catalogReads[app] > 1
        ? catalogDrift.fault : "";
      const expectedProvider = app === "claude" ? value.claudeProviderId : value.codexProviderId;
      if (url.searchParams.get("providerId") !== expectedProvider) {
        sendJson(response, 400, { error: "wrong provider" });
        return;
      }
      sendJson(response, 200, {
        data: fault === "model_removed" ? []
          : fault === "model_added"
            ? [...modelData, { id: "kiro-extra", object: "model", owned_by: "kiro" }]
            : fault === "capability_changed"
              ? modelData.map((entry) => entry.id === value.model
                ? { ...entry, capabilities: { tools: false } }
                : entry)
            : fault === "reordered"
              ? [...modelData].reverse()
            : modelData,
        source: fault === "wrong_source" ? "static_registry"
          : fault === "allowed_source" ? "kiro_account_cache"
            : value.catalogs[app].source,
        fetchedAtMs: fault === "future"
          ? Date.now() + 10 * 60_000 : value.catalogs[app].fetchedAtMs,
        stale: catalogStale || fault === "stale",
      });
      return;
    }
    sendJson(response, 404, { error: "not found" });
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  return {
    origin: `http://127.0.0.1:${address.port}`,
    seen,
    close: () => new Promise((resolve) => server.close(resolve)),
  };
}

function completeEnv(value, origin, receiptFile) {
  return {
    SERVER_URL: origin,
    CC_SWITCH_SERVER_TOKEN: serverSecret,
    CC_SWITCH_SHARE_URL: origin,
    ROUTER_API_TOKEN: routerSecret,
    CC_SWITCH_KIRO_SIGNED_USER: value.signedUser,
    CC_SWITCH_KIRO_SESSION_ID: value.sessionId,
    [value.spec.accountEnv]: value.accountId,
    [value.spec.claudeProviderEnv]: value.claudeProviderId,
    [value.spec.codexProviderEnv]: value.codexProviderId,
    [value.spec.shareEnv]: value.shareId,
    [value.spec.modelEnv]: value.model,
    [value.spec.receiptEnv]: receiptFile,
  };
}

async function runFixture(value, options = {}, receiptOverrides = {}) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "kiro-receipt-"));
  const receiptFile = path.join(directory, "receipt.json");
  writeReceipt(receiptFile, value, receiptOverrides);
  const server = await startMockServer(value, options);
  try {
    const result = await runScript(value, completeEnv(value, server.origin, receiptFile));
    return { result, seen: server.seen };
  } finally {
    await server.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
}

test("Kiro receipt harness reports missing scope inputs without claiming live evidence", async () => {
  const value = fixture("builder_id", "us-east-1");
  const result = await runScript(value);
  assert.equal(result.code, 0, result.stderr);
  const output = JSON.parse(result.stdout);
  assert.equal(output.verificationState, "blocked_inputs");
  assert.equal(output.liveState, "live_pending");
  assert.ok(output.missingInputs.includes(value.spec.accountEnv));
  assert.ok(output.missingInputs.includes(value.spec.receiptEnv));
});

test("Kiro receipt rejects a different bound auth kind or runtime region", async (t) => {
  for (const [name, authKind, region, options] of [
    ["auth_kind", "builder_id", "us-east-1", { accountAuthKind: "idc" }],
    ["runtime_region", "api_key", "eu-central-1", { accountRegion: "us-east-1" }],
    ["missing_scope", "social", "us-east-1", { omitAccountScope: true }],
  ]) {
    await t.test(name, async () => {
      const value = fixture(authKind, region);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `kiro-scope-${name}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, value);
      const server = await startMockServer(value, options);
      try {
        const result = await runScript(value, completeEnv(value, server.origin, receiptFile));
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /details redacted/);
      } finally {
        await server.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("all Kiro scopes require an explicit false Account relogin flag", async (t) => {
  for (const authKind of authKinds) {
    for (const region of regions) {
      for (const accountReloginFault of ["missing", "null", "string"]) {
        await t.test(`${authKind}/${region}/${accountReloginFault}`, async () => {
          const value = fixture(authKind, region);
          const { result, seen } = await runFixture(value, { accountReloginFault });
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Kiro acceptance failed (details redacted)");
          assert.equal(seen.filter((entry) => entry.path === "/api/providers").length, 0);
          assert.equal(seen.filter((entry) => entry.path === "/v1/models").length, 0);
        });
      }
    }
  }
});

test("all eight Kiro auth-kind and region receipts remain independent", async (t) => {
  for (const authKind of authKinds) {
    for (const region of regions) {
      await t.test(`${authKind}/${region}`, async () => {
        const value = fixture(authKind, region);
        const { result, seen } = await runFixture(value);
        assert.equal(result.code, 0, `${authKind}/${region}: ${result.stderr}`);
        assert.match(result.stdout, /verificationState=contract_verified/);
        assert.match(result.stdout, /liveState=live_pending/);
        assert.deepEqual(
          seen.map((entry) => entry.path),
          ["/api/accounts", "/api/providers", "/api/shares", "/v1/models", "/v1/models", "/api/accounts", "/api/providers", "/api/shares", "/v1/models", "/v1/models"],
        );
        const catalogRequests = seen.filter((candidate) => candidate.path === "/v1/models");
        assert.deepEqual(
          catalogRequests.map((entry) => new URLSearchParams(entry.search).get("app")).sort(),
          ["claude", "claude", "codex", "codex"],
        );
        for (const entry of catalogRequests) {
          const query = new URLSearchParams(entry.search);
          const expectedProviderId = query.get("app") === "claude"
            ? value.claudeProviderId
            : value.codexProviderId;
          assert.equal(query.get("expectedShareId"), value.shareId);
          assert.equal(query.get("expectedProviderId"), expectedProviderId);
        }
        assert.equal(
          seen.filter((entry) => entry.authorization === `Bearer ${serverSecret}`).length,
          6,
        );
        assert.equal(
          seen.filter((entry) => entry.authorization === `Bearer ${routerSecret}`).length,
          4,
        );
      });
    }
  }
});

test("all Kiro scopes recheck both catalog contents and authority before acceptance", async (t) => {
  for (const authKind of authKinds) {
    for (const region of regions) {
      for (const app of ["claude", "codex"]) {
        for (const fault of ["model_removed", "model_added", "capability_changed", "stale", "wrong_source", "future"]) {
          await t.test(`${authKind}/${region}/${app}/${fault}`, async () => {
            const value = fixture(authKind, region);
            const { result, seen } = await runFixture(value, { catalogDrift: { app, fault } });
            assert.equal(result.code, 1, result.stdout);
            assert.equal(result.stderr.trim(), "[FAIL] Kiro acceptance failed (details redacted)");
            const expectedCatalogRequests = app === "claude" && !["model_added", "capability_changed"].includes(fault) ? 3 : 4;
            assert.equal(seen.filter((entry) => entry.path === "/v1/models").length, expectedCatalogRequests);
          });
        }
        await t.test(`${authKind}/${region}/${app}/allowed_source`, async () => {
          const value = fixture(authKind, region);
          const { result, seen } = await runFixture(value, {
            catalogDrift: { app, fault: "allowed_source" },
          });
          assert.equal(result.code, 0, result.stderr);
          assert.match(result.stdout, /contract_verified.*live_pending/);
          assert.equal(seen.filter((entry) => entry.path === "/v1/models").length, 4);
        });
      }
    }
  }
});

test("all Kiro scopes reject incomplete or duplicate IDs across the full catalog", async (t) => {
  for (const authKind of authKinds) {
    for (const region of regions) {
      for (const app of ["claude", "codex"]) {
        for (const fault of ["missing", "blank", "duplicate"]) {
          await t.test(`${authKind}/${region}/${app}/${fault}`, async () => {
            const value = fixture(authKind, region);
            value.modelDataByApp = { [app]: [...value.modelData] };
            value.modelDataByApp[app].push(
              { id: "kiro-other", object: "model", owned_by: "kiro" },
              fault === "missing"
                ? { object: "model", owned_by: "kiro" }
                : { id: fault === "blank" ? " " : "kiro-other", object: "model", owned_by: "kiro" },
            );
            updateCatalogDigests(value, { raw: true });
            const { result, seen } = await runFixture(value);
            assert.equal(result.code, 1, result.stdout);
            assert.equal(result.stderr.trim(), "[FAIL] Kiro acceptance failed (details redacted)");
            assert.equal(seen.filter((entry) => entry.path === "/v1/models").length, app === "claude" ? 1 : 2);
          });
        }
      }
    }
  }
});

test("all Kiro scopes accept catalog row reordering without changing model content", async (t) => {
  for (const authKind of authKinds) {
    for (const region of regions) {
      for (const app of ["claude", "codex"]) {
        await t.test(`${authKind}/${region}/${app}`, async () => {
          const value = fixture(authKind, region);
          value.modelData.push({ id: "kiro-other", object: "model", owned_by: "kiro" });
          updateCatalogDigests(value);
          const { result, seen } = await runFixture(value, {
            catalogDrift: { app, fault: "reordered" },
          });
          assert.equal(result.code, 0, result.stderr);
          assert.match(result.stdout, /contract_verified.*live_pending/);
          assert.equal(seen.filter((entry) => entry.path === "/v1/models").length, 4);
        });
      }
    }
  }
});

test("Kiro private receipts cannot be hard-linked from inside the repository", async (t) => {
  for (const authKind of authKinds) {
    for (const region of regions) {
      await t.test(`${authKind}/${region}`, async () => {
        const value = fixture(authKind, region);
        const insideDirectory = fs.mkdtempSync(path.join(repoRoot, ".kiro-receipt-hardlink-"));
        const outsideDirectory = fs.mkdtempSync(path.join(path.dirname(repoRoot), ".kiro-receipt-outside-"));
        const insideReceipt = path.join(insideDirectory, "receipt.json");
        const outsideReceipt = path.join(outsideDirectory, "receipt.json");
        writeReceipt(insideReceipt, value);
        fs.linkSync(insideReceipt, outsideReceipt);
        const server = await startMockServer(value);
        try {
          const result = await runScript(value, completeEnv(value, server.origin, outsideReceipt));
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Kiro acceptance failed (details redacted)");
          assert.deepEqual(server.seen, []);
        } finally {
          await server.close();
          fs.rmSync(outsideDirectory, { recursive: true, force: true });
          fs.rmSync(insideDirectory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Kiro receipt cannot be replaced during control-plane validation", async (t) => {
  for (const authKind of authKinds) {
    for (const region of regions) {
      await t.test(`${authKind}/${region}`, async () => {
        const value = fixture(authKind, region);
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `kiro-receipt-swap-${authKind}-`));
        const receiptFile = path.join(directory, "receipt.json");
        const replacementFile = path.join(directory, "replacement.json");
        writeReceipt(receiptFile, value, { model: "not-the-selected-model" });
        writeReceipt(replacementFile, value);
        const originalInode = fs.statSync(receiptFile).ino;
        const server = await startMockServer(value, {
          onFirstControl: () => fs.renameSync(replacementFile, receiptFile),
        });
        try {
          const result = await runScript(value, completeEnv(value, server.origin, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Kiro acceptance failed (details redacted)");
          assert.ok(server.seen.some((entry) => entry.path === "/api/accounts"));
          assert.notEqual(fs.statSync(receiptFile).ino, originalInode);
        } finally {
          await server.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Kiro private receipt must remain the same file through final acceptance", async (t) => {
  for (const authKind of authKinds) {
    for (const region of regions) {
      for (const change of ["replacement", "in_place"]) {
        await t.test(`${authKind}/${region}/${change}`, async () => {
          const value = fixture(authKind, region);
          const directory = fs.mkdtempSync(path.join(os.tmpdir(), `kiro-receipt-final-${authKind}-`));
          const receiptFile = path.join(directory, "receipt.json");
          const replacementFile = path.join(directory, "replacement.json");
          writeReceipt(receiptFile, value);
          if (change === "replacement") {
            fs.writeFileSync(replacementFile, "not the accepted receipt\n", { mode: 0o600 });
          }
          const originalInode = fs.statSync(receiptFile).ino;
          const server = await startMockServer(value, {
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
            const result = await runScript(value, completeEnv(value, server.origin, receiptFile));
            assert.equal(result.code, 1, result.stdout);
            assert.equal(result.stderr.trim(), "[FAIL] Kiro acceptance failed (details redacted)");
            assert.ok(server.seen.some((entry) => entry.path === "/api/accounts"));
            if (change === "replacement") {
              assert.notEqual(fs.statSync(receiptFile).ino, originalInode);
            } else {
              assert.equal(fs.statSync(receiptFile).ino, originalInode);
            }
          } finally {
            await server.close();
            fs.rmSync(directory, { recursive: true, force: true });
          }
        });
      }
    }
  }
});

test("Kiro final acceptance rejects changed bytes despite unchanged reported metadata", async (t) => {
  for (const authKind of authKinds) {
    for (const region of regions) {
      await t.test(`${authKind}/${region}`, async () => {
        const value = fixture(authKind, region);
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `kiro-receipt-bytes-${authKind}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, value);
        const preload = [
          'import fs from "node:fs";',
          `const target = process.env[${JSON.stringify(value.spec.receiptEnv)}];`,
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
        const server = await startMockServer(value, {
          onFirstControl: () => {
            fs.writeFileSync(receiptFile, " ".repeat(fs.statSync(receiptFile).size), { flag: "r+" });
          },
        });
        try {
          const result = await runScript(value, {
            ...completeEnv(value, server.origin, receiptFile),
            NODE_OPTIONS: `--import=data:text/javascript,${encodeURIComponent(preload)}`,
          });
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Kiro acceptance failed (details redacted)");
          assert.ok(server.seen.some((entry) => entry.path === "/api/accounts"));
          assert.match(fs.readFileSync(receiptFile, "utf8"), /^ +$/);
        } finally {
          await server.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Kiro receipt rejects an in-place rewrite during its snapshot", async (t) => {
  for (const authKind of authKinds) {
    for (const region of regions) {
      await t.test(`${authKind}/${region}`, async () => {
        const value = fixture(authKind, region);
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `kiro-receipt-inplace-${authKind}-`));
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
        const server = await startMockServer(value);
        try {
          const result = await runScript(value, {
            ...completeEnv(value, server.origin, receiptFile),
            NODE_OPTIONS: `--import=data:text/javascript,${encodeURIComponent(preload)}`,
          });
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Kiro acceptance failed (details redacted)");
          assert.deepEqual(server.seen, []);
          assert.match(fs.readFileSync(receiptFile, "utf8"), /^ +$/);
        } finally {
          await server.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Kiro receipt rejects generation drift and stale catalogs", async () => {
  const value = fixture("idc", "eu-central-1");
  const drift = await runFixture(value, {
    providerGeneration: value.authIdentityGeneration + 1,
  });
  assert.equal(drift.result.code, 1);
  assert.equal(drift.result.stderr.trim(), "[FAIL] Kiro acceptance failed (details redacted)");

  const stale = await runFixture(value, { catalogStale: true });
  assert.equal(stale.result.code, 1);
  assert.equal(stale.result.stderr.trim(), "[FAIL] Kiro acceptance failed (details redacted)");
});

test("Kiro receipt rejects cross-region identity and incomplete recovery decisions", async () => {
  const value = fixture("social", "us-east-1");
  const wrongRegion = await runFixture(value, {}, { region: "eu-central-1" });
  assert.equal(wrongRegion.result.code, 1);

  const decisions = expectedDecisions(value);
  delete decisions.crossRegionFallback;
  const incomplete = await runFixture(value, {}, { decisions });
  assert.equal(incomplete.result.code, 1);
  assert.equal(
    incomplete.result.stderr.trim(),
    "[FAIL] Kiro acceptance failed (details redacted)",
  );
});

test("Kiro fixture receipt can never impersonate live_verified", async () => {
  const value = fixture("api_key", "eu-central-1");
  const forged = await runFixture(value, {}, {
    verificationState: "live_verified",
    liveState: "live_verified",
  });
  assert.equal(forged.result.code, 1);
  assert.equal(forged.result.stderr.trim(), "[FAIL] Kiro acceptance failed (details redacted)");
});

test("all Kiro auth-kind and region receipts reject binding drift", async (t) => {
  for (const authKind of authKinds) {
    for (const region of regions) {
      for (const drift of ["account", "claude_provider", "codex_provider", "share"]) {
        await t.test(`${authKind}/${region}/${drift}`, async () => {
          const value = fixture(authKind, region);
          const { result } = await runFixture(value, { drift });
          assert.equal(result.code, 1, result.stdout);
          assert.match(result.stderr, /details redacted/);
        });
      }
    }
  }
});

test("all Kiro scopes reject ambiguous or inactive Shares before catalog access", async (t) => {
  for (const authKind of authKinds) {
    for (const region of regions) {
      for (const shareFault of ["duplicate_claude", "duplicate_codex", "primary_conflict", "disabled", "paused"]) {
        await t.test(`${authKind}/${region}/${shareFault}`, async () => {
          const value = fixture(authKind, region);
          const { result, seen } = await runFixture(value, { shareFault });
          assert.equal(result.code, 1, result.stdout);
          assert.match(result.stderr, /details redacted/);
          assert.equal(seen.filter((entry) => entry.path === "/v1/models").length, 0);
        });
      }
    }
  }
});

test("all Kiro scopes reject incomplete or inconsistent dual-Provider revisions before catalogs", async (t) => {
  const providerFaults = [
    "missing_view_revision",
    "missing_runtime_revision",
    "mismatched_revisions",
    "legacy_top_level_only",
    "blank_runtime_fingerprint",
  ];
  for (const authKind of authKinds) {
    for (const region of regions) {
      for (const scopeFault of [
        ...["claude", "codex"].flatMap((app) => providerFaults.map((fault) => `${app}_${fault}`)),
        "missing_share_revision",
      ]) {
        await t.test(`${authKind}/${region}/${scopeFault}`, async () => {
          const value = fixture(authKind, region);
          value.claudeProviderRevision = 0;
          value.codexProviderRevision = 0;
          if (scopeFault === "missing_share_revision") value.shareRevision = 0;
          if (scopeFault === "claude_blank_runtime_fingerprint") value.claudeRuntimeFingerprint = "   ";
          if (scopeFault === "codex_blank_runtime_fingerprint") value.codexRuntimeFingerprint = "   ";
          const { result, seen } = await runFixture(value, { scopeFault });
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Kiro acceptance failed (details redacted)");
          assert.equal(seen.filter((entry) => entry.path === "/v1/models").length, 0);
        });
      }
    }
  }
});

test("all Kiro auth-kind and region receipts reject old or unrelated model catalogs", async (t) => {
  for (const authKind of authKinds) {
    for (const region of regions) {
      for (const invalid of ["old", "future", "wrong_source"]) {
        await t.test(`${authKind}/${region}/${invalid}`, async () => {
          const value = fixture(authKind, region);
          if (invalid === "old") value.catalogs.claude.fetchedAtMs -= 10 * 60 * 1_000;
          if (invalid === "future") value.catalogs.codex.fetchedAtMs += 10 * 60 * 1_000;
          if (invalid === "wrong_source") value.catalogs.claude.source = "static_registry";
          const { result } = await runFixture(value);
          assert.equal(result.code, 1, result.stdout);
          assert.match(result.stderr, /details redacted/);
        });
      }
    }
  }
});

test("all Kiro scopes reject either catalog if it expires during final binding validation", async (t) => {
  for (const authKind of authKinds) {
    for (const region of regions) {
      for (const app of ["claude", "codex"]) {
        await t.test(`${authKind}/${region}/${app}`, async () => {
          const value = fixture(authKind, region);
          value.catalogs[app].fetchedAtMs = Date.now() - 5 * 60 * 1_000 + 1_000;
          const { result, seen } = await runFixture(value, { finalShareDelayMs: 1_500 });
          assert.equal(result.code, 1, result.stdout);
          assert.match(result.stderr, /details redacted/);
          assert.equal(seen.filter((entry) => entry.path === "/v1/models").length, app === "claude" ? 3 : 4);
        });
      }
    }
  }
});

test("all Kiro auth-kind and region receipts expiring during final binding validation are rejected", async (t) => {
  for (const authKind of authKinds) {
    for (const region of regions) {
      await t.test(`${authKind}/${region}`, async () => {
        const value = fixture(authKind, region);
        const { result, seen } = await runFixture(
          value,
          { finalShareDelayMs: 2_000 },
          { recordedAt: new Date(Date.now() - 24 * 60 * 60 * 1_000 + 1_200).toISOString() },
        );
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /details redacted/);
        assert.equal(seen.filter((entry) => entry.path === "/api/shares").length, 2);
      });
    }
  }
});

test("Kiro current bound cache remains valid only as fixture evidence", async () => {
  const value = fixture("builder_id", "us-east-1");
  value.catalogs.claude.source = "kiro_account_cache";
  value.catalogs.codex.source = "kiro_account_cache";
  const { result } = await runFixture(value);
  assert.equal(result.code, 0, result.stderr);
  assert.match(result.stdout, /contract_verified.*live_pending/);
});

test("Kiro control-plane response bound rejects unfinished oversized bodies", async (t) => {
  for (const [authKind, region, oversizedControl, expectedPaths] of [
    ["builder_id", "us-east-1", "declared", ["/api/accounts"]],
    ["api_key", "eu-central-1", "chunked", ["/api/accounts", "/api/providers"]],
  ]) {
    await t.test(`${authKind}/${region}/${oversizedControl}`, async () => {
      const value = fixture(authKind, region);
      const startedAt = Date.now();
      const { result, seen } = await runFixture(value, { oversizedControl });
      assert.equal(result.code, 1, result.stdout);
      assert.match(result.stderr, /details redacted/);
      assert.deepEqual(seen.map((entry) => entry.path), expectedPaths);
      assert.ok(Date.now() - startedAt < 3_000, "oversized response waited for timeout");
    });
  }
});
