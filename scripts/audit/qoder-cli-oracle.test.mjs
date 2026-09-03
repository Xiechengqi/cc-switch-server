import assert from "node:assert/strict";
import test from "node:test";

import {
  auditQoderCliOracle,
  loadQoderCliOracle,
} from "./audit-qoder-cli-oracle.mjs";

function fixture() {
  return structuredClone(loadQoderCliOracle());
}

test("frozen Qoder CLI oracle contract is self-contained and reviewable", () => {
  assert.deepEqual(auditQoderCliOracle(fixture()), {
    rails: 3,
    packages: 2,
    projectionPointers: 27,
    acceptedDifferences: 12,
  });

  const inventedCount = fixture();
  inventedCount.verification.rustQoderTests += 1;
  assert.throws(
    () => auditQoderCliOracle(inventedCount),
    /verification metadata drifted/,
  );
});

test("oracle rejects missing rail and cross-site package reuse", () => {
  const missing = fixture();
  missing.rails.pop();
  assert.throws(() => auditQoderCliOracle(missing), /three credential rails/);

  const mixed = fixture();
  mixed.rails.find((rail) => rail.id === "cn_oauth").packageSite = "global";
  assert.throws(() => auditQoderCliOracle(mixed), /package site must match/);

  const extraHeader = fixture();
  const required = extraHeader.rails.find((rail) => rail.id === "global_oauth").wire
    .requiredHeaders;
  required.push("x-invented-header");
  required.sort();
  assert.throws(
    () => auditQoderCliOracle(extraHeader),
    /global_oauth\.wire drifted from its independently frozen digest/,
  );
});

test("oracle rejects unexplained projection drift", () => {
  const document = fixture();
  document.canonicalCase.serverProjection["/stream"] = false;
  assert.throws(() => auditQoderCliOracle(document), /server projection drifted/);

  const coherent = fixture();
  coherent.canonicalCase.modelConfig.source = "coherently-mutated-source";
  coherent.canonicalCase.cli2apiProjection["/model_config/source"] =
    "coherently-mutated-source";
  coherent.canonicalCase.serverProjection["/model_config/source"] =
    "coherently-mutated-source";
  coherent.canonicalCase.normalizedServerBody.model_config.source =
    "coherently-mutated-source";
  coherent.canonicalCase.normalizedServerBody.chat_context.extra.modelConfig.source =
    "coherently-mutated-source";
  assert.throws(
    () => auditQoderCliOracle(coherent),
    /canonical input drifted from its independently frozen digest/,
  );

  const rewrittenReason = fixture();
  rewrittenReason.canonicalCase.acceptedDifferences[0].reason =
    "a newly invented explanation that remains long enough to look reviewable";
  assert.throws(
    () => auditQoderCliOracle(rewrittenReason),
    /accepted differences drifted from its independently frozen digest/,
  );
});

test("oracle rejects external workspace dependencies and credentials", () => {
  const external = fixture();
  external.source.referenceRepository.localPath = "/data/projects/proxy/cli2api";
  assert.throws(() => auditQoderCliOracle(external), /external path/);

  const credential = fixture();
  credential.canonicalCase.request.authorization =
    "Bearer secret-material-that-must-never-enter-the-fixture";
  assert.throws(() => auditQoderCliOracle(credential), /bearer credential/);
});

test("oracle cannot claim live verification without independent receipts", () => {
  const document = fixture();
  document.liveState = "live_verified";
  assert.throws(() => auditQoderCliOracle(document), /live_pending/);
});

test("oracle rejects legacy refresh endpoints and lifecycle timing drift", () => {
  const legacyRefresh = fixture();
  legacyRefresh.rails.find((rail) => rail.id === "global_oauth").login.refreshPath =
    "/algo/api/v3/user/jobToken?Encode=1";
  assert.throws(
    () => auditQoderCliOracle(legacyRefresh),
    /legacy center refresh path/,
  );

  const coherentWirePaths = fixture();
  const wire = coherentWirePaths.rails.find((rail) => rail.id === "global_pat").wire;
  wire.actualPath = "/algo/api/v2/service/pro/sse/invented_generation?Encode=1";
  wire.signaturePath = "/api/v2/service/pro/sse/invented_generation";
  assert.throws(
    () => auditQoderCliOracle(coherentWirePaths),
    /global_pat\.wire drifted from its independently frozen digest/,
  );

  for (const [field, value] of [
    ["pollIntervalSeconds", 2],
    ["pollTimeoutSeconds", 600],
  ]) {
    const timing = fixture();
    timing.rails.find((rail) => rail.id === "cn_oauth").login[field] = value;
    assert.throws(() => auditQoderCliOracle(timing), /login drifted/);
  }
});

test("oracle rejects refresh user-agent and device-token response drift", () => {
  const userAgent = fixture();
  userAgent.rails.find((rail) => rail.id === "global_oauth").login.refreshHeaders[
    "user-agent"
  ] = "qoder/1.24.2";
  assert.throws(() => auditQoderCliOracle(userAgent), /login drifted/);

  const response = fixture();
  const fields = response.rails.find((rail) => rail.id === "cn_oauth").login
    .refreshResponseRequiredFields;
  fields.splice(fields.indexOf("device_token"), 1, "access_token");
  fields.sort();
  assert.throws(() => auditQoderCliOracle(response), /login drifted/);

  const encoding = fixture();
  encoding.encodingVectors[0].encoded = "coherently-invented-vector";
  encoding.signatureVector.encodedBody = "coherently-invented-vector";
  encoding.signatureVector.signatureMd5 = "00000000000000000000000000000000";
  assert.throws(
    () => auditQoderCliOracle(encoding),
    /encoding vectors drifted from its independently frozen digest/,
  );
});

test("oracle rejects authorization and COSY headers on lifecycle calls", () => {
  const poll = fixture();
  poll.rails.find((rail) => rail.id === "global_oauth").login.pollHeaders.authorization =
    "Bearer <redacted>";
  assert.throws(() => auditQoderCliOracle(poll), /login drifted/);

  const refresh = fixture();
  refresh.rails.find((rail) => rail.id === "cn_oauth").login.refreshHeaders[
    "cosy-clienttype"
  ] = "0";
  assert.throws(() => auditQoderCliOracle(refresh), /login drifted/);
});

test("oracle freezes bounded compatibility without weakening account or terminal safety", () => {
  const forwarded = fixture();
  forwarded.compatibilityPolicy.cnClientIp.trustsDownstreamForwardedHeaders = true;
  assert.throws(
    () => auditQoderCliOracle(forwarded),
    /bounded compatibility policy drifted/,
  );

  const ambiguous = fixture();
  ambiguous.compatibilityPolicy.toolHistory.missingResultIds = "first_match";
  assert.throws(
    () => auditQoderCliOracle(ambiguous),
    /bounded compatibility policy drifted/,
  );

  const fallback = fixture();
  fallback.compatibilityPolicy.safety.crossAccountFallback = true;
  assert.throws(
    () => auditQoderCliOracle(fallback),
    /bounded compatibility policy drifted/,
  );
});
