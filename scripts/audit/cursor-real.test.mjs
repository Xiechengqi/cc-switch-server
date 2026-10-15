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

function binding(rail, overrides = {}) {
  const spec = specs[rail];
  return {
    providerRevision: 3,
    runtimeFingerprint: `cursor-${rail}-runtime`,
    shareRevision: 5,
    authIdentityGeneration: rail === "oauth" ? spec.generation : null,
    tokenRefreshGeneration:
      rail === "oauth" ? spec.tokenRefreshGeneration : null,
    credentialGeneration: rail === "api_key" ? spec.generation : null,
    ...overrides,
  };
}

function scopeDigest(rail, current = binding(rail)) {
  const value = ids(rail);
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

function providerBindingDigest(rail, current = binding(rail)) {
  const value = ids(rail);
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

function buildReceipt(rail, current = binding(rail)) {
  const spec = specs[rail];
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
    scopeDigest: scopeDigest(rail, current),
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
      providerBindingDigest: providerBindingDigest(rail, current),
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

function writeReceipt(file, rail, mutate = () => {}, current = binding(rail)) {
  const receipt = buildReceipt(rail, current);
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
  { providerRail = rail, failProvidersWith = "", drift = "", shareFault = "", scopeFault = "", accountFault = "", catalogSource = "cursor_public_api", catalogStale = false, catalogAgeMs = 0, catalogDrift = "", catalogExtraModel = false, catalogInvalidKind = "", finalShareDelayMs = 0, oversizedControl = "", onFirstControl = null } = {},
) {
  const spec = specs[providerRail];
  const value = ids(rail);
  const seen = [];
  let accountReads = 0;
  let providerReads = 0;
  let shareReads = 0;
  let catalogReads = 0;
  let firstControlSeen = false;
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    seen.push({
      path: url.pathname,
      search: url.search,
      authorization: request.headers.authorization || "",
    });
    if (!firstControlSeen && ["/api/accounts", "/api/providers"].includes(url.pathname)) {
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
            email: "cursor-fixture@example.test",
            providerType: "cursor_oauth",
            authIdentityGeneration: specs.oauth.generation,
            tokenRefreshGeneration: specs.oauth.tokenRefreshGeneration + (drift === "account" && accountReads > 1 ? 1 : 0),
            hasAccessToken: true,
            hasRefreshToken: true,
            hasApiKey: accountFault === "missing_has_api_key" ? undefined : false,
            needsRelogin: accountFault === "missing_needs_relogin" ? undefined : false,
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
            revision: ["missing_view_revision", "missing_both_revisions"].includes(scopeFault) ? undefined
              : 3 + (scopeFault === "mismatched_revisions" ? 1 : 0)
                + (drift === "provider" && providerReads > 1 ? 1 : 0),
            provider: { id: value.providerId },
            runtime: {
              providerRevision: ["missing_runtime_revision", "missing_both_revisions"].includes(scopeFault)
                ? undefined : 3 + (drift === "provider" && providerReads > 1 ? 1 : 0),
              driverId: "special.cursor",
              configurationState: "ready",
              runtimeFingerprint: scopeFault === "blank_runtime_fingerprint"
                ? "   " : `cursor-${providerRail}-runtime`,
              authRef: drift === "credential" && providerReads > 1 && providerRail === "api_key"
                ? { ...authRef, credentialGeneration: spec.generation + 1 }
                : authRef,
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
            app,
            providerId: shareFault === "primary_conflict" ? "cursor-other-provider" : value.providerId,
            providerType: spec.providerType,
            enabled: shareFault !== "disabled",
            status: shareFault === "paused" ? "paused" : "active",
            configRevision: scopeFault === "missing_share_revision" ? undefined
              : 5 + (drift === "share" && shareReads > 1 ? 1 : 0),
            bindings: [
              { app, providerId: value.providerId, providerType: spec.providerType },
              ...(shareFault === "duplicate_conflict"
                ? [{ app, providerId: "cursor-other-provider", providerType: spec.providerType }]
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
      assert.equal(url.searchParams.get("app"), app);
      assert.equal(url.searchParams.get("providerId"), value.providerId);
      const data = [
        ...(catalogDrift === "model_removed" && catalogReads > 1
          ? [] : [{
            id: model,
            object: "model",
            owned_by: "cursor",
            capabilities: { tools: catalogDrift === "selected_capability" && catalogReads > 1 ? false : true },
          }]),
        ...(catalogExtraModel || (catalogDrift === "model_added" && catalogReads > 1)
          ? [{ id: "newly-visible-fast-model", object: "model", owned_by: "cursor" }] : []),
        ...(catalogInvalidKind ? [
          { id: "zz-cursor-other", object: "model", owned_by: "cursor" },
          catalogInvalidKind === "missing" ? { object: "model", owned_by: "cursor" }
            : { id: catalogInvalidKind === "blank" ? " "
              : catalogInvalidKind === "duplicate" ? "zz-cursor-other" : 42,
            object: "model", owned_by: "cursor" },
        ] : []),
      ];
      sendJson(response, 200, {
        object: "list",
        source: catalogDrift === "source" && catalogReads > 1 ? "static_registry" : catalogSource,
        stale: catalogStale || (catalogDrift === "stale" && catalogReads > 1),
        fetchedAtMs: Date.now() - catalogAgeMs,
        data: catalogDrift === "order" && catalogReads > 1 ? [...data].reverse() : data,
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
          rail === "oauth" ? 2 : 0,
        );
        assert.equal(
          mock.seen.filter((entry) => entry.path === "/api/providers").length,
          2,
        );
        assert.equal(
          mock.seen.filter((entry) => entry.path === "/api/shares").length,
          2,
        );
        assert.equal(
          mock.seen.filter((entry) => entry.path === "/v1/models").length,
          rail === "api_key" ? 2 : 1,
        );
        const catalogRequest = mock.seen.find((entry) => entry.path === "/v1/models");
        const catalogQuery = new URLSearchParams(catalogRequest.search);
        assert.equal(catalogQuery.get("expectedShareId"), ids(rail).shareId);
        assert.equal(catalogQuery.get("expectedProviderId"), ids(rail).providerId);
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

test("Cursor private receipts cannot be hard-linked from inside the repository", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const insideDirectory = fs.mkdtempSync(path.join(repoRoot, ".cursor-receipt-hardlink-"));
      const outsideDirectory = fs.mkdtempSync(path.join(path.dirname(repoRoot), ".cursor-receipt-outside-"));
      const insideReceipt = path.join(insideDirectory, "receipt.json");
      const outsideReceipt = path.join(outsideDirectory, "receipt.json");
      writeReceipt(insideReceipt, rail);
      fs.linkSync(insideReceipt, outsideReceipt);
      const mock = await startMockServer(rail);
      try {
        const result = await runScript(rail, fullEnv(rail, mock.url, outsideReceipt));
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

test("Cursor receipt cannot be replaced during control-plane validation", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `cursor-receipt-swap-${rail}-`));
      const receiptFile = path.join(directory, "receipt.json");
      const replacementFile = path.join(directory, "replacement.json");
      writeReceipt(receiptFile, rail, (value) => { value.model = "not-the-selected-model"; });
      writeReceipt(replacementFile, rail);
      const originalInode = fs.statSync(receiptFile).ino;
      const mock = await startMockServer(rail, {
        onFirstControl: () => fs.renameSync(replacementFile, receiptFile),
      });
      try {
        const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /details redacted/);
        assert.ok(mock.seen.some((entry) => entry.path.startsWith("/api/")));
        assert.notEqual(fs.statSync(receiptFile).ino, originalInode);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Cursor private receipt must remain the same file through final acceptance", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const change of ["replacement", "in_place"]) {
      await t.test(`${rail}/${change}`, async () => {
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `cursor-receipt-final-${rail}-`));
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
          assert.equal(result.stderr.trim(), "[FAIL] Cursor acceptance failed (details redacted)");
          assert.ok(mock.seen.some((entry) => entry.path.startsWith("/api/")));
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

test("Cursor final acceptance rejects changed bytes despite unchanged reported metadata", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `cursor-receipt-bytes-${rail}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, rail);
      const preload = [
        'import fs from "node:fs";',
        `const target = process.env[${JSON.stringify(specs[rail].receiptEnv)}];`,
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
        assert.equal(result.stderr.trim(), "[FAIL] Cursor acceptance failed (details redacted)");
        assert.ok(mock.seen.some((entry) => entry.path.startsWith("/api/")));
        assert.match(fs.readFileSync(receiptFile, "utf8"), /^ +$/);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Cursor receipt rejects an in-place rewrite during its snapshot", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `cursor-receipt-inplace-${rail}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, rail);
      const preload = [
        'import fs from "node:fs";',
        'const originalRead = fs.readSync;',
        `const target = process.env[${JSON.stringify(specs[rail].receiptEnv)}];`,
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

test("Cursor OAuth Account requires explicit credential and relogin booleans", async (t) => {
  for (const accountFault of ["missing_has_api_key", "missing_needs_relogin"]) {
    await t.test(accountFault, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `cursor-account-${accountFault}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, "oauth");
      const mock = await startMockServer("oauth", { accountFault });
      try {
        const result = await runScript("oauth", fullEnv("oauth", mock.url, receiptFile));
        assert.equal(result.code, 1, result.stdout);
        assert.equal(result.stderr.trim(), "[FAIL] Cursor acceptance failed (details redacted)");
        assert.equal(mock.seen.filter((entry) => entry.path === "/api/providers").length, 0);
        assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 0);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
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

test("Cursor acceptance rejects binding drift on both rails", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const drift of rail === "oauth"
      ? ["account", "provider", "share"]
      : ["credential", "provider", "share"]) {
      await t.test(`${rail}/${drift}`, async () => {
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `cursor-${rail}-${drift}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, rail);
        const mock = await startMockServer(rail, { drift });
        try {
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

test("Cursor Share rejects ambiguous or inactive bindings before catalog access", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const shareFault of ["duplicate_conflict", "primary_conflict", "disabled", "paused"]) {
      await t.test(`${rail}/${shareFault}`, async () => {
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `cursor-share-${rail}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, rail);
        const mock = await startMockServer(rail, { shareFault });
        try {
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
  }
});

test("Cursor rails reject incomplete or inconsistent Provider and Share revision scope before catalogs", async (t) => {
  for (const rail of Object.keys(specs)) {
    for (const scopeFault of [
      "missing_runtime_revision",
      "missing_view_revision",
      "mismatched_revisions",
      "missing_both_revisions",
      "missing_share_revision",
      "blank_runtime_fingerprint",
    ]) {
      await t.test(`${rail}/${scopeFault}`, async () => {
        const current = binding(rail, {
          ...(scopeFault === "missing_both_revisions" ? { providerRevision: 0 } : {}),
          ...(scopeFault === "missing_share_revision" ? { shareRevision: 0 } : {}),
          ...(scopeFault === "blank_runtime_fingerprint" ? { runtimeFingerprint: "   " } : {}),
        });
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `cursor-scope-${rail}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, rail, () => {}, current);
        const mock = await startMockServer(rail, { scopeFault });
        try {
          const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Cursor acceptance failed (details redacted)");
          assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 0);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Cursor API-key receipt rejects non-fresh public model evidence", async (t) => {
  for (const [invalid, options] of [
    ["stale", { catalogStale: true }],
    ["old", { catalogAgeMs: 10 * 60 * 1_000 }],
    ["future", { catalogAgeMs: -10 * 60 * 1_000 }],
    ["wrong_source", { catalogSource: "static_registry" }],
  ]) {
    await t.test(invalid, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `cursor-catalog-${invalid}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, "api_key");
      const mock = await startMockServer("api_key", options);
      try {
        const result = await runScript("api_key", fullEnv("api_key", mock.url, receiptFile));
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /details redacted/);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Cursor API-key receipt rechecks the selected model and catalog authority before acceptance", async (t) => {
  for (const catalogDrift of ["model_removed", "stale", "source"]) {
    await t.test(catalogDrift, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `cursor-catalog-drift-${catalogDrift}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, "api_key");
      const mock = await startMockServer("api_key", { catalogDrift });
      try {
        const result = await runScript("api_key", fullEnv("api_key", mock.url, receiptFile));
        assert.equal(result.code, 1, result.stdout);
        assert.equal(result.stderr.trim(), "[FAIL] Cursor acceptance failed (details redacted)");
        assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 2);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Cursor API-key receipt rejects final catalog content drift beyond selected id presence", async (t) => {
  for (const catalogDrift of ["model_added", "selected_capability"]) {
    await t.test(catalogDrift, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `cursor-catalog-content-${catalogDrift}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, "api_key");
      const mock = await startMockServer("api_key", { catalogDrift });
      try {
        const result = await runScript("api_key", fullEnv("api_key", mock.url, receiptFile));
        assert.equal(result.code, 1, `${result.stdout}\n${result.stderr}`);
        assert.equal(result.stderr.trim(), "[FAIL] Cursor acceptance failed (details redacted)");
        assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 2);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Cursor API-key receipt rejects invalid IDs in the full public catalog", async (t) => {
  for (const catalogInvalidKind of ["missing", "blank", "duplicate", "nonstring"]) {
    await t.test(catalogInvalidKind, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `cursor-catalog-id-${catalogInvalidKind}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, "api_key");
      const mock = await startMockServer("api_key", { catalogInvalidKind });
      try {
        const result = await runScript("api_key", fullEnv("api_key", mock.url, receiptFile));
        assert.equal(result.code, 1, `${result.stdout}\n${result.stderr}`);
        assert.equal(result.stderr.trim(), "[FAIL] Cursor acceptance failed (details redacted)");
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Cursor API-key receipt allows a pure catalog row reorder", async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "cursor-catalog-reorder-"));
  const receiptFile = path.join(directory, "receipt.json");
  writeReceipt(receiptFile, "api_key");
  const mock = await startMockServer("api_key", {
    catalogExtraModel: true,
    catalogDrift: "order",
  });
  try {
    const result = await runScript("api_key", fullEnv("api_key", mock.url, receiptFile));
    assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
    assert.equal(mock.seen.filter((entry) => entry.path === "/v1/models").length, 2);
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("Cursor API-key receipt rejects a catalog that expires before acceptance", async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "cursor-catalog-expiry-"));
  const receiptFile = path.join(directory, "receipt.json");
  writeReceipt(receiptFile, "api_key");
  const mock = await startMockServer("api_key", {
    catalogAgeMs: 5 * 60_000 - 1_000,
    finalShareDelayMs: 1_500,
  });
  try {
    const result = await runScript("api_key", fullEnv("api_key", mock.url, receiptFile));
    assert.equal(mock.seen.filter((entry) => entry.path === "/api/shares").length, 2);
    assert.equal(result.code, 1, result.stdout);
    assert.match(result.stderr, /details redacted/);
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("Cursor OAuth and API-key receipts expiring during final binding validation are rejected", async (t) => {
  for (const rail of Object.keys(specs)) {
    await t.test(rail, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `cursor-${rail}-receipt-expiry-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, rail, (receipt) => {
        receipt.recordedAt = new Date(Date.now() - 24 * 60 * 60 * 1_000 + 1_200).toISOString();
      });
      const mock = await startMockServer(rail, { finalShareDelayMs: 2_000 });
      try {
        const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
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

test("Cursor control-plane response bound rejects unfinished oversized bodies on both rails", async (t) => {
  for (const [rail, oversizedControl, expectedPath] of [
    ["oauth", "declared", "/api/accounts"],
    ["api_key", "chunked", "/api/providers"],
  ]) {
    await t.test(`${rail}/${oversizedControl}`, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `cursor-oversized-${rail}-`));
      const receiptFile = path.join(directory, "receipt.json");
      writeReceipt(receiptFile, rail);
      const mock = await startMockServer(rail, { oversizedControl });
      try {
        const startedAt = Date.now();
        const result = await runScript(rail, fullEnv(rail, mock.url, receiptFile));
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /details redacted/);
        assert.deepEqual(mock.seen.map((entry) => entry.path), [expectedPath]);
        assert.ok(Date.now() - startedAt < 3_000, "oversized response waited for timeout");
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});
