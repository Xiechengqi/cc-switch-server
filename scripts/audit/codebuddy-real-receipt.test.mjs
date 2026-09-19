import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import test from "node:test";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const script = path.join(repoRoot, "scripts/smoke/codebuddy-real-receipt.mjs");
const contract = JSON.parse(
  fs.readFileSync(
    path.join(repoRoot, "assets/contract/codebuddy-reference-delta.json"),
    "utf8",
  ),
);
const targetCommit = execFileSync("git", ["rev-parse", "HEAD"], {
  cwd: repoRoot,
  encoding: "utf8",
}).trim();
const serverSecret = "codebuddy-server-secret-for-harness";
const routerSecret = "codebuddy-router-secret-for-harness";

const specs = Object.freeze({
  intl: Object.freeze({
    accountEnv: "CODEBUDDY_INTL_TEST_ACCOUNT",
    providerEnvs: Object.freeze({
      claude: "CC_SWITCH_CODEBUDDY_INTL_CLAUDE_PROVIDER_ID",
      codex: "CC_SWITCH_CODEBUDDY_INTL_CODEX_PROVIDER_ID",
      gemini: "CC_SWITCH_CODEBUDDY_INTL_GEMINI_PROVIDER_ID",
    }),
    shareEnv: "CC_SWITCH_CODEBUDDY_INTL_SHARE_ID",
    modelEnv: "CC_SWITCH_CODEBUDDY_INTL_MODEL",
    receiptEnv: "CODEBUDDY_INTL_REAL_RECEIPT_FILE",
  }),
  cn: Object.freeze({
    accountEnv: "CODEBUDDY_CN_TEST_ACCOUNT",
    providerEnvs: Object.freeze({
      claude: "CC_SWITCH_CODEBUDDY_CN_CLAUDE_PROVIDER_ID",
      codex: "CC_SWITCH_CODEBUDDY_CN_CODEX_PROVIDER_ID",
      gemini: "CC_SWITCH_CODEBUDDY_CN_GEMINI_PROVIDER_ID",
    }),
    shareEnv: "CC_SWITCH_CODEBUDDY_CN_SHARE_ID",
    modelEnv: "CC_SWITCH_CODEBUDDY_CN_MODEL",
    receiptEnv: "CODEBUDDY_CN_REAL_RECEIPT_FILE",
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

function fixture(site) {
  const model = site === "intl" ? "default-model" : "default";
  const accountId = `codebuddy-${site}-fixture-account`;
  const providerIds = Object.fromEntries(
    ["claude", "codex", "gemini"].map((app) => [
      app,
      `codebuddy-${site}-${app}-provider`,
    ]),
  );
  const providers = {
    claude: { providerRevision: 17, runtimeFingerprint: `runtime-${site}-claude` },
    codex: { providerRevision: 19, runtimeFingerprint: `runtime-${site}-codex` },
    gemini: { providerRevision: 23, runtimeFingerprint: `runtime-${site}-gemini` },
  };
  const fetchedAtMs = Date.now();
  const modelData = [
    {
      id: model,
      object: "model",
      owned_by: "codebuddy",
      capabilities: { tools: true, reasoning: true },
    },
  ];
  const catalogs = Object.fromEntries(
    ["claude", "codex", "gemini"].map((app) => [
      app,
      {
        source: "codebuddy_live_model_catalog",
        fetchedAtMs,
        stale: false,
        digest: digest(`cc-switch-server:codebuddy-${app}-catalog:v2`, modelData),
      },
    ]),
  );
  return {
    site,
    model,
    accountId,
    authIdentityGeneration: 7,
    tokenRefreshGeneration: 11,
    providerIds,
    providers,
    shareId: `codebuddy-${site}-share`,
    shareRevision: 29,
    fetchedAtMs,
    modelData,
    catalogs,
  };
}

function generations(value) {
  const surfaceBindings = Object.fromEntries(
    Object.entries(value.providers).map(([app, provider]) => [
      app,
      {
        providerRevision: provider.providerRevision,
        runtimeFingerprintDigest: digest(
          `cc-switch-server:codebuddy-${app}-runtime:v2`,
          provider.runtimeFingerprint,
        ),
      },
    ]),
  );
  const accountIdentityDigest = digest("cc-switch-server:codebuddy-account:v2", {
    accountId: value.accountId,
    site: value.site,
    authIdentityGeneration: value.authIdentityGeneration,
    tokenRefreshGeneration: value.tokenRefreshGeneration,
  });
  const providerBindingDigest = digest("cc-switch-server:codebuddy-provider-bindings:v2", {
    accountIdentityDigest,
    bindings: Object.entries(value.providerIds)
      .map(([app, providerId]) => ({ app, providerId, ...surfaceBindings[app] }))
      .sort((left, right) => left.app.localeCompare(right.app)),
  });
  const shareBindingDigest = digest("cc-switch-server:codebuddy-share-binding:v2", {
    shareId: value.shareId,
    shareRevision: value.shareRevision,
    bindings: Object.entries(value.providerIds)
      .map(([app, providerId]) => ({ app, providerId, providerType: "codebuddy_oauth" }))
      .sort((left, right) => left.app.localeCompare(right.app)),
  });
  return {
    authIdentityGeneration: value.authIdentityGeneration,
    tokenRefreshGeneration: value.tokenRefreshGeneration,
    accountIdentityDigest,
    providerBindingDigest,
    surfaceBindings,
    shareRevision: value.shareRevision,
    shareBindingDigest,
  };
}

function expectedDecisions() {
  return {
    sameAccountFirst401: "replayed_once_precommit",
    second401: "terminal",
    identityGenerationDrift: "terminal",
    crossSiteFallback: "disabled",
    crossDomainFallback: "disabled",
    crossAccountFallback: "disabled",
    crossProviderFallback: "disabled",
    crossShareFallback: "disabled",
    postCommitReplay: "disabled",
    authoritativeEmptyCatalog: "empty_success",
    catalogStale: "same_identity_only",
    enterpriseRuntime: "disabled",
    multimodalRuntime: "disabled",
    deepseekV41Runtime: "disabled_without_separate_evidence",
    receiptAutoEnablesCapabilities: "false",
  };
}

function bodyHashes(value) {
  return Object.fromEntries(
    contract.realAcceptance.requiredBodyHashes.map((name) => [
      name,
      digest(`codebuddy-fixture:${value.site}`, name),
    ]),
  );
}

function receipt(value, overrides = {}) {
  const hashes = bodyHashes(value);
  const scopedGenerations = generations(value);
  const scopeDigest = digest("cc-switch-server:codebuddy-real-scope:v2", {
    site: value.site,
    targetCommit,
    harnessRevision: 2,
    model: value.model,
    generations: scopedGenerations,
    catalogs: value.catalogs,
    bodyHashes: hashes,
  });
  const measurements = Object.fromEntries(
    contract.realAcceptance.requiredMeasurements.map((name) => [name, 1]),
  );
  Object.assign(measurements, {
    catalogRuns: 3,
    claudeRuns: 2,
    codexRuns: 2,
    geminiRuns: 2,
    recoveryRuns: 2,
    terminalEofRuns: 6,
  });
  return {
    schemaVersion: 2,
    providerFamily: "codebuddy",
    site: value.site,
    verificationState: "contract_verified",
    liveState: "live_pending",
    targetCommit,
    harnessRevision: 2,
    recordedAt: new Date().toISOString(),
    scopeDigest,
    model: value.model,
    catalogs: value.catalogs,
    checks: Object.fromEntries(
      contract.realAcceptance.requiredChecks.map((name) => [name, "pass"]),
    ),
    bodyHashes: hashes,
    measurements,
    generations: scopedGenerations,
    decisions: expectedDecisions(),
    decoyRequests: {
      otherAccount: 0,
      otherProvider: 0,
      otherShare: 0,
      otherSite: 0,
      otherDomain: 0,
      poolRouter: 0,
      enterprise: 0,
      multimodal: 0,
    },
    sensitiveScan: { status: "pass", matches: 0, scannedBytes: 4096 },
    ...overrides,
  };
}

function clearedEnv() {
  const cleared = {};
  for (const spec of Object.values(specs)) {
    cleared[spec.accountEnv] = "";
    cleared[spec.shareEnv] = "";
    cleared[spec.modelEnv] = "";
    cleared[spec.receiptEnv] = "";
    for (const name of Object.values(spec.providerEnvs)) cleared[name] = "";
  }
  return cleared;
}

function runScript(value, overrides = {}) {
  const spec = specs[value.site];
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [script, "--site", value.site], {
      cwd: repoRoot,
      env: {
        ...process.env,
        ...clearedEnv(),
        RUN_REAL: "1",
        CC_SWITCH_CODEBUDDY_HARNESS_MODE: "fixture",
        ROUTER_API_TOKEN_HEADER: "Authorization",
        CC_SWITCH_REAL_TIMEOUT_MS: "5000",
        [spec.accountEnv]: value.accountId,
        [spec.shareEnv]: value.shareId,
        [spec.modelEnv]: value.model,
        ...Object.fromEntries(
          Object.entries(spec.providerEnvs).map(([app, name]) => [name, value.providerIds[app]]),
        ),
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

function providerView(value, app, { bindingMismatch = false } = {}) {
  return {
    app,
    providerRevision: value.providers[app].providerRevision,
    providerType: "codebuddy_oauth",
    providerTypeId: "codebuddy_oauth",
    provider: { id: value.providerIds[app] },
    runtime: {
      driverId: "special.codebuddy_oauth",
      runtimeFingerprint: value.providers[app].runtimeFingerprint,
      configurationState: "ready",
      authRef: {
        kind: "managed_account",
        accountId: bindingMismatch ? "codebuddy-decoy-account" : value.accountId,
        expectedProviderType: "codebuddy_oauth",
        authIdentityGeneration: value.authIdentityGeneration,
      },
    },
  };
}

async function startMockServer(value, { bindingMismatch = false, leakSecret = false } = {}) {
  const seen = [];
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    seen.push({
      method: request.method,
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
            providerType: "codebuddy_oauth",
            authIdentityGeneration: value.authIdentityGeneration,
            tokenRefreshGeneration: value.tokenRefreshGeneration,
            hasAccessToken: true,
            hasRefreshToken: true,
            hasApiKey: false,
            hasProfile: true,
            hasRaw: true,
            needsRelogin: false,
          },
          {
            id: "codebuddy-decoy-account",
            providerType: "codebuddy_oauth",
            authIdentityGeneration: 99,
            tokenRefreshGeneration: 99,
          },
        ],
        ...(leakSecret ? { leak: serverSecret } : {}),
      });
      return;
    }
    if (request.method === "GET" && url.pathname === "/api/providers") {
      sendJson(response, 200, {
        ok: true,
        providers: [
          providerView(value, "claude", { bindingMismatch }),
          providerView(value, "codex", { bindingMismatch }),
          providerView(value, "gemini", { bindingMismatch }),
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
            enabled: true,
            status: "active",
            configRevision: value.shareRevision,
            app: "claude",
            providerId: value.providerIds.claude,
            providerType: "codebuddy_oauth",
            bindings: [
              {
                app: "codex",
                providerId: value.providerIds.codex,
                providerType: "codebuddy_oauth",
              },
              {
                app: "gemini",
                providerId: value.providerIds.gemini,
                providerType: "codebuddy_oauth",
              },
            ],
          },
        ],
      });
      return;
    }
    if (request.method === "GET" && url.pathname === "/v1/models") {
      const app = url.searchParams.get("app");
      const providerId = url.searchParams.get("providerId");
      if (!value.providerIds[app] || value.providerIds[app] !== providerId) {
        sendJson(response, 404, { error: "unknown binding" });
        return;
      }
      sendJson(response, 200, {
        object: "list",
        data: value.modelData,
        source: "codebuddy_live_model_catalog",
        stale: false,
        fetchedAtMs: value.fetchedAtMs,
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

async function withHarness(site, options, callback) {
  const value = fixture(site);
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), `codebuddy-${site}-receipt-`));
  const receiptFile = path.join(directory, "receipt.json");
  const server = await startMockServer(value, options?.server || {});
  try {
    fs.writeFileSync(
      receiptFile,
      `${JSON.stringify(receipt(value, options?.receipt || {}), null, 2)}\n`,
      { mode: 0o600 },
    );
    const spec = specs[site];
    const result = await runScript(value, {
      SERVER_URL: server.origin,
      CC_SWITCH_SERVER_TOKEN: serverSecret,
      CC_SWITCH_SHARE_URL: server.origin,
      ROUTER_API_TOKEN: routerSecret,
      [spec.receiptEnv]: receiptFile,
      ...(options?.env || {}),
    });
    await callback({ result, server, value, receiptFile });
  } finally {
    await server.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
}

test("CodeBuddy receipt gate reports blocked inputs without network access", async () => {
  const value = fixture("intl");
  const result = await runScript(value, {
    SERVER_URL: "",
    CC_SWITCH_SERVER_TOKEN: "",
    CC_SWITCH_SHARE_URL: "",
    ROUTER_API_TOKEN: "",
  });
  assert.equal(result.code, 0, result.stderr);
  const output = JSON.parse(result.stdout);
  assert.equal(output.site, "intl");
  assert.equal(output.verificationState, "blocked_inputs");
  assert.equal(output.liveState, "live_pending");
  assert.ok(output.missingInputs.includes("SERVER_URL"));
});

test("CodeBuddy Intl and CN fixture receipts stay contract-only", async () => {
  for (const site of ["intl", "cn"]) {
    await withHarness(site, {}, async ({ result, server }) => {
      assert.equal(result.code, 0, result.stderr);
      assert.match(result.stdout, /verificationState=contract_verified/);
      assert.match(result.stdout, /liveState=live_pending/);
      assert.doesNotMatch(result.stdout, /live_verified/);
      assert.equal(server.seen.length, 6);
      assert.equal(
        server.seen.filter((entry) => entry.path === "/v1/models").length,
        3,
      );
      assert.ok(
        server.seen
          .filter((entry) => entry.path.startsWith("/api/"))
          .every((entry) => entry.authorization === `Bearer ${serverSecret}`),
      );
      assert.ok(
        server.seen
          .filter((entry) => entry.path === "/v1/models")
          .every((entry) => entry.authorization === `Bearer ${routerSecret}`),
      );
    });
  }
});

test("CodeBuddy fixture mode rejects a receipt that claims live verification", async () => {
  await withHarness(
    "intl",
    { receipt: { verificationState: "live_verified", liveState: "live_verified" } },
    async ({ result }) => {
      assert.equal(result.code, 1);
      assert.equal(result.stdout, "");
      assert.match(result.stderr, /details redacted/);
      assert.doesNotMatch(result.stderr, /live_verified/);
    },
  );
});

test("CodeBuddy receipt scope binds all six body hashes", async () => {
  const value = fixture("cn");
  const tampered = bodyHashes(value);
  tampered.gemini_stream = "f".repeat(64);
  await withHarness(
    "cn",
    { receipt: { bodyHashes: tampered } },
    async ({ result }) => {
      assert.equal(result.code, 1);
      assert.match(result.stderr, /details redacted/);
    },
  );
});

test("CodeBuddy Provider binding mismatch fails before receipt acceptance", async () => {
  await withHarness(
    "intl",
    { server: { bindingMismatch: true } },
    async ({ result, server }) => {
      assert.equal(result.code, 1);
      assert.match(result.stderr, /details redacted/);
      assert.equal(server.seen.filter((entry) => entry.path === "/v1/models").length, 0);
    },
  );
});

test("CodeBuddy validator redacts secret-bearing control-plane failures", async () => {
  await withHarness(
    "cn",
    { server: { leakSecret: true } },
    async ({ result }) => {
      assert.equal(result.code, 1);
      assert.match(result.stderr, /details redacted/);
      assert.doesNotMatch(result.stderr, new RegExp(serverSecret));
      assert.doesNotMatch(result.stdout, new RegExp(serverSecret));
    },
  );
});
