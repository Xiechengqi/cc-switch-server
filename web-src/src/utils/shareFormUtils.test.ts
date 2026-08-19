import { describe, expect, it } from "vitest";

import type { ShareUserGrantMap, ShareUserPolicy } from "@/lib/api/share";
import { buildShareUserGrants } from "./shareFormUtils";

const DEFAULT_POLICY: ShareUserPolicy = {
  tokenPeriod: "lifetime",
  tokenLimit: 10_000,
};

describe("buildShareUserGrants", () => {
  it("preserves Router-managed grants while rebuilding ordinary ACL grants", () => {
    const source: ShareUserGrantMap = {
      "owner@example.com": {
        email: "owner@example.com",
        role: "owner",
        active: true,
        policy: DEFAULT_POLICY,
      },
      "old@example.com": {
        email: "old@example.com",
        role: "shareto",
        active: true,
        policy: DEFAULT_POLICY,
      },
      "renter@example.com": {
        email: "renter@example.com",
        role: "shareto",
        active: true,
        policy: { tokenPeriod: "day", parallelLimit: 2 },
        manager: "routerShareMarket",
        entitlementId: "entitlement-active",
        revision: 7,
      },
      "former@example.com": {
        email: "former@example.com",
        role: "shareto",
        active: false,
        policy: { tokenPeriod: "week", tokenLimit: 5_000 },
        manager: "routerShareMarket",
        entitlementId: "entitlement-revoked",
        revision: 4,
      },
    };

    const result = buildShareUserGrants({
      source,
      ownerEmail: "OWNER@example.com",
      aclEmails: ["new@example.com", "renter@example.com"],
      defaultPolicy: DEFAULT_POLICY,
    });

    expect(result["renter@example.com"]).toBe(source["renter@example.com"]);
    expect(result["former@example.com"]).toBe(source["former@example.com"]);
    expect(result["old@example.com"]).toBeUndefined();
    expect(result["new@example.com"]).toMatchObject({
      email: "new@example.com",
      role: "shareto",
      active: true,
      policy: DEFAULT_POLICY,
    });
  });
});
