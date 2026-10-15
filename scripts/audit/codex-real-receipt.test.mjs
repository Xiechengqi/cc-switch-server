import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import test from "node:test";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const script = path.join(repoRoot, "scripts/smoke/codex-real-receipt.mjs");
const contract = JSON.parse(
  fs.readFileSync(
    path.join(repoRoot, "assets/contract/codex-reference-delta.json"),
    "utf8",
  ),
);
const targetCommit = execFileSync("git", ["rev-parse", "HEAD"], {
  cwd: repoRoot,
  encoding: "utf8",
}).trim();
const serverSecret = "codex-server-secret-for-harness";
const routerSecret = "codex-router-secret-for-harness";

test("Codex receipt manifest query tracks the server default client version", () => {
  const source = fs.readFileSync(path.join(repoRoot, "src/codex_identity.rs"), "utf8");
  assert.match(source, /DEFAULT_CODEX_VERSION: &str = "0\.144\.1"/);
});

const specs = Object.freeze({
  gpt_6_astra: Object.freeze({
    kind: "model",
    providerEnv: "CC_SWITCH_CODEX_GPT_6_ASTRA_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CODEX_GPT_6_ASTRA_SHARE_ID",
    modelEnv: "CC_SWITCH_CODEX_GPT_6_ASTRA_MODEL",
    receiptEnv: "CODEX_GPT_6_ASTRA_REAL_RECEIPT_FILE",
    model: "gpt-6-astra",
  }),
  gpt_image_2_5: Object.freeze({
    kind: "image",
    providerEnv: "CC_SWITCH_CODEX_GPT_IMAGE_2_5_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CODEX_GPT_IMAGE_2_5_SHARE_ID",
    modelEnv: "CC_SWITCH_CODEX_GPT_IMAGE_2_5_MODEL",
    receiptEnv: "CODEX_GPT_IMAGE_2_5_REAL_RECEIPT_FILE",
    model: "gpt-image-2.5",
  }),
  gpt_image_2_5_flare: Object.freeze({
    kind: "image",
    providerEnv: "CC_SWITCH_CODEX_GPT_IMAGE_2_5_FLARE_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CODEX_GPT_IMAGE_2_5_FLARE_SHARE_ID",
    modelEnv: "CC_SWITCH_CODEX_GPT_IMAGE_2_5_FLARE_MODEL",
    receiptEnv: "CODEX_GPT_IMAGE_2_5_FLARE_REAL_RECEIPT_FILE",
    model: "gpt-image-2.5-flare",
  }),
  gpt_image_2_5_sunburst: Object.freeze({
    kind: "image",
    providerEnv: "CC_SWITCH_CODEX_GPT_IMAGE_2_5_SUNBURST_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CODEX_GPT_IMAGE_2_5_SUNBURST_SHARE_ID",
    modelEnv: "CC_SWITCH_CODEX_GPT_IMAGE_2_5_SUNBURST_MODEL",
    receiptEnv: "CODEX_GPT_IMAGE_2_5_SUNBURST_REAL_RECEIPT_FILE",
    model: "gpt-image-2.5-sunburst",
  }),
  ws_prewarm: Object.freeze({
    kind: "websocket",
    providerEnv: "CC_SWITCH_CODEX_WS_PREWARM_PROVIDER_ID",
    shareEnv: "CC_SWITCH_CODEX_WS_PREWARM_SHARE_ID",
    modelEnv: "CC_SWITCH_CODEX_WS_PREWARM_MODEL",
    receiptEnv: "CODEX_WS_PREWARM_REAL_RECEIPT_FILE",
    model: "gpt-5.4-mini",
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
  return {
    operation,
    spec,
    accountId: `codex-${operation}-account`,
    providerId: `codex-${operation}-provider`,
    shareId: `codex-${operation}-share`,
    authIdentityGeneration: index + 6,
    tokenRefreshGeneration: index + 2,
    providerRevision: 3,
    shareRevision: 5,
    runtimeFingerprint: `codex-${operation}-runtime`,
  };
}

function expectedDecisions(value) {
  const common = {
    crossAccountFallback: "disabled",
    crossProviderFallback: "disabled",
    crossShareFallback: "disabled",
    crossRailFallback: "disabled",
    crossSiteFallback: "disabled",
    postCommitReplay: "disabled",
  };
  return value.spec.kind === "image"
    ? {
        ...common,
        modelFallback: "disabled",
        quotaCooldownScope: "usage_limit_account_else_share_runtime_model",
        capabilityAccess: "same_share_host_authenticated",
      }
    : value.spec.kind === "model"
      ? {
          ...common,
          modelFallback: "disabled",
          prismRoute: "disabled",
          quotaCooldownScope: "usage_limit_account_else_share_runtime_model",
        }
      : {
        ...common,
        websocketHttpFallback: "pre_response_create_only",
        connectionReuse: "same_session_scope_only",
        prewarmActivation: "receipt_and_benchmark_gated",
      };
}

function measurements(value) {
  return value.spec.kind === "image"
    ? {
        generationRuns: 2,
        editRuns: 1,
        responsesRuns: 2,
        replicaCount: 2,
        cloudflareRuns: 1,
      }
    : value.spec.kind === "model"
      ? {
          httpRuns: 1,
          sseRuns: 1,
          websocketRuns: 1,
          quotaRuns: 2,
          siblingControlRuns: 1,
        }
      : {
        benchmarkSamples: 7,
        coldTtfbP50Ms: 220,
        prewarmedTtfbP50Ms: 140,
        upstreamConnections: 1,
        turns: 2,
      };
}

function scopeDigest(value) {
  return digest("cc-switch-server:codex-real-scope:v1", {
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
    harnessRevision: 1,
  });
}

function receipt(value, overrides = {}) {
  const operationContract = [
    ...(contract.realAcceptance.operations || []),
    ...(contract.realAcceptanceExtensions || []).flatMap(
      (extension) => extension.operations || [],
    ),
  ].find(
    (candidate) => candidate.operation === value.operation,
  );
  return {
    schemaVersion: 1,
    providerFamily: "codex",
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
        digest(`codex-fixture:${value.operation}`, name),
      ]),
    ),
    measurements: measurements(value),
    generations: {
      authIdentityGeneration: value.authIdentityGeneration,
      tokenRefreshGeneration: value.tokenRefreshGeneration,
      providerRevision: value.providerRevision,
      shareRevision: value.shareRevision,
      providerBindingDigest: digest("cc-switch-server:codex-provider-binding:v1", {
        providerId: value.providerId,
        runtimeFingerprint: value.runtimeFingerprint,
        accountId: value.accountId,
        authIdentityGeneration: value.authIdentityGeneration,
      }),
    },
    decisions: expectedDecisions(value),
    decoyRequests: {
      otherAccount: 0,
      otherProvider: 0,
      otherShare: 0,
      otherRail: 0,
      otherSite: 0,
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
    CODEX_OAUTH_TEST_ACCOUNT: "",
    CODEX_REAL_RECEIPT_FILE: "",
  };
  for (const spec of Object.values(specs)) {
    cleared[spec.providerEnv] = "";
    cleared[spec.shareEnv] = "";
    cleared[spec.modelEnv] = "";
    cleared[spec.receiptEnv] = "";
  }
  const args = operation ? [script, "--operation", operation] : [script];
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, args, {
      cwd: repoRoot,
      env: {
        ...process.env,
        ...cleared,
        RUN_REAL: "1",
        CC_SWITCH_CODEX_HARNESS_MODE: "fixture",
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
  { providerGeneration = value.authIdentityGeneration, providerFailure = "", duplicateShareBinding = false, primaryMismatch = false, shareState = "active", scopeFault = "", drift = "", oversizedAccounts = false, onFirstAccount = null, accountFault = null, manifestModels = [value.spec.model], catalogModels = [value.spec.model], finalShareDelayMs = 0, finalModelFault = "" } = {},
) {
  const seen = [];
  const manifestQueries = [];
  const catalogQueries = [];
  let catalogReads = 0;
  let manifestReads = 0;
  let accountReads = 0;
  let providerReads = 0;
  let shareReads = 0;
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    seen.push(url.pathname);
    if (request.method === "GET" && url.pathname === "/api/accounts") {
      accountReads += 1;
      if (accountReads === 1) onFirstAccount?.();
      if (oversizedAccounts) {
        response.writeHead(200, {
          "content-type": "application/json",
          ...(oversizedAccounts === "declared"
            ? { "content-length": String(4 * 1024 * 1024 + 1) }
            : {}),
        });
        response.write(oversizedAccounts === "declared" ? "{" : "x".repeat(4 * 1024 * 1024 + 1));
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
            providerType: "codex_oauth",
            authIdentityGeneration: value.authIdentityGeneration,
            tokenRefreshGeneration: value.tokenRefreshGeneration + (drift === "account" && accountReads > 1 ? 1 : 0),
            hasAccessToken: true,
            hasRefreshToken: true,
            ...accountFlags,
          },
        ],
      });
      return;
    }
    if (request.method === "GET" && url.pathname === "/api/providers") {
      providerReads += 1;
      if (providerFailure) {
        response.writeHead(500, { "content-type": "text/plain" });
        response.end(providerFailure);
        return;
      }
      sendJson(response, 200, {
        ok: true,
        providers: [
          {
            app: "codex",
            providerType: "codex_oauth",
            providerTypeId: "codex_oauth",
            revision: ["missing_provider_revision", "legacy_top_level_only", "missing_view_revision"].includes(scopeFault)
              ? undefined : value.providerRevision + (scopeFault === "mismatched_revisions" ? 1 : 0)
                + (drift === "provider" && providerReads > 1 ? 1 : 0),
            ...(["legacy_top_level_only", "missing_view_revision", "missing_runtime_revision", "mismatched_revisions"].includes(scopeFault)
              ? { providerRevision: value.providerRevision + (drift === "provider" && providerReads > 1 ? 1 : 0) } : {}),
            provider: { id: value.providerId },
            runtime: {
              providerRevision: ["missing_provider_revision", "legacy_top_level_only", "missing_runtime_revision"].includes(scopeFault)
                ? undefined : value.providerRevision + (drift === "provider" && providerReads > 1 ? 1 : 0),
              driverId: "oauth.openai_codex",
              configurationState: "ready",
              runtimeFingerprint: value.runtimeFingerprint,
              authRef: {
                kind: "managed_account",
                accountId: value.accountId,
                expectedProviderType: "codex_oauth",
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
            providerId: primaryMismatch ? "conflicting-primary-provider" : value.providerId,
            providerType: "codex_oauth",
            enabled: shareState !== "disabled",
            status: shareState === "paused" ? "paused" : "active",
            configRevision: scopeFault === "missing_share_revision" ? undefined
              : value.shareRevision + (drift === "share" && shareReads > 1 ? 1 : 0),
            bindings: [
              {
                app: "codex",
                providerId: value.providerId,
                providerType: "codex_oauth",
              },
              ...(duplicateShareBinding ? [{
                app: "codex",
                providerId: "conflicting-codex-provider",
                providerType: "codex_oauth",
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
      if (url.searchParams.has("client_version")) {
        assert.equal(url.searchParams.get("client_version"), "0.144.1");
        manifestQueries.push(url.searchParams);
        manifestReads += 1;
        const fault = manifestReads > 1 ? finalModelFault : "";
        sendJson(response, 200, {
          models: [
            ...(fault === "manifest_reorder" ? [...manifestModels].reverse() : manifestModels)
              .filter((slug) => !(fault === "manifest_remove" && slug === value.spec.model))
              .map((slug) => ({
                slug,
                use_responses_lite: fault === "manifest_capability" && slug === value.spec.model
                  ? false : true,
              })),
            ...(fault === "manifest_add" ? [{ slug: "unexpected-model" }] : []),
          ],
        });
      } else {
        catalogQueries.push(url.searchParams);
        catalogReads += 1;
        const fault = catalogReads > 1 ? finalModelFault : "";
        let catalogData = catalogModels.map((id) => ({ id }));
        if (fault === "catalog_remove") catalogData = [];
        if (fault === "catalog_add") catalogData = [...catalogData, { id: "unexpected-model" }];
        if (fault === "catalog_reorder") catalogData = [...catalogData].reverse();
        if (fault === "catalog_capability") {
          catalogData = catalogData.map((entry) => entry.id === value.spec.model
            ? { ...entry, capabilities: { tools: false } }
            : entry);
        }
        sendJson(response, 200, {
          object: "list",
          data: catalogData,
        });
      }
      return;
    }
    sendJson(response, 404, { error: "not_found" });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  assert.ok(address && typeof address === "object");
  return {
    origin: `http://127.0.0.1:${address.port}`,
    seen,
    manifestQueries,
    catalogQueries,
    close: () => new Promise((resolve) => server.close(resolve)),
  };
}

function fixtureEnvironment(value, origin, receiptFile, overrides = {}) {
  return {
    SERVER_URL: origin,
    CC_SWITCH_SERVER_TOKEN: serverSecret,
    CC_SWITCH_SHARE_URL: origin,
    ROUTER_API_TOKEN: routerSecret,
    CODEX_OAUTH_TEST_ACCOUNT: value.accountId,
    [value.spec.providerEnv]: value.providerId,
    [value.spec.shareEnv]: value.shareId,
    [value.spec.modelEnv]: value.spec.model,
    [value.spec.receiptEnv]: receiptFile,
    ...overrides,
  };
}

async function withRun(operation, options = {}) {
  const value = fixture(operation);
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "codex-receipt-test-"));
  const receiptFile = path.join(directory, "receipt.json");
  writeReceipt(receiptFile, value, options.receiptOverrides);
  const mock = await startMockServer(value, options.serverOptions);
  try {
    const result = await runScript(
      operation,
      fixtureEnvironment(value, mock.origin, receiptFile, options.envOverrides),
    );
    return {
      result,
      value,
      seen: mock.seen,
      manifestQueries: mock.manifestQueries,
      catalogQueries: mock.catalogQueries,
    };
  } finally {
    await mock.close();
    fs.rmSync(directory, { recursive: true, force: true });
  }
}

test("missing Codex operation stays blocked without network access", async () => {
  const result = await runScript("");
  assert.equal(result.code, 0);
  const output = JSON.parse(result.stdout);
  assert.equal(output.verificationState, "blocked_inputs");
  assert.equal(output.liveState, "live_pending");
});

test("operation inputs stay blocked before any network access", async () => {
  const result = await runScript("gpt_image_2_5");
  assert.equal(result.code, 0);
  const output = JSON.parse(result.stdout);
  assert.equal(output.verificationState, "blocked_inputs");
  assert.ok(output.missingInputs.includes("SERVER_URL"));
  assert.ok(output.missingInputs.includes("CODEX_GPT_IMAGE_2_5_REAL_RECEIPT_FILE"));
});

for (const operation of Object.keys(specs)) {
  test(`${operation} accepts only a complete fixture receipt as live_pending`, async () => {
    const { result, seen } = await withRun(operation);
    assert.equal(result.code, 0, result.stderr);
    assert.match(result.stdout, /contract_verified/);
    assert.match(result.stdout, /live_pending/);
    assert.equal(result.stdout.includes("live_verified"), false);
    for (const path of ["/api/accounts", "/api/providers", "/api/shares"]) {
      assert.equal(seen.filter((entry) => entry === path).length, 2);
    }
    assert.equal(
      seen.filter((entry) => entry === "/v1/models").length,
      operation === "ws_prewarm" || operation === "gpt_6_astra" ? 4 : 0,
    );
  });
}

test("Codex operations reject ambiguous or inactive Shares before model requests", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const [name, serverOptions] of [
      ["duplicate", { duplicateShareBinding: true }],
      ["primary_mismatch", { primaryMismatch: true }],
      ["disabled", { shareState: "disabled" }],
      ["paused", { shareState: "paused" }],
    ]) {
      await t.test(`${operation}/${name}`, async () => {
        const { result, seen } = await withRun(operation, { serverOptions });
        assert.equal(result.code, 1, result.stdout);
        assert.equal(result.stderr.trim(), "[FAIL] Codex acceptance failed (details redacted)");
        assert.equal(seen.filter((entry) => entry === "/v1/models").length, 0);
      });
    }
  }
});

test("Codex Astra and WS prewarm reject final catalog or Account manifest drift", async (t) => {
  for (const operation of ["gpt_6_astra", "ws_prewarm"]) {
    for (const fault of [
      "catalog_remove", "catalog_add", "catalog_capability", "manifest_remove", "manifest_add", "manifest_capability",
    ]) {
      await t.test(`${operation}/${fault}`, async () => {
        const { result, seen } = await withRun(operation, {
          serverOptions: { finalModelFault: fault },
        });
        assert.equal(result.code, 1, `${result.stdout}\n${result.stderr}`);
        assert.equal(result.stderr.trim(), "[FAIL] Codex acceptance failed (details redacted)");
        assert.equal(seen.filter((entry) => entry === "/api/shares").length, 2);
      });
    }
  }
});

test("Codex Astra and WS prewarm reject invalid IDs in the full catalog or manifest", async (t) => {
  for (const operation of ["gpt_6_astra", "ws_prewarm"]) {
    for (const source of ["catalog", "manifest"]) {
      for (const kind of ["missing", "blank", "duplicate", "nonstring"]) {
        await t.test(`${operation}/${source}/${kind}`, async () => {
          const invalid = kind === "missing" ? undefined
            : kind === "blank" ? " "
              : kind === "duplicate" ? "zz-codex-other" : 42;
          const models = [specs[operation].model, "zz-codex-other", invalid];
          const { result } = await withRun(operation, {
            serverOptions: source === "catalog"
              ? { catalogModels: models }
              : { manifestModels: models },
          });
          assert.equal(result.code, 1, `${result.stdout}\n${result.stderr}`);
          assert.equal(result.stderr.trim(), "[FAIL] Codex acceptance failed (details redacted)");
        });
      }
    }
  }
});

test("Codex Astra and WS prewarm preserve pure catalog row reordering", async (t) => {
  for (const operation of ["gpt_6_astra", "ws_prewarm"]) {
    await t.test(operation, async () => {
      const { result, catalogQueries } = await withRun(operation, {
        serverOptions: {
          catalogModels: [specs[operation].model, "zz-codex-other"],
          finalModelFault: "catalog_reorder",
        },
      });
      assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
      assert.equal(catalogQueries.length, 2);
    });
  }
});

test("Codex Astra and WS prewarm allow a pure manifest row reorder", async (t) => {
  for (const operation of ["gpt_6_astra", "ws_prewarm"]) {
    await t.test(operation, async () => {
      const { result, manifestQueries } = await withRun(operation, {
        serverOptions: {
          manifestModels: [specs[operation].model, "other-available-model"],
          finalModelFault: "manifest_reorder",
        },
      });
      assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
      assert.equal(manifestQueries.length, 2);
    });
  }
});

test("Codex operations reject incomplete revision or runtime fingerprint scope before models", async (t) => {
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
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `codex-scope-${operation}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, value);
        const mock = await startMockServer(value, { scopeFault });
        try {
          const result = await runScript(operation, fixtureEnvironment(value, mock.origin, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Codex acceptance failed (details redacted)");
          assert.equal(mock.seen.filter((entry) => entry === "/v1/models").length, 0);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Codex operations reject legacy top-level revision masking real Provider scope", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const scopeFault of [
      "legacy_top_level_only",
      "missing_view_revision",
      "missing_runtime_revision",
      "mismatched_revisions",
    ]) {
      await t.test(`${operation}/${scopeFault}`, async () => {
        const value = fixture(operation);
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `codex-real-scope-${operation}-`));
        const receiptFile = path.join(directory, "receipt.json");
        writeReceipt(receiptFile, value);
        const mock = await startMockServer(value, { scopeFault });
        try {
          const result = await runScript(operation, fixtureEnvironment(value, mock.origin, receiptFile));
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Codex acceptance failed (details redacted)");
          assert.equal(mock.seen.filter((entry) => entry === "/v1/models").length, 0);
        } finally {
          await mock.close();
          fs.rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

test("Codex OAuth operations require explicit false Account booleans", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const field of ["hasApiKey", "needsRelogin"]) {
      for (const variant of ["missing", "null", "string"]) {
        await t.test(`${operation}/${field}/${variant}`, async () => {
          const { result, seen } = await withRun(operation, {
            serverOptions: { accountFault: { field, variant } },
          });
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Codex acceptance failed (details redacted)");
          assert.equal(seen.filter((entry) => entry === "/api/providers").length, 0);
          assert.equal(seen.filter((entry) => entry === "/v1/models").length, 0);
        });
      }
    }
  }
});

test("Codex model and WS receipts require the bound Account manifest, not just the public model list", async (t) => {
  for (const operation of ["gpt_6_astra", "ws_prewarm"]) {
    for (const [caseName, manifestModels] of [
      ["missing", ["different-account-model"]],
      ["duplicate", [specs[operation].model, specs[operation].model]],
    ]) {
      await t.test(`${operation}/${caseName}`, async () => {
        const { result, seen } = await withRun(operation, {
          serverOptions: { manifestModels },
        });
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /details redacted/);
        assert.equal(seen.filter((entry) => entry === "/v1/models").length, 2);
      });
    }
  }
});

test("Codex model and WS manifest requests declare the selected Share and Provider", async (t) => {
  for (const operation of ["gpt_6_astra", "ws_prewarm"]) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      const { result, manifestQueries } = await withRun(operation);
      assert.equal(result.code, 0, result.stderr);
      assert.equal(manifestQueries.length, 2);
      for (const query of manifestQueries) {
        assert.equal(query.get("expectedShareId"), value.shareId);
        assert.equal(query.get("expectedProviderId"), value.providerId);
      }
    });
  }
});

test("Codex model and WS catalog requests declare the selected Share and Provider", async (t) => {
  for (const operation of ["gpt_6_astra", "ws_prewarm"]) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      const { result, catalogQueries } = await withRun(operation);
      assert.equal(result.code, 0, result.stderr);
      assert.equal(catalogQueries.length, 2);
      for (const query of catalogQueries) {
        assert.equal(query.get("app"), "codex");
        assert.equal(query.get("providerId"), value.providerId);
        assert.equal(query.get("expectedShareId"), value.shareId);
        assert.equal(query.get("expectedProviderId"), value.providerId);
      }
    });
  }
});

test("image operations reject a non-canonical exact model before network access", async () => {
  const value = fixture("gpt_image_2_5_flare");
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "codex-model-test-"));
  const receiptFile = path.join(directory, "receipt.json");
  writeReceipt(receiptFile, value);
  try {
    const result = await runScript(
      value.operation,
      fixtureEnvironment(value, "http://127.0.0.1:9", receiptFile, {
        [value.spec.modelEnv]: "gpt-image-2.5",
      }),
    );
    assert.equal(result.code, 1);
    assert.match(result.stderr, /details redacted/);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("Provider generation mismatch fails closed", async () => {
  const value = fixture("gpt_image_2_5");
  const { result } = await withRun(value.operation, {
    serverOptions: { providerGeneration: value.authIdentityGeneration + 1 },
  });
  assert.equal(result.code, 1);
  assert.match(result.stderr, /details redacted/);
});

test("a missing operation check cannot be promoted", async () => {
  const value = fixture("gpt_image_2_5_sunburst");
  const original = receipt(value);
  const checks = { ...original.checks };
  delete checks.cloudflare_stream_flush;
  const { result } = await withRun(value.operation, {
    receiptOverrides: { checks },
  });
  assert.equal(result.code, 1);
  assert.equal(result.stdout.includes("live_verified"), false);
});

test("WS benchmark measurements require benefit, samples, and reuse", async () => {
  const { result } = await withRun("ws_prewarm", {
    receiptOverrides: {
      measurements: {
        benchmarkSamples: 4,
        coldTtfbP50Ms: 120,
        prewarmedTtfbP50Ms: 140,
        upstreamConnections: 2,
        turns: 2,
      },
    },
  });
  assert.equal(result.code, 1);
  assert.equal(result.stdout.includes("live_verified"), false);
});

test("fixture mode rejects a forged live_verified state", async () => {
  const { result } = await withRun("gpt_image_2_5", {
    receiptOverrides: {
      verificationState: "live_verified",
      liveState: "live_verified",
    },
  });
  assert.equal(result.code, 1);
  assert.equal(result.stdout.includes("live_verified"), false);
});

test("control-plane failures never print tokens, prompts, or raw bodies", async () => {
  const rawFailure = `private-prompt:${serverSecret}:${routerSecret}:raw-upstream-body`;
  const { result } = await withRun("gpt_image_2_5", {
    serverOptions: { providerFailure: rawFailure },
  });
  const output = `${result.stdout}\n${result.stderr}`;
  assert.equal(result.code, 1);
  assert.equal(output.includes(serverSecret), false);
  assert.equal(output.includes(routerSecret), false);
  assert.equal(output.includes("private-prompt"), false);
  assert.equal(output.includes("raw-upstream-body"), false);
  assert.match(output, /details redacted/);
});

test("Codex private receipt cannot be hard-linked from inside the repository", async () => {
  const operation = "gpt_6_astra";
  const value = fixture(operation);
  const insideDirectory = fs.mkdtempSync(path.join(repoRoot, ".codex-receipt-hardlink-"));
  const outsideDirectory = fs.mkdtempSync(path.join(path.dirname(repoRoot), ".codex-receipt-outside-"));
  const insideReceipt = path.join(insideDirectory, "receipt.json");
  const outsideReceipt = path.join(outsideDirectory, "receipt.json");
  writeReceipt(insideReceipt, value);
  fs.linkSync(insideReceipt, outsideReceipt);
  const mock = await startMockServer(value);
  try {
    const result = await runScript(
      operation,
      fixtureEnvironment(value, mock.origin, outsideReceipt),
    );
    assert.equal(result.code, 1, result.stdout);
    assert.match(result.stderr, /details redacted/);
    assert.deepEqual(mock.seen, []);
  } finally {
    await mock.close();
    fs.rmSync(outsideDirectory, { recursive: true, force: true });
    fs.rmSync(insideDirectory, { recursive: true, force: true });
  }
});

test("Codex receipt cannot be replaced during control-plane validation", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `codex-receipt-swap-${operation}-`));
      const receiptFile = path.join(directory, "receipt.json");
      const replacementFile = path.join(directory, "replacement.json");
      writeReceipt(receiptFile, value, { model: "not-the-selected-model" });
      writeReceipt(replacementFile, value);
      const originalInode = fs.statSync(receiptFile).ino;
      const mock = await startMockServer(value, {
        onFirstAccount: () => fs.renameSync(replacementFile, receiptFile),
      });
      try {
        const result = await runScript(
          operation,
          fixtureEnvironment(value, mock.origin, receiptFile),
        );
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /details redacted/);
        assert.ok(mock.seen.includes("/api/accounts"));
        assert.notEqual(fs.statSync(receiptFile).ino, originalInode);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Codex private receipt must remain the same file through final acceptance", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const change of ["replacement", "in_place"]) {
      await t.test(`${operation}/${change}`, async () => {
        const value = fixture(operation);
        const directory = fs.mkdtempSync(path.join(os.tmpdir(), `codex-receipt-final-${operation}-`));
        const receiptFile = path.join(directory, "receipt.json");
        const replacementFile = path.join(directory, "replacement.json");
        writeReceipt(receiptFile, value);
        if (change === "replacement") {
          fs.writeFileSync(replacementFile, "not the accepted receipt\n", { mode: 0o600 });
        }
        const originalInode = fs.statSync(receiptFile).ino;
        const mock = await startMockServer(value, {
          onFirstAccount: () => {
            if (change === "replacement") {
              fs.renameSync(replacementFile, receiptFile);
            } else {
              fs.writeFileSync(receiptFile, " ".repeat(fs.statSync(receiptFile).size), { flag: "r+" });
              fs.utimesSync(receiptFile, new Date(0), new Date(0));
            }
          },
        });
        try {
          const result = await runScript(
            operation,
            fixtureEnvironment(value, mock.origin, receiptFile),
          );
          assert.equal(result.code, 1, result.stdout);
          assert.equal(result.stderr.trim(), "[FAIL] Codex acceptance failed (details redacted)");
          assert.ok(mock.seen.includes("/api/accounts"));
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

test("Codex final acceptance rejects changed bytes despite unchanged reported metadata", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `codex-receipt-bytes-${operation}-`));
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
        onFirstAccount: () => {
          fs.writeFileSync(receiptFile, " ".repeat(fs.statSync(receiptFile).size), { flag: "r+" });
        },
      });
      try {
        const result = await runScript(operation, fixtureEnvironment(value, mock.origin, receiptFile, {
          NODE_OPTIONS: `--import=data:text/javascript,${encodeURIComponent(preload)}`,
        }));
        assert.equal(result.code, 1, result.stdout);
        assert.equal(result.stderr.trim(), "[FAIL] Codex acceptance failed (details redacted)");
        assert.ok(mock.seen.includes("/api/accounts"));
        assert.match(fs.readFileSync(receiptFile, "utf8"), /^ +$/);
      } finally {
        await mock.close();
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

test("Codex receipt rejects an in-place rewrite during its snapshot", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const value = fixture(operation);
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), `codex-receipt-inplace-${operation}-`));
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
          ...fixtureEnvironment(value, mock.origin, receiptFile),
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

test("Codex receipt gate rejects oversized control-plane bodies before waiting for EOF", async (t) => {
  for (const oversizedAccounts of ["declared", "chunked"]) {
    await t.test(oversizedAccounts, async () => {
      const startedAt = Date.now();
      const { result, seen } = await withRun("gpt_6_astra", {
        serverOptions: { oversizedAccounts },
      });
      assert.equal(result.code, 1, result.stdout);
      assert.match(result.stderr, /details redacted/);
      assert.deepEqual(seen, ["/api/accounts"]);
      assert.ok(Date.now() - startedAt < 3_000, "oversized response waited for timeout");
    });
  }
});

test("Codex receipt gate rejects binding drift before acceptance for every operation", async (t) => {
  for (const operation of Object.keys(specs)) {
    for (const drift of ["account", "provider", "share"]) {
      await t.test(`${operation}/${drift}`, async () => {
        const { result } = await withRun(operation, { serverOptions: { drift } });
        assert.equal(result.code, 1, result.stdout);
        assert.match(result.stderr, /details redacted/);
      });
    }
  }
});

test("Codex operation receipts expiring during final binding validation are rejected", async (t) => {
  for (const operation of Object.keys(specs)) {
    await t.test(operation, async () => {
      const { result, seen } = await withRun(operation, {
        receiptOverrides: {
          recordedAt: new Date(Date.now() - 24 * 60 * 60 * 1_000 + 1_200).toISOString(),
        },
        serverOptions: { finalShareDelayMs: 2_000 },
      });
      assert.equal(result.code, 1, result.stdout);
      assert.match(result.stderr, /details redacted/);
      assert.equal(seen.filter((entry) => entry === "/api/shares").length, 2);
    });
  }
});
