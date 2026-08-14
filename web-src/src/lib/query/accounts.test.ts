import { describe, expect, it } from "vitest";

import {
  normalizeAccountCapabilitiesResponse,
  normalizeAccountManagerCapability,
  type AccountManagerCapability,
} from "@/lib/server-legacy-api";
import {
  accountCapabilitySupportsAuthCenter,
  accountCapabilitySupportsManagedBinding,
  findAccountCapability,
  hasLiveQuotaRefreshCapability,
  liveQuotaQueryRoots,
  resolveManagedAccountCapabilityState,
} from "./accounts";

function capability(
  overrides: Partial<AccountManagerCapability> = {},
): AccountManagerCapability {
  return {
    providerType: "claude_oauth",
    manager: "native_oauth_account_with_refresh",
    managerKind: "native_oauth",
    support: "manual_token_store",
    status: "native_login_refresh",
    loginFlows: [],
    supportsStartLogin: true,
    supportsCallback: true,
    supportsRefresh: true,
    supportsQuota: true,
    supportsRefreshPlan: true,
    supportsCachedQuota: true,
    supportsLiveQuotaRefresh: true,
    refreshCapability: "oauth_request",
    quotaCapability: "live_refresh",
    inferenceBindingSupported: true,
    credentialOwnership: "managed_account",
    deprecatedForInference: false,
    migrationTarget: null,
    supportsImport: true,
    supportsDelete: true,
    subscriptionExpiryCapability: "manual_required",
    ...overrides,
  };
}

describe("account capability normalization", () => {
  it("derives the DeepSeek import-only capability from a legacy response", () => {
    const normalized = normalizeAccountManagerCapability({
      providerType: "deepseek_account",
      manager: "manual_token_store",
      support: "manual_token_store",
      status: "manual_import_only",
      loginFlows: [],
      supportsStartLogin: false,
      supportsCallback: false,
      supportsRefresh: false,
      supportsQuota: true,
      supportsRefreshPlan: false,
      supportsImport: true,
      supportsDelete: true,
      subscriptionExpiryCapability: "not_applicable",
    });

    expect(normalized).toMatchObject({
      managerKind: "import_only",
      refreshCapability: "unavailable",
      quotaCapability: "cached_only",
      supportsCachedQuota: true,
      supportsLiveQuotaRefresh: false,
      inferenceBindingSupported: true,
      credentialOwnership: "managed_account",
      deprecatedForInference: false,
      migrationTarget: null,
    });
  });

  it("fails legacy static account records closed for inference binding", () => {
    const normalized = normalizeAccountManagerCapability({
      providerType: "deepseek_api",
      manager: "manual_token_store",
      support: "manual_token_store",
      status: "manual_api_key_available",
      loginFlows: [],
      supportsStartLogin: false,
      supportsCallback: false,
      supportsRefresh: false,
      supportsQuota: true,
      supportsRefreshPlan: false,
      supportsImport: true,
      supportsDelete: true,
      subscriptionExpiryCapability: "not_applicable",
    });

    expect(normalized).toMatchObject({
      managerKind: "static_credential",
      inferenceBindingSupported: false,
      credentialOwnership: "metadata_only",
      deprecatedForInference: true,
      migrationTarget: "provider",
    });
  });

  it("rejects unsuccessful capability responses", () => {
    expect(() =>
      normalizeAccountCapabilitiesResponse({ ok: false, capabilities: [] }),
    ).toThrow("account capability response is invalid");
  });
});

describe("account capability selectors", () => {
  it("finds providers and fails missing or metadata-only bindings closed", () => {
    const managed = capability({ providerType: "deepseek_account" });
    const metadataOnly = capability({
      providerType: "deepseek_api",
      inferenceBindingSupported: false,
      credentialOwnership: "metadata_only",
      deprecatedForInference: true,
      migrationTarget: "provider",
    });

    expect(
      findAccountCapability([managed, metadataOnly], "deepseek_account"),
    ).toBe(managed);
    expect(accountCapabilitySupportsManagedBinding(undefined)).toBe(false);
    expect(accountCapabilitySupportsManagedBinding(metadataOnly)).toBe(false);
    expect(accountCapabilitySupportsManagedBinding(managed)).toBe(true);
    expect(accountCapabilitySupportsAuthCenter(managed)).toBe(true);
    expect(accountCapabilitySupportsAuthCenter(metadataOnly)).toBe(false);
  });

  it("distinguishes capability loading failures from unsupported providers", () => {
    const managed = capability({ providerType: "deepseek_account" });

    expect(resolveManagedAccountCapabilityState("pending", managed)).toBe(
      "loading",
    );
    expect(resolveManagedAccountCapabilityState("error", managed)).toBe(
      "load_error",
    );
    expect(resolveManagedAccountCapabilityState("success", undefined)).toBe(
      "unsupported",
    );
    expect(resolveManagedAccountCapabilityState("success", managed)).toBe(
      "supported",
    );
  });

  it("returns only authoritative live quota query roots", () => {
    const capabilities = [
      capability({ providerType: "claude_oauth" }),
      capability({ providerType: "gemini_cli" }),
      capability({ providerType: "agy_oauth" }),
      capability({ providerType: "qoder_cosy" }),
      capability({ providerType: "ollama_cloud" }),
      capability({
        providerType: "github_copilot",
        supportsLiveQuotaRefresh: true,
        quotaCapability: "imported_snapshot",
      }),
      capability({
        providerType: "cursor_oauth",
        supportsLiveQuotaRefresh: false,
        quotaCapability: "live_refresh",
      }),
    ];

    expect(hasLiveQuotaRefreshCapability(capabilities)).toBe(true);
    expect(liveQuotaQueryRoots(capabilities)).toEqual([
      "claude_oauth",
      "google_gemini_oauth",
      "agy_oauth",
      "qoder_cosy",
      "ollama",
    ]);
  });
});
