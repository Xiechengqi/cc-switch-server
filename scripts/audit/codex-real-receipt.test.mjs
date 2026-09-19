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

const specs = Object.freeze({
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
  const operationContract = contract.realAcceptance.operations.find(
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
  { providerGeneration = value.authIdentityGeneration, providerFailure = "" } = {},
) {
  const seen = [];
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    seen.push(url.pathname);
    if (request.method === "GET" && url.pathname === "/api/accounts") {
      sendJson(response, 200, {
        ok: true,
        accounts: [
          {
            id: value.accountId,
            email: `${value.operation}@example.test`,
            providerType: "codex_oauth",
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
            providerRevision: value.providerRevision,
            provider: { id: value.providerId },
            runtime: {
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
      sendJson(response, 200, {
        ok: true,
        shares: [
          {
            id: value.shareId,
            configRevision: value.shareRevision,
            bindings: [
              {
                app: "codex",
                providerId: value.providerId,
                providerType: "codex_oauth",
              },
            ],
          },
        ],
      });
      return;
    }
    if (request.method === "GET" && url.pathname === "/v1/models") {
      sendJson(response, 200, { object: "list", data: [{ id: value.spec.model }] });
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
    return { result, value, seen: mock.seen };
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
    assert.ok(seen.includes("/api/accounts"));
    assert.ok(seen.includes("/api/providers"));
    assert.ok(seen.includes("/api/shares"));
    assert.equal(seen.includes("/v1/models"), operation === "ws_prewarm");
  });
}

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
