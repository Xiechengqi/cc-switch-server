import { describe, expect, it } from "vitest";

import { resolveManagedAccountId } from "@/lib/authBinding";
import type { Provider } from "@/types";
import {
  canTestModelProvider,
  getProviderQuotaSource,
  isManagedOauthProvider,
} from "./providerMetaUtils";

function provider(providerType: string): Pick<Provider, "category" | "meta"> {
  return {
    category: "third_party",
    meta: { providerType },
  };
}

describe("provider quota source", () => {
  it("keeps Agy independent from Antigravity", () => {
    expect(getProviderQuotaSource(provider("agy_oauth"), "gemini")).toBe(
      "agy_oauth",
    );
    expect(
      getProviderQuotaSource(provider("antigravity_oauth"), "gemini"),
    ).toBe("antigravity_oauth");
  });

  it("routes CodeBuddy OAuth to its managed quota source", () => {
    const codeBuddy = provider("codebuddy_oauth");

    expect(getProviderQuotaSource(codeBuddy, "claude")).toBe(
      "codebuddy_oauth",
    );
    expect(isManagedOauthProvider(codeBuddy, "claude")).toBe(true);
  });

  it("preserves a server-native gemini_cli account binding", () => {
    const googleProvider: Pick<Provider, "category" | "meta"> = {
      category: "official",
      meta: {
        providerType: "gemini_cli",
        authBinding: {
          source: "managed_account",
          authProvider: "gemini_cli",
          accountId: "gemini-account-1",
        },
      },
    };

    expect(getProviderQuotaSource(googleProvider, "claude")).toBe(
      "google_gemini_oauth",
    );
    expect(isManagedOauthProvider(googleProvider, "claude")).toBe(true);
    expect(canTestModelProvider(googleProvider, "claude")).toBe(true);
    expect(
      resolveManagedAccountId(googleProvider.meta, "google_gemini_oauth"),
    ).toBe("gemini-account-1");
  });
});
