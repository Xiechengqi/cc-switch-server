import { describe, expect, it } from "vitest";

import type { ShareUserGrantMap } from "@/lib/api/share";

import { applyShareUserPolicyBatch } from "./share-user-policy-batch";

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
