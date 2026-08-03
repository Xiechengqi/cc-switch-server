import type { ProviderMeta } from "@/types";

export interface ManagedAccountIdentity {
  accountId: string;
  authIdentityGeneration: number;
}

export function normalizeManagedAuthProvider(authProvider: string): string {
  return authProvider === "gemini_cli" ? "google_gemini_oauth" : authProvider;
}

export function managedAuthProvidersMatch(
  left: string | null | undefined,
  right: string | null | undefined,
): boolean {
  if (!left || !right) return false;
  return (
    normalizeManagedAuthProvider(left) === normalizeManagedAuthProvider(right)
  );
}

export function isManagedAccountBindingSource(
  source: string | null | undefined,
): boolean {
  return (
    source === "managed_account" ||
    source === "account" ||
    source === "account_store"
  );
}

export function resolveManagedAccountId(
  meta: ProviderMeta | undefined,
  authProvider: string,
): string | null {
  const binding = meta?.authBinding;

  if (
    binding &&
    isManagedAccountBindingSource(binding.source) &&
    managedAuthProvidersMatch(binding.authProvider, authProvider)
  ) {
    return binding.accountId ?? null;
  }

  if (authProvider === "github_copilot") {
    return meta?.githubAccountId ?? null;
  }

  return null;
}

export function resolveManagedAccountIdentity(
  meta: ProviderMeta | undefined,
  authProvider: string,
): ManagedAccountIdentity | null {
  const binding = meta?.authBinding;
  if (
    !binding ||
    !isManagedAccountBindingSource(binding.source) ||
    !managedAuthProvidersMatch(binding.authProvider, authProvider) ||
    !binding.accountId ||
    !Number.isSafeInteger(binding.authIdentityGeneration) ||
    (binding.authIdentityGeneration ?? -1) < 0
  ) {
    return null;
  }

  return {
    accountId: binding.accountId,
    authIdentityGeneration: binding.authIdentityGeneration!,
  };
}
