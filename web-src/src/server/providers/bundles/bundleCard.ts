import type { ManagedAuthAccount } from "@/lib/api/auth";
import type { ProviderBundleView, ProviderResource } from "@/lib/api/providers";
import { managedAuthProvidersMatch } from "@/lib/authBinding";
import { familyById, profileById } from "@/server/providerRegistry";

const DEFAULT_API_URLS: Partial<Record<string, string>> = {
  "codex.openai_api_key": "https://api.openai.com",
  "gemini.google_api_key": "https://generativelanguage.googleapis.com",
};

export interface ProviderBundleDisplayTarget {
  kind: "oauth_account" | "api_key_account" | "api_url";
  value: string | null;
  subscriptionLevel?: string | null;
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

export function providerBundleTestResource(
  bundle: ProviderBundleView,
): ProviderResource | undefined {
  if (!bundle.enabledApps.includes(bundle.testApp)) return undefined;
  return bundle.surfaces[bundle.testApp];
}

function endpointFromResource(resource: ProviderResource): string | null {
  const endpoint = resource.runtime?.endpoint.trim();
  if (endpoint) return endpoint;
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
      subscriptionLevel: account?.subscriptionLevel?.trim() || null,
    };
  }

  if (resource.cursorAccount) {
    return {
      kind: "api_key_account",
      value:
        resource.cursorAccount.label?.trim() ||
        resource.cursorAccount.email?.trim() ||
        resource.cursorAccount.name?.trim() ||
        resource.cursorAccount.credentialName?.trim() ||
        null,
      subscriptionLevel:
        resource.cursorAccount.subscriptionLevel?.trim() || null,
    };
  }

  const candidates = [
    resource,
    ...bundle.enabledApps
      .map((app) => bundle.surfaces[app])
      .filter((candidate): candidate is ProviderResource => Boolean(candidate)),
  ];
  const endpoint = candidates
    .filter(
      (candidate, index) =>
        candidates.findIndex((item) => item.app === candidate.app) === index,
    )
    .map(endpointFromResource)
    .find((value): value is string => Boolean(value));
  return { kind: "api_url", value: endpoint ?? null };
}
