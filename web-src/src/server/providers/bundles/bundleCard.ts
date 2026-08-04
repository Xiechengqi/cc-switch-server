import type { ManagedAuthAccount } from "@/lib/api/auth";
import type { ProviderBundleView, ProviderResource } from "@/lib/api/providers";
import { managedAuthProvidersMatch } from "@/lib/authBinding";
import { familyById, profileById } from "@/server/providerRegistry";
import { endpointEnvironmentKey } from "@/server/providers/editor/providerDraft";
import { extractCodexBaseUrl } from "@/utils/providerConfigUtils";

const DEFAULT_API_URLS: Partial<Record<string, string>> = {
  "codex.openai_api_key": "https://api.openai.com",
  "gemini.google_api_key": "https://generativelanguage.googleapis.com",
};

export interface ProviderBundleDisplayTarget {
  kind: "oauth_account" | "api_url";
  value: string | null;
}

export function providerBundlePrimaryResource(
  bundle: ProviderBundleView,
): ProviderResource | undefined {
  const family = familyById(bundle.familyId);
  const credentialApp = family
    ? profileById(family.credentialProfileId)?.app
    : undefined;
  if (credentialApp) return bundle.surfaces[credentialApp];

  for (const app of [...bundle.enabledApps, ...bundle.supportedApps]) {
    const resource = bundle.surfaces[app];
    if (resource) return resource;
  }
  return undefined;
}

function endpointFromResource(resource: ProviderResource): string | null {
  const settings = resource.provider.settingsConfig;
  const endpointKey = endpointEnvironmentKey(resource.app);
  const env = settings.env;
  if (env && typeof env === "object" && !Array.isArray(env)) {
    const value = (env as Record<string, unknown>)[endpointKey];
    if (typeof value === "string" && value.trim()) return value.trim();
  }

  const direct = settings[endpointKey];
  if (typeof direct === "string" && direct.trim()) return direct.trim();
  if (resource.app === "codex" && typeof settings.config === "string") {
    const value = extractCodexBaseUrl(settings.config)?.trim();
    if (value) return value;
  }
  return resource.profileId
    ? (DEFAULT_API_URLS[resource.profileId] ?? null)
    : null;
}

function accountForResource(
  resource: ProviderResource,
  accounts: ManagedAuthAccount[],
): ManagedAuthAccount | undefined {
  const binding = resource.provider.meta?.authBinding;
  if (!binding?.accountId) return undefined;
  return accounts.find(
    (account) =>
      account.id === binding.accountId &&
      (!binding.authProvider ||
        managedAuthProvidersMatch(account.provider, binding.authProvider)),
  );
}

export function providerBundleDisplayTarget(
  bundle: ProviderBundleView,
  accounts: ManagedAuthAccount[],
): ProviderBundleDisplayTarget {
  const resource = providerBundlePrimaryResource(bundle);
  if (!resource) return { kind: "api_url", value: null };

  const profile = resource.profileId
    ? profileById(resource.profileId)
    : undefined;
  const binding = resource.provider.meta?.authBinding;
  if (
    profile?.credentialPolicy.mode === "managed_account" ||
    binding?.accountId
  ) {
    const account = accountForResource(resource, accounts);
    return {
      kind: "oauth_account",
      value:
        account?.email?.trim() ||
        account?.login?.trim() ||
        binding?.accountId?.trim() ||
        null,
    };
  }

  return { kind: "api_url", value: endpointFromResource(resource) };
}
