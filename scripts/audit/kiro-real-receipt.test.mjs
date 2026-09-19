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
      source: "kiro_upstream_fetch_available_models",
      fetchedAtMs: Date.now(),
      stale: false,
      digest: digest("cc-switch-server:kiro-claude-catalog:v1", value.modelData),
    },
    codex: {
      source: "kiro_upstream_fetch_available_models",
      fetchedAtMs: Date.now(),
      stale: false,
      digest: digest("cc-switch-server:kiro-codex-catalog:v1", value.modelData),
    },
  };
  return value;
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
  { providerGeneration = value.authIdentityGeneration, catalogStale = false } = {},
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
            email: `${value.authKind}@example.test`,
            providerType: "kiro_oauth",
            authIdentityGeneration: value.authIdentityGeneration,
            tokenRefreshGeneration: value.tokenRefreshGeneration,
            tokenType: value.authKind === "api_key" ? "API_KEY" : "Bearer",
            hasAccessToken: true,
            hasRefreshToken: value.authKind !== "api_key",
            hasApiKey: value.authKind === "api_key",
            hasProfile: true,
            hasRaw: true,
            needsRelogin: false,
          },
        ],
      });
      return;
    }
    if (request.method === "GET" && url.pathname === "/api/providers") {
      const provider = (app, id, revision, runtimeFingerprint) => ({
        app,
        provider: { id },
        providerType: "kiro_oauth",
        providerTypeId: "kiro_oauth",
        providerRevision: revision,
        runtime: {
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
      });
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
      sendJson(response, 200, {
        ok: true,
        shares: [
          {
            id: value.shareId,
            configRevision: value.shareRevision,
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
            ],
          },
        ],
      });
      return;
    }
    if (request.method === "GET" && url.pathname === "/v1/models") {
      const app = url.searchParams.get("app");
      const expectedProvider = app === "claude" ? value.claudeProviderId : value.codexProviderId;
      if (url.searchParams.get("providerId") !== expectedProvider) {
        sendJson(response, 400, { error: "wrong provider" });
        return;
      }
      sendJson(response, 200, {
        data: value.modelData,
        source: value.catalogs[app].source,
        fetchedAtMs: value.catalogs[app].fetchedAtMs,
        stale: catalogStale,
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

test("all eight Kiro auth-kind and region receipts remain independent", async () => {
  for (const authKind of authKinds) {
    for (const region of regions) {
      const value = fixture(authKind, region);
      const { result, seen } = await runFixture(value);
      assert.equal(result.code, 0, `${authKind}/${region}: ${result.stderr}`);
      assert.match(result.stdout, /verificationState=contract_verified/);
      assert.match(result.stdout, /liveState=live_pending/);
      assert.deepEqual(
        seen.map((entry) => entry.path),
        ["/api/accounts", "/api/providers", "/api/shares", "/v1/models", "/v1/models"],
      );
      assert.equal(
        seen.filter((entry) => entry.authorization === `Bearer ${serverSecret}`).length,
        3,
      );
      assert.equal(
        seen.filter((entry) => entry.authorization === `Bearer ${routerSecret}`).length,
        2,
      );
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
