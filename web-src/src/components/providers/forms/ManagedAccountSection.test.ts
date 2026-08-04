import { describe, expect, it } from "vitest";

import { providerRegistry } from "@/server/providerRegistry";
import { resolveManagedAccountSectionProviderType } from "./ManagedAccountSection";

describe("ManagedAccountSection", () => {
  it("provides an authentication entry for every managed account profile", () => {
    const providerTypes = new Set(
      providerRegistry.profiles.flatMap((profile) =>
        profile.credentialPolicy.mode === "managed_account"
          ? [profile.credentialPolicy.accountProviderType]
          : [],
      ),
    );

    for (const providerType of providerTypes) {
      expect(
        resolveManagedAccountSectionProviderType(providerType),
        providerType,
      ).toBe(providerType);
    }
  });

  it("rejects unknown managed account types", () => {
    expect(
      resolveManagedAccountSectionProviderType("unknown_oauth"),
    ).toBeNull();
  });
});
