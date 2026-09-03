#!/usr/bin/env node

import fs from "node:fs";
import crypto from "node:crypto";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
export const oraclePath = path.join(
  repoRoot,
  "assets/contract/qoder-cli-oracle.json",
);

const expectedSource = Object.freeze({
  commit: "9b18f2de06c53f12bf2c5112c7a71e3e64755b97",
  files: Object.freeze({
    "worker/src/compat.mjs":
      "6c004d1bfb88cd2c1522588725d651be5de48e9c148c5c9f0c263c35b77b8dc8",
    "worker/src/daemon.mjs":
      "86dc7b9f77bf8ada68ec1e7ba0d744e09bfdcd0d7d9e7cf3f8fd35df0ed9064c",
    "worker/src/plaintext.mjs":
      "1f25273ac8b8b7b156ea70945bcbedcca38007cc5fea55d7360fa2d4ca85a413",
  }),
});

const expectedPackages = Object.freeze({
  global: Object.freeze({
    name: "@qoder-ai/qodercli",
    version: "1.1.32",
    npmIntegrity:
      "sha512-wzFOvUC8mP1TKfmcoP5pFQ5X2L5zNbqIVr0X8313cJt9u0iGbFZBeh+NTGc1nKljL6EOagHLt4PvDWNyddgx5w==",
    npmShasum: "04620839405c5a3f05915f03925bc0949688e66c",
    bundleSha256:
      "24de5b12520cbe49c0027b53654eaee02bddd857e3d9f19a6198824e365d89bf",
  }),
  cn: Object.freeze({
    name: "@qodercn-ai/qoderclicn",
    version: "1.1.32",
    npmIntegrity:
      "sha512-xLt0mlunz6KhEqMJ/UQN97vQvfpzqaRi/B4vGWdYtg6AUrQ4LnqMZSAEuSZbU/2eV4ETQvzp4e/WENFpwdYJ4A==",
    npmShasum: "4be70c9095247cb96e59b6a52b51f5096bf61bc9",
    bundleSha256:
      "5a82eeffbeb015d78c4945b7f4ed989494d2ea8cc7fdf2dbfc6ad04c17418f8b",
  }),
});

const expectedRailIdentity = Object.freeze({
  global_oauth: Object.freeze({ site: "global", credentialRail: "global_oauth" }),
  global_pat: Object.freeze({ site: "global", credentialRail: "pat_job_token" }),
  cn_oauth: Object.freeze({ site: "cn", credentialRail: "cn_oauth" }),
});

const expectedOauthLifecycle = Object.freeze({
  authorizationPath: "/device/selectAccounts",
  authorizationQueryRequired: Object.freeze([
    "challenge",
    "challenge_method",
    "client_id",
    "machine_id",
    "nonce",
  ]),
  nonceFormat: "uuid_v4",
  pollForbiddenHeaders: Object.freeze(["authorization", "cosy-*", "user-agent"]),
  pollHeaders: Object.freeze({ accept: "application/json" }),
  pollIntervalSeconds: 1,
  pollMethod: "GET",
  pollOrigin: "openapi",
  pollPath: "/api/v1/deviceToken/poll",
  pollPendingHttpStatus: 404,
  pollQueryRequired: Object.freeze(["challenge_method", "nonce", "verifier"]),
  pollTimeoutSeconds: 300,
  pkceMethod: "S256",
  refreshForbiddenHeaders: Object.freeze([
    "authorization",
    "cosy-*",
    "proxy-authorization",
    "x-qoder-account",
  ]),
  refreshHeaders: Object.freeze({
    accept: "application/json",
    "content-type": "application/json",
    "user-agent": "qoder/1.1.32",
  }),
  refreshMethod: "POST",
  refreshOrigin: "openapi",
  refreshPath: "/api/v1/deviceToken/refresh",
  refreshRequestBody: Object.freeze({ refresh_token: "<redacted>" }),
  refreshResponseOptionalFields: Object.freeze(["refresh_token_expires_at"]),
  refreshResponseRequiredFields: Object.freeze([
    "device_token",
    "expires_at",
    "refresh_token",
  ]),
  refreshTokenRotates: true,
  stateBound: true,
  userinfoPath: "/api/v1/userinfo",
});

const requiredHooks = Object.freeze([
  "createWasmContext",
  "modelCatalog",
  "prepareInferRequest",
  "quotaApi",
  "skipMain",
]);

const expectedVerification = Object.freeze({
  rustQoderTests: 63,
  nodeOracleMutationTests: 9,
  nodeRealHarnessFixtureTests: 7,
});

// These independent digests are deliberate mutation tripwires. The fixture keeps
// the reviewable JSON values; this script prevents a fixture, both projections,
// and accepted-difference reasons from being changed coherently in one edit.
const expectedRailSectionDigests = Object.freeze({
  global_oauth: Object.freeze({
    origins: "1a1b145ee15405bf1e2ee2eff68559b566fdf9e8cfd3428024cee7034992c7fb",
    wire: "b326774acd02ee708ef9af312cf6144002cb80e5161ac8c65b3be0e13266a0ac",
    catalog: "a830dcd8e1e7384c6d20e907d82c642390b8642036a6112cb3bfbac8e3e587a3",
    quota: "13ce613daeb8e33b9e56e1f18f6c3bbe19317d5a8151168c32742eccae22eb3b",
  }),
  global_pat: Object.freeze({
    origins: "1a1b145ee15405bf1e2ee2eff68559b566fdf9e8cfd3428024cee7034992c7fb",
    wire: "b326774acd02ee708ef9af312cf6144002cb80e5161ac8c65b3be0e13266a0ac",
    catalog: "a830dcd8e1e7384c6d20e907d82c642390b8642036a6112cb3bfbac8e3e587a3",
    quota: "13ce613daeb8e33b9e56e1f18f6c3bbe19317d5a8151168c32742eccae22eb3b",
  }),
  cn_oauth: Object.freeze({
    origins: "c8ed37a3113b1d0e63d32f2fbfba37c64a204bcb4c837e75a75d386bcf698e33",
    wire: "0acc32e678c076d0f2fc41475e6c455019529578190f7f019882c61c2c9e01e5",
    catalog: "a830dcd8e1e7384c6d20e907d82c642390b8642036a6112cb3bfbac8e3e587a3",
    quota: "e060f983cbc872be946661d19189435190b57ea4142f2c49d4da80272a93a1c4",
  }),
});

const expectedCanonicalDigests = Object.freeze({
  input: "ac958885d887c3e9d024e173e52779815886a1bf0facbf9af40749fe6ff7806a",
  cli2apiProjection:
    "6fc28e2ad93a8d671561d1d52aa3fe13d6cffe2fda93f4e640859126fcf3908a",
  serverProjection:
    "77691d27bfd5249228f5434689466ba3c2ba1a6d9f1f1d94ae419da69c437856",
  acceptedDifferences:
    "fdaa646c23a703cdacb6416723875f271aae2499106f7d029ae1a9b287b0897a",
  randomFieldSchema:
    "b2a30c3e0d876cf021a24214aa90388bb9fc648722955dd856213bc927abfb2b",
  normalizedServerBody:
    "fbdd08c108db2dda1745264ab253e9be21ed30437cdaea4aa6c4207f80d2ae7f",
});

const expectedVectorDigests = Object.freeze({
  encodingVectors:
    "e841d840067e0d1a53b957aa9f8a8a94cfc20b883428f3f42b9019fe15387697",
  signatureVector:
    "03e3bc3def69e5842e8374bff0f03c5903c4c342951cf083fcf03fa8953c69cf",
});

const expectedCompatibilityPolicyDigest =
  "a65172223d49ed4f60a99d43f4fcd1f38b7ee23cfcda3be39c6be527c27e327f";

function fail(message) {
  throw new Error(`Qoder CLI oracle contract: ${message}`);
}

function assert(condition, message) {
  if (!condition) fail(message);
}

function stable(value) {
  return JSON.stringify(value, Object.keys(value || {}).sort());
}

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

function canonicalDigest(value) {
  return crypto
    .createHash("sha256")
    .update(JSON.stringify(canonical(value)))
    .digest("hex");
}

function assertFrozenDigest(actual, expected, label) {
  assert(
    canonicalDigest(actual) === expected,
    `${label} drifted from its independently frozen digest`,
  );
}

function assertExact(actual, expected, label) {
  assert(
    JSON.stringify(canonical(actual)) === JSON.stringify(canonical(expected)),
    `${label} drifted from the frozen CLI contract`,
  );
}

function uniqueSortedStrings(value, label, { allowEmpty = false } = {}) {
  assert(Array.isArray(value), `${label} must be an array`);
  const strings = value.map((item, index) => {
    assert(typeof item === "string", `${label}[${index}] must be a string`);
    const text = item.trim();
    assert(allowEmpty || text.length > 0, `${label}[${index}] must not be empty`);
    return text;
  });
  assert(new Set(strings).size === strings.length, `${label} contains duplicates`);
  assert(
    strings.join("\n") === [...strings].sort().join("\n"),
    `${label} must stay sorted for reviewable diffs`,
  );
  return strings;
}

function assertDigest(value, length, label) {
  assert(
    typeof value === "string" && new RegExp(`^[a-f0-9]{${length}}$`).test(value),
    `${label} must be a ${length}-character lowercase hex digest`,
  );
}

function assertFixedOrigin(value, label) {
  let parsed;
  try {
    parsed = new URL(value);
  } catch {
    fail(`${label} must be a URL`);
  }
  assert(parsed.protocol === "https:", `${label} must use HTTPS`);
  assert(!parsed.username && !parsed.password, `${label} must not contain credentials`);
  assert(parsed.pathname === "/", `${label} must be an origin without a path`);
  assert(!parsed.search && !parsed.hash, `${label} must not contain query or fragment`);
}

function assertPath(value, label, { queryAllowed = true } = {}) {
  assert(typeof value === "string" && value.startsWith("/"), `${label} must be absolute`);
  assert(!value.startsWith("//"), `${label} must not be protocol-relative`);
  assert(!value.includes(".."), `${label} must not contain traversal`);
  assert(!/^\/https?:/i.test(value), `${label} must not embed an origin`);
  if (!queryAllowed) assert(!value.includes("?"), `${label} must not contain a query`);
}

function assertNoExternalDependency(document) {
  const serialized = JSON.stringify(document);
  for (const forbidden of [
    "/data/projects/",
    "file://",
    "../cli2api",
    "node_modules/@qoder",
    "QODERCLI_JS",
    "QODERCNCLI_JS",
  ]) {
    assert(!serialized.includes(forbidden), `must not depend on external path ${forbidden}`);
  }
}

function assertNoSensitiveMaterial(document) {
  const serialized = JSON.stringify(document);
  const forbiddenPatterns = [
    [/Bearer\s+[A-Za-z0-9._~+\/-]{12,}/i, "bearer credential"],
    [/\beyJ[A-Za-z0-9_-]{12,}\.[A-Za-z0-9_-]{12,}\.[A-Za-z0-9_-]{8,}\b/, "JWT"],
    [/\bpt-[A-Za-z0-9_-]{8,}\b/, "Qoder PAT"],
    [/"(?:access|refresh|id)_token"\s*:\s*"(?!<redacted>)[^"]+"/i, "OAuth token"],
  ];
  for (const [pattern, label] of forbiddenPatterns) {
    assert(!pattern.test(serialized), `contains ${label}`);
  }
  assert(
    document.signatureVector?.cosyKey === "cosy-key",
    "signature vector must use the reviewed synthetic cosy key",
  );
}

function auditSource(source) {
  assert(source && typeof source === "object", "source is required");
  const reference = source.referenceRepository;
  assert(reference?.name === "cli2api", "reference repository name drifted");
  assert(reference.commit === expectedSource.commit, "reference commit drifted");
  assert(
    reference.role === "read_only_capture_and_plaintext_projection",
    "reference role must stay read-only",
  );
  assert(
    stable(reference.files) === stable(expectedSource.files),
    "reference file digests drifted without review",
  );
  for (const [name, digest] of Object.entries(reference.files)) {
    assert(!path.isAbsolute(name) && !name.includes(".."), `reference file ${name} is unsafe`);
    assertDigest(digest, 64, `reference file ${name}`);
  }

  assert(Array.isArray(source.packages) && source.packages.length === 2, "two CLI packages are required");
  const sites = new Set();
  for (const pkg of source.packages) {
    assert(pkg && expectedPackages[pkg.site], `unexpected package site ${pkg?.site}`);
    assert(!sites.has(pkg.site), `duplicate package site ${pkg.site}`);
    sites.add(pkg.site);
    const expected = expectedPackages[pkg.site];
    for (const field of ["name", "version", "npmIntegrity", "npmShasum", "bundleSha256"]) {
      assert(pkg[field] === expected[field], `${pkg.site} package ${field} drifted`);
    }
    assertDigest(pkg.npmShasum, 40, `${pkg.site} npm shasum`);
    assertDigest(pkg.bundleSha256, 64, `${pkg.site} bundle SHA-256`);
    const hooks = uniqueSortedStrings(pkg.captureHooks, `${pkg.site}.captureHooks`);
    assert(hooks.join("\n") === requiredHooks.join("\n"), `${pkg.site} hook set drifted`);
  }
}

function auditRails(rails) {
  assert(Array.isArray(rails) && rails.length === 3, "exactly three credential rails are required");
  assert(
    !JSON.stringify(rails).includes("/algo/api/v3/user/jobToken"),
    "legacy center refresh path must not reappear",
  );
  const seen = new Set();
  for (const rail of rails) {
    const expected = expectedRailIdentity[rail?.id];
    assert(expected, `unexpected rail ${rail?.id}`);
    assert(!seen.has(rail.id), `duplicate rail ${rail.id}`);
    seen.add(rail.id);
    assert(rail.site === expected.site, `${rail.id} site drifted`);
    assert(rail.credentialRail === expected.credentialRail, `${rail.id} credential rail drifted`);
    assert(rail.packageSite === rail.site, `${rail.id} package site must match account site`);
    for (const section of ["origins", "wire", "catalog", "quota"]) {
      assertFrozenDigest(
        rail[section],
        expectedRailSectionDigests[rail.id][section],
        `${rail.id}.${section}`,
      );
    }
    assert(rail.origins && typeof rail.origins === "object", `${rail.id} origins are required`);
    for (const [name, origin] of Object.entries(rail.origins)) {
      assertFixedOrigin(origin, `${rail.id}.origins.${name}`);
    }
    assert(rail.login && typeof rail.login === "object", `${rail.id} login contract is required`);
    for (const [name, value] of Object.entries(rail.login)) {
      if (name.endsWith("Path")) assertPath(value, `${rail.id}.login.${name}`);
    }
    if (rail.id === "global_pat") {
      assertExact(
        rail.login,
        {
          patExchangePath: "/api/v1/jobToken/exchange",
          patPrefixSchema: "pt-<redacted>",
          persistentJobToken: false,
        },
        "global_pat.login",
      );
      assert(rail.login.persistentJobToken === false, "PAT job token must stay transient");
      assert(rail.login.patPrefixSchema === "pt-<redacted>", "PAT schema must stay redacted");
    } else {
      const expectedLogin = {
        ...expectedOauthLifecycle,
        machineIdFormat: rail.site === "global" ? "lower_hex_36" : "uuid_v4",
        ...(rail.site === "cn"
          ? { authStatusPath: "/algo/api/v3/user/status?Encode=1" }
          : {}),
      };
      assertExact(rail.login, expectedLogin, `${rail.id}.login`);
      uniqueSortedStrings(
        rail.login.authorizationQueryRequired,
        `${rail.id}.login.authorizationQueryRequired`,
      );
      uniqueSortedStrings(
        rail.login.pollForbiddenHeaders,
        `${rail.id}.login.pollForbiddenHeaders`,
      );
      uniqueSortedStrings(
        rail.login.pollQueryRequired,
        `${rail.id}.login.pollQueryRequired`,
      );
      uniqueSortedStrings(
        rail.login.refreshForbiddenHeaders,
        `${rail.id}.login.refreshForbiddenHeaders`,
      );
      uniqueSortedStrings(
        rail.login.refreshResponseOptionalFields,
        `${rail.id}.login.refreshResponseOptionalFields`,
      );
      uniqueSortedStrings(
        rail.login.refreshResponseRequiredFields,
        `${rail.id}.login.refreshResponseRequiredFields`,
      );
    }

    const wire = rail.wire;
    assert(wire && typeof wire === "object", `${rail.id} wire contract is required`);
    assertPath(wire.actualPath, `${rail.id}.wire.actualPath`);
    assertPath(wire.signaturePath, `${rail.id}.wire.signaturePath`, { queryAllowed: false });
    assert(wire.actualPath !== wire.signaturePath, `${rail.id} actual and signature paths must remain distinct`);
    assert(wire.clientVersion === "1.24.2", `${rail.id} COSY client version drifted`);
    const required = uniqueSortedStrings(wire.requiredHeaders, `${rail.id}.requiredHeaders`);
    const forbidden = uniqueSortedStrings(wire.forbiddenHeaders, `${rail.id}.forbiddenHeaders`);
    assert(required.includes("authorization"), `${rail.id} must require authorization`);
    assert(required.includes("cosy-sigpath"), `${rail.id} must require cosy-sigpath`);
    assert(forbidden.includes("x-qoder-account"), `${rail.id} must reject routing identity upstream`);
    assert(!required.some((name) => forbidden.includes(name)), `${rail.id} header sets overlap`);
    if (rail.site === "global") {
      assert(wire.clientType === "5" && wire.dataPolicy === "disagree", `${rail.id} Global profile drifted`);
      assert(required.includes("cosy-scene"), `${rail.id} Global scene header is required`);
      assert(forbidden.includes("cosy-machinecode"), `${rail.id} Global machinecode must remain absent`);
    } else {
      assert(wire.clientType === "0" && wire.dataPolicy === "DISAGREE", `${rail.id} CN profile drifted`);
      assert(required.includes("cosy-machinecode"), `${rail.id} CN machinecode header is required`);
      assert(forbidden.includes("cosy-scene"), `${rail.id} CN scene header must remain absent`);
    }
    for (const section of ["catalog", "quota"]) {
      assertPath(rail[section]?.actualPath, `${rail.id}.${section}.actualPath`);
      assertPath(rail[section]?.signaturePath, `${rail.id}.${section}.signaturePath`, {
        queryAllowed: false,
      });
    }
  }
  assert(
    [...seen].sort().join("\n") === Object.keys(expectedRailIdentity).sort().join("\n"),
    "credential rail set is incomplete",
  );
}

function auditDifferential(canonicalCase) {
  assert(canonicalCase && typeof canonicalCase === "object", "canonical case is required");
  const canonicalInput = {
    modelKey: canonicalCase.modelKey,
    siteCases: canonicalCase.siteCases,
    sessionId: canonicalCase.sessionId,
    userType: canonicalCase.userType,
    nowMs: canonicalCase.nowMs,
    request: canonicalCase.request,
    modelConfig: canonicalCase.modelConfig,
  };
  assertFrozenDigest(canonicalInput, expectedCanonicalDigests.input, "canonical input");
  assert(
    canonicalCase.siteCases?.join("\n") === "global\ncn",
    "canonical case must independently cover Global and CN",
  );
  const oracle = canonicalCase.cli2apiProjection;
  const server = canonicalCase.serverProjection;
  assert(oracle && server, "both differential projections are required");
  assertFrozenDigest(
    oracle,
    expectedCanonicalDigests.cli2apiProjection,
    "cli2api projection",
  );
  assertFrozenDigest(
    server,
    expectedCanonicalDigests.serverProjection,
    "server projection",
  );
  const pointers = [...new Set([...Object.keys(oracle), ...Object.keys(server)])].sort();
  const differences = pointers.filter(
    (pointer) => JSON.stringify(oracle[pointer]) !== JSON.stringify(server[pointer]),
  );
  const accepted = canonicalCase.acceptedDifferences;
  assert(Array.isArray(accepted), "acceptedDifferences must be an array");
  assertFrozenDigest(
    accepted,
    expectedCanonicalDigests.acceptedDifferences,
    "accepted differences",
  );
  const acceptedPointers = accepted.map((entry, index) => {
    assert(entry && typeof entry.pointer === "string", `acceptedDifferences[${index}] needs a pointer`);
    assert(
      typeof entry.reason === "string" && entry.reason.trim().length >= 24,
      `acceptedDifferences[${index}] needs a reviewable reason`,
    );
    return entry.pointer;
  });
  assert(new Set(acceptedPointers).size === acceptedPointers.length, "accepted differences contain duplicates");
  assert(
    [...acceptedPointers].sort().join("\n") === differences.join("\n"),
    `unexplained projection drift: expected ${differences.join(", ")}`,
  );
  const randomFields = canonicalCase.randomFieldSchema;
  assert(randomFields && typeof randomFields === "object", "random field schema is required");
  assertFrozenDigest(
    randomFields,
    expectedCanonicalDigests.randomFieldSchema,
    "random field schema",
  );
  assert(
    randomFields["/chat_record_id"] === "same_as_request_id",
    "chat record must remain bound to request id",
  );
  assertFrozenDigest(
    canonicalCase.normalizedServerBody,
    expectedCanonicalDigests.normalizedServerBody,
    "normalized full server body",
  );
  assert(
    canonicalCase.normalizedServerBody?.request_id === "<request_id_uuid_v4>" &&
      canonicalCase.normalizedServerBody?.chat_record_id === "<request_id_uuid_v4>" &&
      canonicalCase.normalizedServerBody?.request_set_id === "<request_set_id_uuid_v4>" &&
      canonicalCase.normalizedServerBody?.business?.id === "<business_id_uuid_v4>",
    "normalized full server body must preserve the reviewed random-field placeholders",
  );
}

function auditReceipt(receipt) {
  assert(receipt && typeof receipt === "object", "receipt schema is required");
  const required = uniqueSortedStrings(receipt.requiredFields, "receipt.requiredFields");
  const forbidden = uniqueSortedStrings(receipt.forbiddenFields, "receipt.forbiddenFields");
  const zero = uniqueSortedStrings(receipt.successRequiresZero, "receipt.successRequiresZero");
  for (const field of [
    "site",
    "credentialRail",
    "authIdentityGeneration",
    "tokenRefreshGeneration",
    "pathHeaderSchemaDigest",
    "surfaceChecks",
    "terminalChecks",
    "otherAccountRequests",
    "otherSiteRequests",
    "otherProviderRequests",
    "sensitiveScan",
  ]) {
    assert(required.includes(field), `receipt is missing required field ${field}`);
  }
  for (const field of zero) assert(required.includes(field), `zero field ${field} must be required`);
  assert(!required.some((field) => forbidden.includes(field)), "receipt required/forbidden fields overlap");
}

function auditQuotaAndTerminalCases(document) {
  assertExact(
    document.quotaCase,
    {
      input: {
        userType: "teams",
        isQuotaExceeded: false,
        userQuota: { total: 100, used: 20, remaining: 80 },
        addOnQuota: { total: 10, used: 2, remaining: 8 },
      },
      expectedBucketIds: ["qoder_add_on", "qoder_user"],
      missingBalanceState: "unknown",
      informationalOnly: true,
    },
    "quotaCase",
  );
  uniqueSortedStrings(document.quotaCase.expectedBucketIds, "quotaCase.expectedBucketIds");
  assertExact(
    document.terminalContract,
    {
      successRequires: [
        "valid_inner_chunk",
        "authoritative_finish_reason",
        "upstream_eof",
        "exactly_one_downstream_done",
      ],
      rejects: [
        "missing_terminal",
        "malformed_json",
        "second_terminal",
        "business_data_after_terminal",
        "auth_error_after_commit",
      ],
    },
    "terminalContract",
  );
}

function auditCompatibilityPolicy(policy) {
  assert(policy && typeof policy === "object", "compatibilityPolicy is required");
  assertFrozenDigest(
    policy,
    expectedCompatibilityPolicyDigest,
    "bounded compatibility policy",
  );
  assert(
    policy.cnClientIp?.trustsDownstreamForwardedHeaders === false,
    "CN client IP must not trust downstream forwarded headers",
  );
  assert(
    policy.toolHistory?.missingResultIds === "infer_only_when_unique",
    "ambiguous tool results must not be guessed",
  );
  assert(
    policy.safety?.crossAccountFallback === false &&
      policy.safety?.crossSiteFallback === false &&
      policy.safety?.strictEofTerminalUnchanged === true,
    "compatibility must preserve the single-account strict-terminal boundary",
  );
  for (const source of policy.provenance || []) {
    assertDigest(source.commit, 40, `${source.name} compatibility commit`);
    for (const [name, digest] of Object.entries(source.files || {})) {
      assert(!path.isAbsolute(name) && !name.includes(".."), `${source.name} file is unsafe`);
      assertDigest(digest, 64, `${source.name} compatibility file ${name}`);
    }
  }
}

export function auditQoderCliOracle(document) {
  assert(document && typeof document === "object" && !Array.isArray(document), "root must be an object");
  assert(document.schemaVersion === 2, "schemaVersion must be 2");
  assert(document.providerType === "qoder_cosy", "providerType must be qoder_cosy");
  assert(document.scope === "single_bound_account_only", "scope must stay single-account only");
  assert(document.offlineState === "fixture_verified", "offline state must be fixture_verified");
  assert(document.liveState === "live_pending", "live state must remain live_pending without receipts");
  assertExact(document.verification, expectedVerification, "verification metadata");
  assertNoExternalDependency(document);
  assertNoSensitiveMaterial(document);
  auditSource(document.source);
  auditRails(document.rails);
  auditDifferential(document.canonicalCase);
  auditReceipt(document.receiptSchema);
  auditQuotaAndTerminalCases(document);
  auditCompatibilityPolicy(document.compatibilityPolicy);
  assert(Array.isArray(document.encodingVectors) && document.encodingVectors.length === 3, "three encoding vectors are required");
  assertFrozenDigest(
    document.encodingVectors,
    expectedVectorDigests.encodingVectors,
    "encoding vectors",
  );
  assertFrozenDigest(
    document.signatureVector,
    expectedVectorDigests.signatureVector,
    "signature vector",
  );
  assert(
    document.signatureVector?.signaturePath ===
      "/api/v2/service/pro/sse/agent_chat_generation",
    "signature vector must use the reviewed logical generation path",
  );
  return {
    rails: document.rails.length,
    packages: document.source.packages.length,
    projectionPointers: new Set([
      ...Object.keys(document.canonicalCase.cli2apiProjection),
      ...Object.keys(document.canonicalCase.serverProjection),
    ]).size,
    acceptedDifferences: document.canonicalCase.acceptedDifferences.length,
  };
}

export function loadQoderCliOracle(file = oraclePath) {
  const bytes = fs.readFileSync(file);
  assert(bytes.length <= 256 * 1024, "fixture exceeds 256 KiB");
  let document;
  try {
    document = JSON.parse(bytes.toString("utf8"));
  } catch (error) {
    fail(`invalid JSON: ${error.message}`);
  }
  return document;
}

function main() {
  const result = auditQoderCliOracle(loadQoderCliOracle());
  console.log(
    `Qoder CLI oracle ok: packages=${result.packages} rails=${result.rails} pointers=${result.projectionPointers} acceptedDifferences=${result.acceptedDifferences} live=live_pending`,
  );
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main();
}
