import { useQuery } from "@tanstack/react-query";
import { authListAccounts } from "@/lib/api/auth";
import {
  loadAccountCapabilities,
  type AccountManagerCapability,
} from "@/lib/server-legacy-api";

export const accountCapabilityKeys = {
  all: ["account-capabilities"] as const,
};

export const managedAccountKeys = {
  all: ["managed-auth-accounts"] as const,
};

export function useManagedAccountsQuery(
  options: { enabled?: boolean } = {},
) {
  return useQuery({
    queryKey: managedAccountKeys.all,
    queryFn: () => authListAccounts(),
    enabled: options.enabled ?? true,
    staleTime: 30_000,
  });
}

export function useAccountCapabilitiesQuery(
  options: { enabled?: boolean } = {},
) {
  return useQuery({
    queryKey: accountCapabilityKeys.all,
    queryFn: loadAccountCapabilities,
    enabled: options.enabled ?? true,
    staleTime: 5 * 60 * 1000,
  });
}

export function findAccountCapability(
  capabilities: AccountManagerCapability[] | undefined,
  providerType: string,
): AccountManagerCapability | undefined {
  return capabilities?.find(
    (capability) => capability.providerType === providerType,
  );
}

export function accountCapabilitySupportsManagedBinding(
  capability: AccountManagerCapability | undefined,
): boolean {
  return Boolean(
    capability?.inferenceBindingSupported &&
    capability.credentialOwnership === "managed_account" &&
    !capability.deprecatedForInference,
  );
}

export type ManagedAccountCapabilityState =
  "loading" | "load_error" | "unsupported" | "supported";

export function resolveManagedAccountCapabilityState(
  queryStatus: "pending" | "error" | "success",
  capability: AccountManagerCapability | undefined,
): ManagedAccountCapabilityState {
  if (queryStatus === "pending") return "loading";
  if (queryStatus === "error") return "load_error";
  return accountCapabilitySupportsManagedBinding(capability)
    ? "supported"
    : "unsupported";
}

export function accountCapabilitySupportsAuthCenter(
  capability: AccountManagerCapability | undefined,
): boolean {
  return Boolean(
    accountCapabilitySupportsManagedBinding(capability) &&
    (capability?.supportsStartLogin || capability?.supportsImport),
  );
}

export function hasLiveQuotaRefreshCapability(
  capabilities: AccountManagerCapability[] | undefined,
): boolean {
  return Boolean(
    capabilities?.some(
      (capability) =>
        capability.supportsLiveQuotaRefresh &&
        capability.quotaCapability === "live_refresh",
    ),
  );
}

export function liveQuotaQueryRoots(
  capabilities: AccountManagerCapability[] | undefined,
): string[] {
  const roots = new Set<string>();
  for (const capability of capabilities ?? []) {
    if (
      !capability.supportsLiveQuotaRefresh ||
      capability.quotaCapability !== "live_refresh"
    ) {
      continue;
    }
    const root = quotaQueryRoot(capability.providerType);
    if (root) roots.add(root);
  }
  return [...roots];
}

function quotaQueryRoot(providerType: string): string | null {
  switch (providerType) {
    case "gemini_cli":
      return "google_gemini_oauth";
    case "ollama_cloud":
      return "ollama";
    case "claude_oauth":
    case "codex_oauth":
    case "grok_oauth":
    case "kiro_oauth":
    case "amazon_q_oauth":
    case "qoder_cosy":
    case "antigravity_oauth":
    case "agy_oauth":
      return providerType;
    default:
      return null;
  }
}
