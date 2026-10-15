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
const fixtureCatalogAtMs = Date.now();

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

function catalogs(rail, catalogAtByApp = {}, catalogDataByApp = {}) {
  const value = values(rail);
  return Object.entries(value.providerIds).map(([app]) => ({
    app,
    model: value.models[app],
    source: "authenticated_fetch_available_models",
    stale: false,
    fetchedAtMs: catalogAtByApp[app] ?? fixtureCatalogAtMs,
    digest: digest(`cc-switch-server:antigravity-${app}-catalog:v2`, catalogDataByApp[app] ?? [
      { id: value.models[app], object: "model", owned_by: "antigravity" },
    ]),
  }));
}

function scopeDigest(rail, catalogAtByApp = {}, scopedBindings = bindings(rail), shareRevision = values(rail).shareRevision, catalogDataByApp = {}) {
  const value = values(rail);
  return digest("cc-switch-server:antigravity-real-scope:v2", {
    rail,
    accountId: value.accountId,
    authIdentityGeneration: value.authIdentityGeneration,
    tokenRefreshGeneration: value.tokenRefreshGeneration,
    providerIds: value.providerIds,
    shareId: value.shareId,
    models: value.models,
    bindings: scopedBindings,
    shareRevision,
    catalogs: catalogs(rail, catalogAtByApp, catalogDataByApp),
    targetCommit,
    harnessRevision: 2,
  });
}

function receipt(rail, overrides = {}) {
  const value = values(rail);
  return {
    schemaVersion: 2,
    providerFamily: "antigravity",
    rail,
    verificationState: "contract_verified",
    liveState: "live_pending",
    targetCommit,
    harnessRevision: 2,
    recordedAt: new Date().toISOString(),
    scopeDigest: scopeDigest(rail),
    models: value.models,
    catalogs: catalogs(rail),
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

async function startMockServer(rail, { providerRail = rail, failProvidersWith = "", catalogAgeMs = 0, catalogAtByApp = {}, catalogDataByApp = {}, catalogChanged = false, finalCatalogFault = null, duplicateShareApp = "", primaryMismatch = false, shareState = "active", scopeFault = "", accountFault = null, drift = "", oversizedControl = "", onFirstControl = null, hasRefreshToken = true, finalShareDelayMs = 0 } = {}) {
  const selected = specs[providerRail];
  const value = values(rail);
  const seen = [];
  let firstControlSeen = false;
  let accountReads = 0;
  let providerReads = 0;
  let shareReads = 0;
  const catalogReads = { claude: 0, gemini: 0 };
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
            email: `${rail}@example.test`,
            providerType: selected.providerType,
            authIdentityGeneration: value.authIdentityGeneration,
            tokenRefreshGeneration: value.tokenRefreshGeneration + (drift === "account" && accountReads > 1 ? 1 : 0),
            hasAccessToken: true,
            hasRefreshToken: hasRefreshToken && !(drift === "refresh_credential" && accountReads > 1),
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
        providers: Object.entries(value.providerIds).map(([app, providerId], index) => {
          const revision = index + 3 + (drift === "provider" && providerReads > 1 && app === "claude" ? 1 : 0);
          const missingBoth = scopeFault === `missing_${app}_revision`;
          const legacyOnly = scopeFault === `legacy_${app}_top_level_only`;
          const legacyCompatibleFault = /^legacy_(claude|gemini)_top_level_only$|^missing_(claude|gemini)_(view|runtime)_revision$|^mismatched_(claude|gemini)_revisions$/.test(scopeFault);
          return {
            app,
            providerType: selected.providerType,
            providerTypeId: selected.providerType,
            revision: missingBoth || legacyOnly || scopeFault === `missing_${app}_view_revision`
              ? undefined : scopeFault === `negative_${app}_revision` ? -1
                : revision + (scopeFault === `mismatched_${app}_revisions` ? 1 : 0),
            ...(legacyCompatibleFault ? { providerRevision: revision } : {}),
            provider: { id: providerId },
            runtime: {
              providerRevision: missingBoth || legacyOnly || scopeFault === `missing_${app}_runtime_revision`
                ? undefined : scopeFault === `negative_${app}_revision` ? -1 : revision,
              driverId: selected.driverId,
              configurationState: "ready",
              runtimeFingerprint: scopeFault === `empty_${app}_fingerprint` ? ""
                : scopeFault === `blank_${app}_fingerprint` ? "   " : `${rail}-${app}-runtime`,
              authRef: {
                kind: "managed_account",
                accountId: value.accountId,
                expectedProviderType: selected.providerType,
                authIdentityGeneration: value.authIdentityGeneration,
              },
            },
          };
        }),
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
            providerId: primaryMismatch ? "conflicting-primary-provider" : value.providerIds.claude,
            providerType: selected.providerType,
            enabled: shareState !== "disabled",
            status: shareState === "paused" ? "paused" : "active",
            configRevision: scopeFault === "missing_share_revision" ? undefined
              : scopeFault === "negative_share_revision" ? -1
                : value.shareRevision + (drift === "share" && shareReads > 1 ? 1 : 0),
            bindings: [
              {
                app: "claude",
                providerId: value.providerIds.claude,
                providerType: selected.providerType,
              },
              {
                app: "gemini",
                providerId: value.providerIds.gemini,
                providerType: selected.providerType,
              },
              ...(duplicateShareApp ? [{
                app: duplicateShareApp,
                providerId: "conflicting-antigravity-provider",
                providerType: selected.providerType,
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
      assert.ok(app === "claude" || app === "gemini");
      assert.equal(url.searchParams.get("providerId"), value.providerIds[app]);
      catalogReads[app] += 1;
      const catalogData = catalogDataByApp[app] ?? [
        { id: value.models[app], object: "model", owned_by: "antigravity" },
      ];
      const fault = finalCatalogFault?.app === app && catalogReads[app] > 1
        ? finalCatalogFault.kind : "";
      let responseData = fault === "remove" ? [] : catalogData;
      if (fault === "add" || (catalogChanged && app === "claude")) {
        responseData = [...responseData, { id: "newly-visible-model", object: "model", owned_by: "antigravity" }];
      }
      if (fault === "reorder") responseData = [...catalogData].reverse();
      if (fault === "capability") {
        responseData = catalogData.map((entry) => entry.id === value.models[app]
          ? { ...entry, capabilities: { tools: false } }
          : entry);
      }
      sendJson(response, 200, {
        object: "list",
        source: fault === "source" ? "untrusted_catalog" : "authenticated_fetch_available_models",
        stale: fault === "stale",
        fetchedAtMs: fault === "future" ? Date.now() + 10 * 60_000
          : catalogAtByApp[app] ?? fixtureCatalogAtMs - catalogAgeMs,
        data: responseData,
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
        const catalogRequests = mock.seen.filter((entry) => entry.path === "/v1/models");
        assert.equal(catalogRequests.length, 4);
        const selected = values(rail);
        for (const app of ["claude", "gemini"]) {
          const scoped = catalogRequests.filter(
            (entry) => new URLSearchParams(entry.search).get("app") === app,
          );
          assert.equal(scoped.length, 2, `${rail}/${app} catalog count`);
          const query = new URLSearchParams(scoped[0].search);
          assert.equal(query.get("providerId"), selected.providerIds[app]);
          assert.equal(query.get("expectedShareId"), selected.shareId);
          assert.equal(query.get("expectedProviderId"), selected.providerIds[app]);
        }
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Antigravity and AGY reject final catalog changes before private receipt acceptance", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const app of ["claude", "gemini"]) {
      for (const kind of ["remove", "add", "capability", "stale", "source", "future"]) {
        await t.test(`${rail}/${app}/${kind}`, async () => {
          const directory = fs.mkdtempSync(path.join(os.tmpdir(), `${rail}-final-catalog-`));
          const receiptFile = path.join(directory, "receipt.json");
          writeReceipt(receiptFile, rail);
          const mock = await startMockServer(rail, { finalCatalogFault: { app, kind } });
          try {
            const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
            assert.equal(result.code, 1, `${result.stdout}\n${result.stderr}`);
            assert.equal(result.stderr.trim(), "[FAIL] Antigravity acceptance failed (details redacted)");
            assert.ok(fs.existsSync(receiptFile));
            assert.equal(mock.seen.filter((entry) => entry.path === "/api/shares").length, 2);
          } finally {
            await mock.close();
            fs.rmSync(directory, { recursive: true, force: true });
          }
        });
      }
    }
  }
});

test("Antigravity and AGY reject invalid IDs in each complete App catalog", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const app of ["claude", "gemini"]) {
      for (const kind of ["missing", "blank", "duplicate", "nonstring"]) {
        await t.test(`${rail}/${app}/${kind}`, async () => {
          const selected = values(rail);
          const data = [
            { id: selected.models[app], object: "model", owned_by: "antigravity" },
            { id: "zz-antigravity-other", object: "model", owned_by: "antigravity" },
            kind === "missing" ? { object: "model", owned_by: "antigravity" }
              : { id: kind === "blank" ? " " : kind === "duplicate" ? "zz-antigravity-other" : 42, object: "model", owned_by: "antigravity" },
          ];
          const catalogDataByApp = { [app]: data };
          const directory = fs.mkdtempSync(path.join(os.tmpdir(), `${rail}-catalog-id-`));
          const receiptFile = path.join(directory, "receipt.json");
          writeReceipt(receiptFile, rail, {
            catalogs: catalogs(rail, {}, catalogDataByApp),
            scopeDigest: scopeDigest(rail, {}, bindings(rail), selected.shareRevision, catalogDataByApp),
          });
          const mock = await startMockServer(rail, { catalogDataByApp });
          try {
            const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
            assert.equal(result.code, 1, `${result.stdout}\n${result.stderr}`);
            assert.equal(result.stderr.trim(), "[FAIL] Antigravity acceptance failed (details redacted)");
          } finally {
            await mock.close();
            fs.rmSync(directory, { recursive: true, force: true });
          }
        });
      }
    }
  }
});

test("Antigravity and AGY accept pure catalog row reordering", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const app of ["claude", "gemini"]) {
      await t.test(`${rail}/${app}`, async () => {
        const selected = values(rail);
        const catalogDataByApp = { [app]: [
          { id: selected.models[app], object: "model", owned_by: "antigravity" },
          { id: "zz-antigravity-other", object: "model", owned_by: "antigravity" },
        ] };
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `${rail}-catalog-reorder-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, rail, {
          catalogs: catalogs(rail, {}, catalogDataByApp),
          scopeDigest: scopeDigest(rail, {}, bindings(rail), selected.shareRevision, catalogDataByApp),
        });
        const mock = await startMockServer(rail, {
          catalogDataByApp,
          finalCatalogFault: { app, kind: "reorder" },
        });
        try {
          const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
          assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
          assert.match(result.stdout, /contract_verified.*live_pending/);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
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

test("Antigravity private receipt cannot enter the repository through an external symlink", async () => {
  const rail = "antigravity_oauth";
  const insideDirectory = fs.mkdtempSync(path.join(repoRoot, ".antigravity-receipt-test-"));
  const outsideDirectory = fs.mkdtempSync(path.join(os.tmpdir(), "antigravity-receipt-link-"));
  const insideReceipt = path.join(insideDirectory, "receipt.json");
  const linkedReceipt = path.join(outsideDirectory, "receipt.json");
  const mock = await startMockServer(rail);
  try {
    writeReceipt(insideReceipt, rail);
    fs.symlinkSync(insideReceipt, linkedReceipt);
    const result = await runScript(rail, fullEnv(rail, mock.url, linkedReceipt));
    assert.equal(result.code, 1, result.stdout);
    assert.equal(mock.seen.length, 0);
  } finally {
    await mock.close();
    fs.rmSync(outsideDirectory, { recursive: true, force: true });
    fs.rmSync(insideDirectory, { recursive: true, force: true });
  }
});

test("Antigravity private receipts cannot be hard-linked from inside the repository", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const insideDirectory = fs.mkdtempSync(path.join(repoRoot, ".antigravity-receipt-hardlink-"));
      const outsideDirectory = fs.mkdtempSync(path.join(path.dirname(repoRoot), ".antigravity-receipt-outside-"));
      const insideReceipt = path.join(insideDirectory, "receipt.json");
      const outsideReceipt = path.join(outsideDirectory, "receipt.json");
      writeReceipt(insideReceipt, rail);
      fs.linkSync(insideReceipt, outsideReceipt);
      const mock = await startMockServer(rail);
      try {
        const result = await runScript(rail, fullEnv(rail, mock.url, outsideReceipt));
        assert.equal(result.code, 1, result.stdout);
        assert.equal(result.stderr.trim(), "[FAIL] Antigravity acceptance failed (details redacted)");
        assert.deepEqual(mock.seen, []);
      } finally {
        await mock.close();
        fs.rmSync(outsideDirectory, { recursive: true, force: true });
        fs.rmSync(insideDirectory, { recursive: true, force: true });
      }
    });
  }
});

test("Antigravity private receipt cannot be replaced during control-plane validation", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `antigravity-receipt-swap-${rail}-`));
      const receiptFile = path.join(directory, "receipt.json");
      const replacementFile = path.join(directory, "replacement.json");
      writeReceipt(receiptFile, rail, { model: "not-the-selected-model" });
      writeReceipt(replacementFile, rail);
      const originalInode = fs.statSync(receiptFile).ino;
      const mock = await startMockServer(rail, {
        onFirstControl: () => fs.renameSync(replacementFile, receiptFile),
      });
      try {
        const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
        assert.equal(result.code, 1, result.stdout);
        assert.equal(result.stderr.trim(), "[FAIL] Antigravity acceptance failed (details redacted)");
        assert.ok(mock.seen.some((entry) => entry.path === "/api/accounts"));
        assert.notEqual(fs.statSync(receiptFile).ino, originalInode);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Antigravity private receipt rejects an in-place rewrite during its snapshot", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `antigravity-receipt-inplace-${rail}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, rail);
      const preload = [
        'import fs from "node:fs";',
        'const originalRead = fs.readSync;',
        'const target = process.env.ANTIGRAVITY_REAL_RECEIPT_FILE;',
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
      const mock = await startMockServer(rail);
      try {
        const result = await runScript(rail, {
          ...fullEnv(rail, mock.url, receiptFile),
          NODE_OPTIONS: `--import=data:text/javascript,${encodeURIComponent(preload)}`,
        });
        assert.equal(result.code, 1, result.stdout);
        assert.equal(result.stderr.trim(), "[FAIL] Antigravity acceptance failed (details redacted)");
        assert.deepEqual(mock.seen, []);
        assert.match(fs.readFileSync(receiptFile, "utf8"), /^ +$/);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Antigravity private receipt must remain the same file through final acceptance", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const change of ["replacement", "in_place"]) {
      await t.test(`${rail}/${change}`, async () => {
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `antigravity-receipt-final-${rail}-`));
        const receiptFile = path.join(directory, "receipt.json");
        const replacementFile = path.join(directory, "replacement.json");
        writeReceipt(receiptFile, rail);
        if (change === "replacement") {
          fs.writeFileSync(replacementFile, "not the accepted receipt\n", { mode: 0o600 });
        }
        const originalInode = fs.statSync(receiptFile).ino;
        const mock = await startMockServer(rail, {
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
          const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Antigravity acceptance failed (details redacted)");
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

test("Antigravity final acceptance rejects changed bytes despite unchanged reported metadata", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `antigravity-receipt-bytes-${rail}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, rail);
      const preload = [
        'import fs from "node:fs";',
        'const target = process.env.ANTIGRAVITY_REAL_RECEIPT_FILE;',
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
      const mock = await startMockServer(rail, {
        onFirstControl: () => {
          fs.writeFileSync(receiptFile, " ".repeat(fs.statSync(receiptFile).size), { flag: "r+" });
        },
      });
      try {
        const result = await runScript(rail, {
          ...fullEnv(rail, mock.url, receiptFile),
          NODE_OPTIONS: `--import=data:text/javascript,${encodeURIComponent(preload)}`,
        });
        assert.equal(result.code, 1, result.stdout);
        assert.equal(result.stderr.trim(), "[FAIL] Antigravity acceptance failed (details redacted)");
        assert.ok(mock.seen.some((entry) => entry.path === "/api/accounts"));
        assert.match(fs.readFileSync(receiptFile, "utf8"), /^ +$/);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Antigravity private receipt size is bounded before control-plane requests", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `antigravity-receipt-size-${rail}-`));
      const receiptFile = path.join(directory, "receipt.json");
      fs.writeFileSync(receiptFile, "x".repeat(1024 * 1024 + 1), { mode: 0o600 });
      const mock = await startMockServer(rail);
      try {
        const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
        assert.equal(result.code, 1, result.stdout);
        assert.equal(result.stderr.trim(), "[FAIL] Antigravity acceptance failed (details redacted)");
        assert.deepEqual(mock.seen, []);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
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

test("Antigravity and AGY reject ambiguous or inactive Shares before catalog requests", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const [name, options] of [
      ["duplicate_claude", { duplicateShareApp: "claude" }],
      ["duplicate_gemini", { duplicateShareApp: "gemini" }],
      ["primary_mismatch", { primaryMismatch: true }],
      ["disabled", { shareState: "disabled" }],
      ["paused", { shareState: "paused" }],
    ]) {
      await t.test(`${rail}/${name}`, async () => {
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `antigravity-share-${rail}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, rail);
        const mock = await startMockServer(rail, options);
        try {
          const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Antigravity acceptance failed (details redacted)");
          assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 0);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Antigravity and AGY reject incomplete Provider or Share revision scope before catalogs", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const scopeFault of [
      "missing_claude_revision",
      "negative_gemini_revision",
      "empty_claude_fingerprint",
      "empty_gemini_fingerprint",
      "missing_share_revision",
      "negative_share_revision",
    ]) {
      await t.test(`${rail}/${scopeFault}`, async () => {
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `antigravity-scope-${rail}-`));
        const receiptFile = path.join(directory, "receipt.json");
        const scopedBindings = bindings(rail).map((binding) => ({
          ...binding,
          providerRevision: scopeFault === `missing_${binding.app}_revision` ? 0
            : scopeFault === `negative_${binding.app}_revision` ? -1
              : binding.providerRevision,
          runtimeFingerprint: scopeFault === `empty_${binding.app}_fingerprint`
            ? "" : binding.runtimeFingerprint,
        }));
        const shareRevision = scopeFault === "missing_share_revision" ? 0
          : scopeFault === "negative_share_revision" ? -1
            : values(rail).shareRevision;
        writeReceipt(receiptFile, rail, {
          scopeDigest: scopeDigest(rail, {}, scopedBindings, shareRevision),
          generations: {
            ...receipt(rail).generations,
            providerBindingDigest: digest(
              "cc-switch-server:antigravity-provider-bindings:v1",
              scopedBindings,
            ),
            shareRevision,
          },
        });
        const mock = await startMockServer(rail, { scopeFault });
        try {
          const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Antigravity acceptance failed (details redacted)");
          assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 0);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Antigravity and AGY reject legacy top-level revision masking real Provider scope", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const app of ["claude", "gemini"]) {
      for (const fault of [
        "legacy_top_level_only",
        "missing_view_revision",
        "missing_runtime_revision",
        "mismatched_revisions",
      ]) {
        await t.test(`${rail}/${app}/${fault}`, async () => {
          const directory = fs.mkdtempSync(path.join(os.tmpdir(), `antigravity-real-scope-${rail}-`));
          const receiptFile = path.join(directory, "receipt.json");
          writeReceipt(receiptFile, rail);
          const scopeFault = fault === "legacy_top_level_only" ? `legacy_${app}_top_level_only`
            : fault === "missing_view_revision" ? `missing_${app}_view_revision`
              : fault === "missing_runtime_revision" ? `missing_${app}_runtime_revision`
                : `mismatched_${app}_revisions`;
          const mock = await startMockServer(rail, { scopeFault });
          try {
            const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
            assert.equal(result.code, 1, result.stdout);
            assert.equal(result.stderr.trim(), "[FAIL] Antigravity acceptance failed (details redacted)");
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

test("Antigravity receipt gate rejects stale catalog and receipt on both rails", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const invalid of ["catalog_old", "catalog_future", "receipt_old", "receipt_future"]) {
      await t.test(`${rail}/${invalid}`, async () => {
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `${rail}-${invalid}-`));
        const receiptFile = path.join(directory, "receipt.json");
        const mock = await startMockServer(rail, {
          catalogAgeMs: invalid === "catalog_old"
            ? 3 * 60 * 60 * 1_000
            : invalid === "catalog_future" ? -10 * 60 * 1_000 : 0,
        });
        try {
          const recordedAt = invalid === "receipt_old"
            ? new Date(Date.now() - 2 * 24 * 60 * 60 * 1_000).toISOString()
            : invalid === "receipt_future"
              ? new Date(Date.now() + 10 * 60 * 1_000).toISOString()
              : undefined;
          writeReceipt(receiptFile, rail, recordedAt ? { recordedAt } : {});
          const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
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

test("Antigravity rails reject catalog or receipt evidence expiring during final binding validation", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const evidence of ["claude_catalog", "gemini_catalog", "receipt"]) {
      await t.test(`${rail}/${evidence}`, async () => {
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `${rail}-${evidence}-expiry-`));
        const receiptFile = path.join(directory, "receipt.json");
        const app = evidence.endsWith("_catalog") ? evidence.replace("_catalog", "") : "";
        const catalogAtByApp = app
          ? { [app]: Date.now() - 2 * 60 * 60 * 1_000 + 1_200 }
          : {};
        const mock = await startMockServer(rail, {
          catalogAtByApp,
          finalShareDelayMs: 2_000,
        });
        try {
          writeReceipt(receiptFile, rail, app
            ? {
              catalogs: catalogs(rail, catalogAtByApp),
              scopeDigest: scopeDigest(rail, catalogAtByApp),
            }
            : { recordedAt: new Date(Date.now() - 24 * 60 * 60 * 1_000 + 1_200).toISOString() });
          const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
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

test("Antigravity receipt gate rejects Account, Provider, Share and refresh-credential drift on both rails", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const drift of ["account", "provider", "share", "refresh_credential"]) {
      await t.test(`${rail}/${drift}`, async () => {
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `${rail}-${drift}-`));
        const receiptFile = path.join(directory, "receipt.json");
        const mock = await startMockServer(rail, { drift });
        try {
          writeReceipt(receiptFile, rail);
          const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
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

test("Antigravity receipt rejects an OAuth Account without a refresh credential before catalog requests", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `${rail}-missing-refresh-`));
      const receiptFile = path.join(directory, "receipt.json");
      const mock = await startMockServer(rail, { hasRefreshToken: false });
      try {
        writeReceipt(receiptFile, rail);
        const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /details redacted/);
        assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 0);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Antigravity and AGY require explicit false Account booleans", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const field of ["hasApiKey", "needsRelogin"]) {
      for (const variant of ["missing", "null", "string"]) {
        await t.test(`${rail}/${field}/${variant}`, async () => {
          const directory = fs.mkdtempSync(path.join(os.tmpdir(), `${rail}-account-`));
          const receiptFile = path.join(directory, "receipt.json");
          const mock = await startMockServer(rail, {
            accountFault: { field, variant },
          });
          try {
            writeReceipt(receiptFile, rail);
            const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
            assert.equal(result.code, 1, result.stdout);
            assert.equal(result.stderr.trim(), "[FAIL] Antigravity acceptance failed (details redacted)");
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

test("Antigravity v2 receipt binds complete catalog contents and its snapshot on both rails", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const invalid of ["catalog_content_changed", "receipt_catalog_tampered"]) {
      await t.test(`${rail}/${invalid}`, async () => {
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `${rail}-${invalid}-`));
        const receiptFile = path.join(directory, "receipt.json");
        const mock = await startMockServer(rail, {
          catalogChanged: invalid === "catalog_content_changed",
        });
        try {
          const overrides = invalid === "receipt_catalog_tampered"
            ? { catalogs: catalogs(rail).map((snapshot) => ({
              ...snapshot,
              digest: "0".repeat(64),
            })) }
            : {};
          writeReceipt(receiptFile, rail, overrides);
          const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.match(result.stderr, /details redacted/);
          assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 2);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Antigravity control-plane response bound rejects unfinished oversized bodies on both rails", async (t) => {
  for (const [rail, oversizedControl, expectedPaths] of [
    ["antigravity_oauth", "declared", ["/api/accounts"]],
    ["agy_oauth", "chunked", ["/api/accounts", "/api/providers"]],
  ]) {
    await t.test(`${rail}/${oversizedControl}`, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `antigravity-oversized-${rail}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, rail);
      const mock = await startMockServer(rail, { oversizedControl });
      try {
        const startedAt = Date.now();
        const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
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
