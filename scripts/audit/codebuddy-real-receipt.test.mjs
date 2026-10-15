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

function catalogDigest(app, data) {
  return digest(
    `cc-switch-server:codebuddy-${app}-catalog:v2`,
    [...data].sort((left, right) => left.id < right.id ? -1 : left.id > right.id ? 1 : 0),
  );
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
        digest: catalogDigest(app, modelData),
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

function providerView(value, app, { bindingMismatch = false, scopeFault = "" } = {}) {
  const fault = scopeFault.startsWith(`${app}_`) ? scopeFault.slice(app.length + 1) : "";
  const revision = value.providers[app].providerRevision;
  return {
    app,
    revision: ["missing_view_revision", "legacy_top_level_only"].includes(fault)
      ? undefined : revision + (fault === "mismatched_revisions" ? 1 : 0),
    ...(fault === "legacy_top_level_only" ? { providerRevision: revision } : {}),
    providerType: "codebuddy_oauth",
    providerTypeId: "codebuddy_oauth",
    provider: { id: value.providerIds[app] },
    runtime: {
      providerRevision: ["missing_runtime_revision", "legacy_top_level_only"].includes(fault)
        ? undefined : revision,
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

async function startMockServer(value, { bindingMismatch = false, duplicateShareApp = "", primaryMismatch = false, shareFault = "", scopeFault = "", accountFault = null, leakSecret = false, drift = "", oversizedControl = "", onFirstControl = null, accountSite = value.site, omitAccountScope = false, finalShareDelayMs = 0, finalCatalogFault = null } = {}) {
  const seen = [];
  let firstControlSeen = false;
  let accountReads = 0;
  let providerReads = 0;
  let shareReads = 0;
  const catalogReads = { claude: 0, codex: 0, gemini: 0 };
  const accountFlags = { hasApiKey: false, needsRelogin: false };
  if (accountFault) {
    if (accountFault.variant === "missing") {
      delete accountFlags[accountFault.field];
    } else {
      accountFlags[accountFault.field] =
        accountFault.variant === "null" ? null : "false";
    }
  }
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    seen.push({
      method: request.method,
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
            providerType: "codebuddy_oauth",
            authIdentityGeneration: value.authIdentityGeneration,
            tokenRefreshGeneration: value.tokenRefreshGeneration + (drift === "account" && accountReads > 1 ? 1 : 0),
            hasAccessToken: true,
            hasRefreshToken: true,
            ...accountFlags,
            hasProfile: true,
            hasRaw: true,
            ...(!omitAccountScope && {
              codebuddyCredentialScope: {
                site: drift === "site" && accountReads > 1
                  ? (value.site === "intl" ? "cn" : "intl")
                  : accountSite,
              },
            }),
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
      providerReads += 1;
      if (oversizedControl === "chunked") {
        response.writeHead(200, { "content-type": "application/json" });
        response.write("x".repeat(4 * 1024 * 1024 + 1));
        return;
      }
      const providers = ["claude", "codex", "gemini"].map((app) =>
        providerView(value, app, { bindingMismatch, scopeFault }),
      );
      if (drift === "provider" && providerReads > 1) {
        providers[0].revision += 1;
        providers[0].runtime.providerRevision += 1;
      }
      sendJson(response, 200, {
        ok: true,
        providers,
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
            enabled: shareFault === "disabled" ? false : shareFault === "missing_enabled" ? undefined : true,
            status: shareFault === "paused" ? "paused" : shareFault === "missing_status" ? undefined : "active",
            configRevision: scopeFault === "missing_share_revision" ? undefined
              : value.shareRevision + (drift === "share" && shareReads > 1 ? 1 : 0),
            app: "claude",
            providerId: primaryMismatch ? "conflicting-primary-provider" : value.providerIds.claude,
            providerType: "codebuddy_oauth",
            bindings: [
              {
                app: "claude",
                providerId: value.providerIds.claude,
                providerType: "codebuddy_oauth",
              },
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
              ...(duplicateShareApp ? [{
                app: duplicateShareApp,
                providerId: "conflicting-codebuddy-provider",
                providerType: "codebuddy_oauth",
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
      const app = url.searchParams.get("app");
      const providerId = url.searchParams.get("providerId");
      if (!value.providerIds[app] || value.providerIds[app] !== providerId) {
        sendJson(response, 404, { error: "unknown binding" });
        return;
      }
      catalogReads[app] += 1;
      const modelData = value.modelDataByApp?.[app] ?? value.modelData;
      const fault = finalCatalogFault?.app === app && catalogReads[app] > 1
        ? finalCatalogFault.kind : "";
      let responseData = fault === "remove" ? [] : modelData;
      if (fault === "add") {
        responseData = [...modelData, { id: "unexpected-model", object: "model" }];
      }
      if (fault === "reorder") responseData = [...modelData].reverse();
      if (fault === "capability") {
        responseData = modelData.map((entry) => entry.id === value.model
          ? { ...entry, capabilities: { ...entry.capabilities, tools: false } }
          : entry);
      }
      sendJson(response, 200, {
        object: "list",
        data: responseData,
        source: fault === "source" ? "untrusted_catalog" : "codebuddy_live_model_catalog",
        stale: fault === "stale",
        fetchedAtMs: fault === "future" ? Date.now() + 2 * 60_000 : value.fetchedAtMs,
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
  options?.mutateFixture?.(value);
  if (options?.catalogOffsetMs !== undefined) {
    value.fetchedAtMs = Date.now() + options.catalogOffsetMs;
    for (const snapshot of Object.values(value.catalogs)) {
      snapshot.fetchedAtMs = value.fetchedAtMs;
    }
  }
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), `codebuddy-${site}-receipt-`));
  const receiptFile = path.join(directory, "receipt.json");
  const server = await startMockServer(value, {
    ...(options?.server || {}),
    ...(options?.server?.onFirstControl && {
      onFirstControl: () => options.server.onFirstControl(receiptFile),
    }),
  });
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

test("CodeBuddy Intl and CN fixture receipts stay contract-only", async (t) => {
  for (const site of ["intl", "cn"]) {
    await t.test(site, async () => withHarness(site, {}, async ({ result, server }) => {
      assert.equal(result.code, 0, result.stderr);
      assert.match(result.stdout, /verificationState=contract_verified/);
      assert.match(result.stdout, /liveState=live_pending/);
      assert.doesNotMatch(result.stdout, /live_verified/);
      assert.equal(server.seen.length, 12);
      for (const controlPath of ["/api/accounts", "/api/providers", "/api/shares"]) {
        assert.equal(server.seen.filter((entry) => entry.path === controlPath).length, 2);
      }
      const catalogRequests = server.seen.filter((entry) => entry.path === "/v1/models");
      assert.equal(catalogRequests.length, 6);
      const selected = fixture(site);
      for (const app of ["claude", "codex", "gemini"]) {
        const scoped = catalogRequests.filter(
          (entry) => new URLSearchParams(entry.search).get("app") === app,
        );
        assert.equal(scoped.length, 2, `${site}/${app} catalog count`);
        const query = new URLSearchParams(scoped[0].search);
        assert.equal(query.get("providerId"), selected.providerIds[app]);
        assert.equal(query.get("expectedShareId"), selected.shareId);
        assert.equal(query.get("expectedProviderId"), selected.providerIds[app]);
      }
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
    }));
  }
});

test("CodeBuddy sites reject final catalog changes before private receipt acceptance", async (t) => {
  for (const site of ["intl", "cn"]) {
    for (const app of ["claude", "codex", "gemini"]) {
      for (const kind of ["remove", "add", "capability", "stale", "source", "future"]) {
        await t.test(`${site}/${app}/${kind}`, async () => {
          await withHarness(site, {
            server: { finalCatalogFault: { app, kind } },
          }, async ({ result, server, receiptFile }) => {
            assert.equal(result.code, 1, `${result.stdout}\n${result.stderr}`);
            assert.equal(result.stderr.trim(), "[FAIL] CodeBuddy acceptance failed (details redacted)");
            assert.ok(fs.existsSync(receiptFile));
            assert.equal(server.seen.filter((entry) => entry.path === "/api/shares").length, 2);
          });
        });
      }
    }
  }
});

test("CodeBuddy sites reject invalid IDs in each full App catalog", async (t) => {
  for (const site of ["intl", "cn"]) {
    for (const app of ["claude", "codex", "gemini"]) {
      for (const kind of ["missing", "blank", "duplicate", "nonstring"]) {
        await t.test(`${site}/${app}/${kind}`, async () => withHarness(site, {
          mutateFixture(value) {
            const modelData = [
              ...value.modelData,
              { id: "codebuddy-other", object: "model", owned_by: "codebuddy" },
              kind === "missing" ? { object: "model", owned_by: "codebuddy" }
                : { id: kind === "blank" ? " " : kind === "duplicate" ? "codebuddy-other" : 42, object: "model", owned_by: "codebuddy" },
            ];
            value.modelDataByApp = { [app]: modelData };
            value.catalogs[app].digest = digest(`cc-switch-server:codebuddy-${app}-catalog:v2`, modelData);
          },
        }, async ({ result }) => {
          assert.equal(result.code, 1, `${result.stdout}\n${result.stderr}`);
          assert.equal(result.stderr.trim(), "[FAIL] CodeBuddy acceptance failed (details redacted)");
        }));
      }
    }
  }
});

test("CodeBuddy sites accept catalog row reordering without content drift", async (t) => {
  for (const site of ["intl", "cn"]) {
    for (const app of ["claude", "codex", "gemini"]) {
      await t.test(`${site}/${app}`, async () => withHarness(site, {
        mutateFixture(value) {
          const modelData = [
            ...value.modelData,
            { id: "codebuddy-other", object: "model", owned_by: "codebuddy" },
          ];
          value.modelDataByApp = { [app]: modelData };
          value.catalogs[app].digest = catalogDigest(app, modelData);
        },
        server: { finalCatalogFault: { app, kind: "reorder" } },
      }, async ({ result }) => {
        assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
        assert.match(result.stdout, /contract_verified.*live_pending/);
      }));
    }
  }
});

test("CodeBuddy private receipts cannot be hard-linked from inside the repository", async (t) => {
  for (const site of ["intl", "cn"]) {
    await t.test(site, async () => {
      const value = fixture(site);
      const insideDirectory = fs.mkdtempSync(path.join(repoRoot, ".codebuddy-receipt-hardlink-"));
      const outsideDirectory = fs.mkdtempSync(path.join(path.dirname(repoRoot), ".codebuddy-receipt-outside-"));
      const insideReceipt = path.join(insideDirectory, "receipt.json");
      const outsideReceipt = path.join(outsideDirectory, "receipt.json");
      fs.writeFileSync(insideReceipt, `${JSON.stringify(receipt(value), null, 2)}\n`, { mode: 0o600 });
      fs.linkSync(insideReceipt, outsideReceipt);
      const server = await startMockServer(value);
      try {
        const result = await runScript(value, {
          SERVER_URL: server.origin,
          CC_SWITCH_SERVER_TOKEN: serverSecret,
          CC_SWITCH_SHARE_URL: server.origin,
          ROUTER_API_TOKEN: routerSecret,
          [specs[site].receiptEnv]: outsideReceipt,
        });
        assert.equal(result.code, 1, result.stdout);
        assert.equal(result.stderr.trim(), "[FAIL] CodeBuddy acceptance failed (details redacted)");
        assert.deepEqual(server.seen, []);
      } finally {
        await server.close();
        fs.rmSync(outsideDirectory, { recursive: true, force: true });
        fs.rmSync(insideDirectory, { recursive: true, force: true });
      }
    });
  }
});

test("CodeBuddy private receipt cannot be replaced during control-plane validation", async (t) => {
  for (const site of ["intl", "cn"]) {
    await t.test(site, async () => {
      const value = fixture(site);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `codebuddy-receipt-swap-${site}-`));
      const receiptFile = path.join(directory, "receipt.json");
      const replacementFile = path.join(directory, "replacement.json");
      fs.writeFileSync(receiptFile, `${JSON.stringify(receipt(value, { model: "not-the-selected-model" }), null, 2)}\n`, { mode: 0o600 });
      fs.writeFileSync(replacementFile, `${JSON.stringify(receipt(value), null, 2)}\n`, { mode: 0o600 });
      const originalInode = fs.statSync(receiptFile).ino;
      const server = await startMockServer(value, {
        onFirstControl: () => fs.renameSync(replacementFile, receiptFile),
      });
      try {
        const result = await runScript(value, {
          SERVER_URL: server.origin,
          CC_SWITCH_SERVER_TOKEN: serverSecret,
          CC_SWITCH_SHARE_URL: server.origin,
          ROUTER_API_TOKEN: routerSecret,
          [specs[site].receiptEnv]: receiptFile,
        });
        assert.equal(result.code, 1, result.stdout);
        assert.equal(result.stderr.trim(), "[FAIL] CodeBuddy acceptance failed (details redacted)");
        assert.ok(server.seen.some((entry) => entry.path === "/api/accounts"));
        assert.notEqual(fs.statSync(receiptFile).ino, originalInode);
      } finally {
        await server.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("CodeBuddy private receipt rejects an in-place rewrite during its snapshot", async (t) => {
  for (const site of ["intl", "cn"]) {
    await t.test(site, async () => {
      const receiptEnv = specs[site].receiptEnv;
      const preload = [
        'import fs from "node:fs";',
        'const originalRead = fs.readSync;',
        `const target = process.env[${JSON.stringify(receiptEnv)}];`,
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
      await withHarness(site, {
        env: { NODE_OPTIONS: `--import=data:text/javascript,${encodeURIComponent(preload)}` },
      }, async ({ result, server, receiptFile }) => {
        assert.equal(result.code, 1, result.stdout);
        assert.equal(result.stderr.trim(), "[FAIL] CodeBuddy acceptance failed (details redacted)");
        assert.deepEqual(server.seen, []);
        assert.match(fs.readFileSync(receiptFile, "utf8"), /^ +$/);
      });
    });
  }
});

test("CodeBuddy private receipt must remain the same file through final acceptance", async (t) => {
  for (const site of ["intl", "cn"]) {
    for (const change of ["replacement", "in_place"]) {
      await t.test(`${site}/${change}`, async () => {
        const value = fixture(site);
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `codebuddy-receipt-final-${site}-`));
        const receiptFile = path.join(directory, "receipt.json");
        const replacementFile = path.join(directory, "replacement.json");
        fs.writeFileSync(receiptFile, `${JSON.stringify(receipt(value), null, 2)}\n`, { mode: 0o600 });
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
          const result = await runScript(value, {
            SERVER_URL: server.origin,
            CC_SWITCH_SERVER_TOKEN: serverSecret,
            CC_SWITCH_SHARE_URL: server.origin,
            ROUTER_API_TOKEN: routerSecret,
            [specs[site].receiptEnv]: receiptFile,
          });
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] CodeBuddy acceptance failed (details redacted)");
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
});

test("CodeBuddy final acceptance checks bytes even when file metadata appears unchanged", async (t) => {
  for (const site of ["intl", "cn"]) {
    await t.test(site, async () => {
      const receiptEnv = specs[site].receiptEnv;
      const preload = [
        'import fs from "node:fs";',
        `const target = process.env[${JSON.stringify(receiptEnv)}];`,
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
      await withHarness(site, {
        env: { NODE_OPTIONS: `--import=data:text/javascript,${encodeURIComponent(preload)}` },
        server: {
          onFirstControl: (target) => {
            fs.writeFileSync(target, " ".repeat(fs.statSync(target).size), { flag: "r+" });
          },
        },
      }, async ({ result, server, receiptFile }) => {
        assert.equal(result.code, 1, result.stdout);
        assert.equal(result.stderr.trim(), "[FAIL] CodeBuddy acceptance failed (details redacted)");
        assert.ok(server.seen.some((entry) => entry.path === "/api/accounts"));
        assert.match(fs.readFileSync(receiptFile, "utf8"), /^ +$/);
      });
    });
  }
});

test("CodeBuddy receipt gate requires a catalog within the production fresh TTL", async (t) => {
  for (const site of ["intl", "cn"]) {
    await t.test(`${site}/cached-seven-minutes`, async () => {
      await withHarness(site, { catalogOffsetMs: -7 * 60_000 }, async ({ result }) => {
        assert.equal(result.code, 0, result.stderr);
      });
    });
    for (const [label, catalogOffsetMs] of [
      ["expired-nine-minutes", -9 * 60_000],
      ["future-two-minutes", 2 * 60_000],
    ]) {
      await t.test(`${site}/${label}`, async () => {
        await withHarness(site, { catalogOffsetMs }, async ({ result }) => {
          assert.equal(result.code, 1, result.stdout);
          assert.match(result.stderr, /details redacted/);
        });
      });
    }
  }
});

test("CodeBuddy Intl and CN reject receipts that expire during final binding validation", async (t) => {
  for (const site of ["intl", "cn"]) {
    await t.test(site, async () => {
      await withHarness(site, {
        receipt: { recordedAt: new Date(Date.now() - 24 * 60 * 60 * 1_000 + 1_200).toISOString() },
        server: { finalShareDelayMs: 2_000 },
      }, async ({ result, server }) => {
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /details redacted/);
        assert.equal(server.seen.filter((entry) => entry.path === "/api/shares").length, 2);
      });
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

test("CodeBuddy Intl and CN reject ambiguous Share bindings for every App", async (t) => {
  for (const site of ["intl", "cn"]) {
    for (const app of ["claude", "codex", "gemini"]) {
      await t.test(`${site}/${app}`, async () => {
        await withHarness(site, { server: { duplicateShareApp: app } }, async ({ result, server }) => {
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] CodeBuddy acceptance failed (details redacted)");
          assert.equal(server.seen.filter((entry) => entry.path === "/v1/models").length, 0);
        });
      });
    }
  }
});

test("CodeBuddy Intl and CN reject Share primary fields that disagree with bindings", async (t) => {
  for (const site of ["intl", "cn"]) {
    await t.test(site, async () => {
      await withHarness(site, { server: { primaryMismatch: true } }, async ({ result, server }) => {
        assert.equal(result.code, 1, result.stdout);
        assert.equal(result.stderr.trim(), "[FAIL] CodeBuddy acceptance failed (details redacted)");
        assert.equal(server.seen.filter((entry) => entry.path === "/v1/models").length, 0);
      });
    });
  }
});

test("CodeBuddy Intl and CN require explicitly active Shares before catalog access", async (t) => {
  for (const site of ["intl", "cn"]) {
    for (const shareFault of ["missing_enabled", "missing_status", "disabled", "paused"]) {
      await t.test(`${site}/${shareFault}`, async () => {
        await withHarness(site, { server: { shareFault } }, async ({ result, server }) => {
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] CodeBuddy acceptance failed (details redacted)");
          assert.equal(server.seen.filter((entry) => entry.path === "/v1/models").length, 0);
        });
      });
    }
  }
});

test("CodeBuddy sites reject incomplete or inconsistent three-Surface revision scope before catalogs", async (t) => {
  const providerFaults = [
    "missing_view_revision",
    "missing_runtime_revision",
    "mismatched_revisions",
    "legacy_top_level_only",
    "blank_runtime_fingerprint",
  ];
  for (const site of ["intl", "cn"]) {
    for (const scopeFault of [
      ...["claude", "codex", "gemini"].flatMap((app) => providerFaults.map((fault) => `${app}_${fault}`)),
      "missing_share_revision",
    ]) {
      await t.test(`${site}/${scopeFault}`, async () => {
        await withHarness(site, {
          server: { scopeFault },
          mutateFixture: (value) => {
            for (const provider of Object.values(value.providers)) provider.providerRevision = 0;
            if (scopeFault === "missing_share_revision") value.shareRevision = 0;
            for (const app of ["claude", "codex", "gemini"]) {
              if (scopeFault === `${app}_blank_runtime_fingerprint`) {
                value.providers[app].runtimeFingerprint = "   ";
              }
            }
          },
        }, async ({ result, server }) => {
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] CodeBuddy acceptance failed (details redacted)");
          assert.equal(server.seen.filter((entry) => entry.path === "/v1/models").length, 0);
        });
      });
    }
  }
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

test("CodeBuddy receipt gate rejects identity and binding drift before acceptance", async (t) => {
  for (const site of ["intl", "cn"]) {
    for (const drift of ["account", "provider", "share", "site"]) {
      await t.test(`${site}/${drift}`, async () => {
        await withHarness(site, { server: { drift } }, async ({ result }) => {
          assert.equal(result.code, 1, result.stdout);
          assert.match(result.stderr, /details redacted/);
        });
      });
    }
  }
});

test("CodeBuddy receipt rejects wrong or absent Account profile site before catalog requests", async (t) => {
  for (const site of ["intl", "cn"]) {
    for (const [label, serverOptions] of [
      ["wrong_site", { accountSite: site === "intl" ? "cn" : "intl" }],
      ["missing_scope", { omitAccountScope: true }],
    ]) {
      await t.test(`${site}/${label}`, async () => {
        await withHarness(site, { server: serverOptions }, async ({ result, server }) => {
          assert.equal(result.code, 1, result.stdout);
          assert.match(result.stderr, /details redacted/);
          assert.equal(server.seen.filter((entry) => entry.path === "/v1/models").length, 0);
        });
      });
    }
  }
});

test("CodeBuddy Intl and CN require explicit false Account booleans", async (t) => {
  for (const site of ["intl", "cn"]) {
    for (const field of ["hasApiKey", "needsRelogin"]) {
      for (const variant of ["missing", "null", "string"]) {
        await t.test(`${site}/${field}/${variant}`, async () => {
          await withHarness(site, {
            server: { accountFault: { field, variant } },
          }, async ({ result, server }) => {
            assert.equal(result.code, 1, result.stdout);
            assert.equal(result.stderr.trim(), "[FAIL] CodeBuddy acceptance failed (details redacted)");
            assert.equal(server.seen.filter((entry) => entry.path === "/api/providers").length, 0);
            assert.equal(server.seen.filter((entry) => entry.path === "/v1/models").length, 0);
          });
        });
      }
    }
  }
});

test("CodeBuddy control-plane response bound rejects unfinished oversized bodies on both sites", async (t) => {
  for (const [site, oversizedControl, expectedPaths] of [
    ["intl", "declared", ["/api/accounts"]],
    ["cn", "chunked", ["/api/accounts", "/api/providers"]],
  ]) {
    await t.test(`${site}/${oversizedControl}`, async () => {
      const startedAt = Date.now();
      await withHarness(site, { server: { oversizedControl } }, async ({ result, server }) => {
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /details redacted/);
        assert.deepEqual(server.seen.map((entry) => entry.path), expectedPaths);
      });
      assert.ok(Date.now() - startedAt < 3_000, "oversized response waited for timeout");
    });
  }
});
