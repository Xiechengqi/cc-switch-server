import assert from "node:assert/strict";
import { it as test } from "vitest";
import type { ShareUserGrantMap } from "@/lib/api/share";
import {
  parseCapabilityPreview,
  emptyCapabilityPolicy,
  SHARE_OPERATIONS,
} from "./share-capabilities";
import {
  capabilityOptions,
  capabilityStatus,
  capabilityPricingText,
  capabilityPermissionMode,
  capabilityPermissionChange,
  reconcileCapabilityGrants,
  capabilityRiskGroups,
  preservesCapabilitySchedule,
  type CapabilityPreview,
} from "./share-capabilities";

const search = "tool.web_search" as const;
const image = "image.generate" as const;
const fingerprint = "a".repeat(64);
function displayPreview(): CapabilityPreview {
  return {
    shareId: "share",
    configRevision: 5,
    displayVersion: 2,
    ready: true,
    legacy: false,
    expiresAt: Date.now() + 60000,
    previewCredential: "ticket",
    policy: emptyCapabilityPolicy(),
    requiredAcknowledgements: [],
    rows: SHARE_OPERATIONS.map((operation) => ({
      app: "codex",
      operation,
      bindingFingerprint: fingerprint,
      controllable: operation === search,
      implemented: operation === search || operation === image,
      providerEnabled: operation !== image,
      accountSupport: "unknown" as const,
      unavailableReason:
        operation === search
          ? null
          : operation === image
            ? ("provider_disabled" as const)
            : ("driver_not_implemented" as const),
      modelScope: "request" as const,
      pricingCoverage: "request_dependent" as const,
      execution: "unknown",
      measurement: "estimated",
      nativeUnit: operation,
      pricing: "missing",
      admission: "estimated_reservation",
      scopes: [],
    })),
  };
}

test("display facts filter unimplemented operations and separate Provider setup from account evidence", () => {
  const preview = displayPreview();
  const options = capabilityOptions(preview, emptyCapabilityPolicy());
  assert.deepEqual(
    options.map((o) => [o.operation, o.available, o.configurable]),
    [
      [image, false, true],
      [search, true, false],
    ],
  );
  const saved = {
    ...emptyCapabilityPolicy(),
    allowedOperations: ["video.generate" as const],
  };
  assert.equal(
    capabilityOptions(preview, saved).find(
      (o) => o.operation === "video.generate",
    )?.selected,
    true,
  );
  assert.equal(
    capabilityStatus(preview.rows.find((r) => r.operation === image)!, 0),
    "需先在供应商设置中开启",
  );
  assert.equal(
    capabilityStatus(preview.rows.find((r) => r.operation === search)!, 0),
    "账号待验证",
  );
  assert.match(capabilityPricingText(preview.rows[0], 0), /实际模型与参数/);
});

test("individual permission modes retain the existing access when choosing a subset", () => {
  assert.equal(capabilityPermissionMode(undefined), "inherit");
  assert.deepEqual(
    capabilityPermissionChange("custom", undefined, [search, image]),
    [search, image],
  );
  assert.deepEqual(
    capabilityPermissionChange("custom", [search], [search, image]),
    [search],
  );
  assert.deepEqual(
    capabilityPermissionChange("disabled", [search], [search]),
    [],
  );
  assert.equal(capabilityPermissionChange("inherit", [], [search]), undefined);
});

test("narrowing Share permissions updates ordinary users without mutating Market contracts or usage", () => {
  const ordinary = {
    email: "a@example.com",
    active: true,
    role: "shareto" as const,
    policy: {
      tokenPeriod: "day" as const,
      tokenLimit: 100,
      allowedOperations: [search, image],
    },
    usage: { tokensUsed: 42 },
  };
  const market = {
    ...ordinary,
    email: "market@example.com",
    manager: "routerShareMarket" as const,
  };
  const grants = {
    [ordinary.email]: ordinary,
    [market.email]: market,
  } as ShareUserGrantMap;
  const result = reconcileCapabilityGrants(
    { ...emptyCapabilityPolicy(), allowedOperations: [search] },
    grants,
  );
  assert.deepEqual(result[ordinary.email].policy.allowedOperations, [search]);
  assert.deepEqual(grants[ordinary.email].policy.allowedOperations, [
    search,
    image,
  ]);
  assert.equal(result[market.email], market);
  assert.equal(result[ordinary.email].usage, ordinary.usage);
});

test("grouped risk review retains exact per-user and per-binding acceptance identities", () => {
  const preview = displayPreview();
  const row = preview.rows.find((r) => r.operation === search)!;
  row.scopes = ["a@example.com", "b@example.com"].map((scope, i) => ({
    scope,
    quotaUnit: "tokens",
    reason: "resource_not_in_token_ledger",
    coverage: "partial",
    condition: `condition-${i}`,
  }));
  preview.requiredAcknowledgements = row.scopes.map((scope) => ({
    condition: scope.condition!,
    scope: scope.scope,
    operation: search,
    quotaUnit: "tokens",
    bindingFingerprint: fingerprint,
    actor: "",
    acceptedAtMs: 0,
  }));
  const groups = capabilityRiskGroups(preview);
  assert.equal(groups.length, 1);
  assert.deepEqual(groups[0].conditions, ["condition-0", "condition-1"]);
  const saved = {
    ...emptyCapabilityPolicy(),
    coverageAcknowledgements: [preview.requiredAcknowledgements[0]],
  };
  assert.deepEqual(capabilityRiskGroups(preview, saved)[0].conditions, [
    "condition-1",
  ]);
});

test("capability edits preserve a pending quota without treating unchanged financial edits as permission changes", () => {
  const policy = {
    tokenPeriod: "day" as const,
    tokenLimit: 100,
    allowedOperations: [search],
  };
  const pendingPolicy = {
    tokenPeriod: "day" as const,
    valueQuota: {
      limitUsdMicros: "1000000",
      tariffPolicyId: "api-equivalent-v1" as const,
      unpricedUsage: "reject" as const,
    },
    allowedOperations: [search],
  };
  const grant = {
    email: "a@example.com",
    active: true,
    role: "shareto" as const,
    policy,
    pendingValueQuota: {
      policy: pendingPolicy,
      effectiveFromMs: 9999999999999,
      retainAccess: true,
    },
  };
  assert.equal(
    preservesCapabilitySchedule(grant, { ...policy, allowedOperations: [] }),
    true,
  );
  assert.equal(
    preservesCapabilitySchedule(grant, {
      ...pendingPolicy,
      allowedOperations: [],
    }),
    true,
  );
  assert.equal(
    preservesCapabilitySchedule(grant, {
      ...policy,
      tokenLimit: 200,
      allowedOperations: [],
    }),
    false,
  );
  assert.equal(preservesCapabilitySchedule(grant, policy), false);
  assert.equal(preservesCapabilitySchedule(grant, pendingPolicy), false);
  const zeroPending = {
    ...pendingPolicy,
    valueQuota: { ...pendingPolicy.valueQuota, unpricedUsage: "zero" as const },
  };
  const zeroGrant = {
    ...grant,
    pendingValueQuota: { ...grant.pendingValueQuota, policy: zeroPending },
  };
  const omittedDefault = {
    ...zeroPending,
    allowedOperations: [],
    valueQuota: { ...zeroPending.valueQuota, unpricedUsage: undefined },
  };
  assert.equal(preservesCapabilitySchedule(zeroGrant, omittedDefault), true);
  assert.equal(preservesCapabilitySchedule(grant, omittedDefault), false);
  assert.equal(
    preservesCapabilitySchedule(grant, {
      ...pendingPolicy,
      valueQuota: {
        ...pendingPolicy.valueQuota,
        tariffPolicyId: "api-equivalent-v2",
      },
    }),
    true,
  );
});

test("malformed display facts cannot turn unknown support or prices into verified capability", () => {
  const preview = displayPreview();
  assert.equal(
    parseCapabilityPreview(preview, "share", { expectedConfigRevision: 5 }),
    preview,
  );
  for (const change of [
    { accountSupport: "yes" },
    { implemented: "true" },
    { pricingCoverage: "full" },
    { unavailableReason: "new_unknown_value" },
  ]) {
    assert.throws(
      () =>
        parseCapabilityPreview(
          { ...preview, rows: [{ ...preview.rows[0], ...change }] },
          "share",
          { expectedConfigRevision: 5 },
        ),
      /invalid or stale/,
    );
  }
});
