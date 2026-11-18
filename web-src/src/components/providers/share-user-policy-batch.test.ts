import { describe, expect, it } from "vitest";

import type { ShareUserGrantMap } from "@/lib/api/share";

import { applyShareUserPolicyBatch, validParallelLimit, batchUsesUsd } from "./share-user-policy-batch";

function grants(): ShareUserGrantMap {
  return {
    "alice@example.com": {
      email: "alice@example.com",
      role: "shareto",
      active: true,
      revision: 7,
      entitlementId: "entitlement-1",
      policy: {
        parallelLimit: 2,
        tokenLimit: 1_000,
        tokenPeriod: "day",
        expiresAt: 1_800_000_000_000,
      },
    },
    "bob@example.com": {
      email: "bob@example.com",
      role: "shareto",
      active: true,
      policy: {
        parallelLimit: 4,
        tokenLimit: 5_000,
        tokenPeriod: "week",
      },
    },
  };
}

describe("applyShareUserPolicyBatch", () => {
  it("overwrites only the selected policy groups", () => {
    const source = grants();
    const updated = applyShareUserPolicyBatch(
      source,
      new Set(["alice@example.com", "bob@example.com"]),
      {
        parallelLimit: { value: 8 },
        expiresAt: { value: undefined },
      },
    );

    expect(updated["alice@example.com"].policy).toEqual({
      parallelLimit: 8,
      tokenLimit: 1_000,
      tokenPeriod: "day",
      expiresAt: undefined,
    });
    expect(updated["bob@example.com"].policy).toEqual({
      parallelLimit: 8,
      tokenLimit: 5_000,
      tokenPeriod: "week",
      expiresAt: undefined,
    });
    expect(updated["alice@example.com"].revision).toBe(7);
    expect(updated["alice@example.com"].entitlementId).toBe("entitlement-1");
    expect(source["alice@example.com"].policy.parallelLimit).toBe(2);
  });

  it("updates the token policy only for selected users", () => {
    const updated = applyShareUserPolicyBatch(
      grants(),
      new Set([" ALICE@EXAMPLE.COM ", "missing@example.com"]),
      {
        tokenLimit: {
          value: 9_000,
          period: "sevenDays",
          periodAnchorAtMs: 1_700_000_000_000,
        },
      },
    );

    expect(updated["alice@example.com"].policy).toMatchObject({
      tokenLimit: 9_000,
      tokenPeriod: "sevenDays",
      tokenPeriodAnchorAtMs: 1_700_000_000_000,
    });
    expect(updated["bob@example.com"].policy).toEqual(
      grants()["bob@example.com"].policy,
    );
  });
});


describe("USD concurrency editing", () => {
  it("rejects new high USD values, retains historical values, and leaves Token limits available", () => {
    expect(validParallelLimit(8, false)).toBe(true);
    expect(validParallelLimit(5, true)).toBe(false);
    expect(validParallelLimit(4, true)).toBe(true);
    expect(validParallelLimit(undefined, true)).toBe(true);
    expect(validParallelLimit(8, true, 8)).toBe(true);
    expect(validParallelLimit(9, true, 8)).toBe(false);
    for (const n of [0, -1, 1.5, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(validParallelLimit(n, true, n)).toBe(false);
    }
  });
  it("caps mixed batches while an active USD policy is being converted to Token", () => {
    const source = Object.values(grants());
    source[0].policy.valueQuota = {limitUsdMicros: "1000000", tariffPolicyId: "api-equivalent-v1"};
    const tokenPatch = {tokenLimit:{value:100, period:"day" as const}};
    expect(batchUsesUsd(source, new Set([source[0].email, source[1].email]), tokenPatch)).toBe(true);
    expect(batchUsesUsd(source, new Set([source[1].email]), tokenPatch)).toBe(false);
    expect(batchUsesUsd(source, new Set([source[1].email]), {tokenLimit:{period:"day", valueQuota:source[0].policy.valueQuota}})).toBe(true);
  });
});
