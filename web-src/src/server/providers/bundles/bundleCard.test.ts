import { describe, expect, it } from "vitest";

import type { ManagedAuthAccount } from "@/lib/api/auth";
import type { ProviderBundleView, ProviderResource } from "@/lib/api/providers";
import type { CoreProviderApp } from "@/server/providerRegistry";
import type { ProviderMeta } from "@/types";
import {
  providerBundleDisplayTarget,
  providerBundlePrimaryResource,
} from "./bundleCard";

function resource(
  app: CoreProviderApp,
  profileId: string,
  settingsConfig: Record<string, unknown>,
  meta?: ProviderMeta,
): ProviderResource {
  return {
    app,
    provider: {
      id: "bundle-1",
      name: "Provider",
      settingsConfig,
      meta,
    },
    providerType: meta?.providerType ?? "custom",
    providerTypeId: meta?.providerType ?? "custom",
    revision: 1,
    profileId,
    identity: { status: "bound" },
    credentialConfigured: true,
    credentialSlots: [],
  };
}

function bundle(
  familyId: string,
  surfaces: Partial<Record<CoreProviderApp, ProviderResource>>,
): ProviderBundleView {
  const supportedApps = Object.keys(surfaces) as CoreProviderApp[];
  return {
    id: "bundle-1",
    familyId,
    revision: 1,
    name: "Provider",
    supportedApps,
    enabledApps: supportedApps,
    credentialConfigured: true,
    credentialSlots: [],
    surfaces,
  };
}

function account(): ManagedAuthAccount {
  return {
    id: "account-1",
    provider: "codex_oauth",
    authIdentityGeneration: 3,
    login: "openai-login",
    email: "owner@example.com",
    avatar_url: null,
    authenticated_at: 1,
    is_default: true,
    github_domain: "",
    subscriptionExpiry: {
      capability: "automatic",
      rule: null,
      ruleNextExpiresAt: null,
      automaticExpiresAt: null,
      legacyManualExpiresAt: null,
      manualExpiresAt: null,
      effectiveExpiresAt: null,
      source: null,
      kind: null,
    },
  };
}

describe("Provider Bundle card data", () => {
  it("uses the family credential Surface and shows its upstream API URL", () => {
    const view = bundle("family.nvidia", {
      claude: resource("claude", "claude.nvidia", {
        env: { ANTHROPIC_BASE_URL: "https://claude.nvidia.example" },
      }),
      codex: resource("codex", "codex.nvidia", {
        env: { OPENAI_BASE_URL: "https://integrate.api.nvidia.com/v1" },
      }),
    });

    expect(providerBundlePrimaryResource(view)?.app).toBe("codex");
    expect(providerBundleDisplayTarget(view, [])).toEqual({
      kind: "api_url",
      value: "https://integrate.api.nvidia.com/v1",
    });
  });

  it("shows the bound OAuth subscription account instead of the Server route", () => {
    const authBinding = {
      source: "managed_account" as const,
      authProvider: "codex_oauth",
      accountId: "account-1",
      authIdentityGeneration: 3,
    };
    const view = bundle("family.openai_oauth", {
      claude: resource(
        "claude",
        "claude.openai_oauth",
        {},
        { providerType: "codex_oauth", authBinding },
      ),
      codex: resource(
        "codex",
        "codex.openai_oauth",
        {},
        { providerType: "codex_oauth", authBinding },
      ),
    });

    expect(providerBundleDisplayTarget(view, [account()])).toEqual({
      kind: "oauth_account",
      value: "owner@example.com",
    });
  });

  it("falls back to the bound account id while account metadata loads", () => {
    const view = bundle("family.openai_oauth", {
      codex: resource(
        "codex",
        "codex.openai_oauth",
        {},
        {
          providerType: "codex_oauth",
          authBinding: {
            source: "managed_account",
            authProvider: "codex_oauth",
            accountId: "account-1",
          },
        },
      ),
    });

    expect(providerBundleDisplayTarget(view, [])).toEqual({
      kind: "oauth_account",
      value: "account-1",
    });
  });
});
