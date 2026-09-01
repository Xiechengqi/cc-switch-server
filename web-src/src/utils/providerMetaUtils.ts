import type { AppId } from "@/lib/api/types";
import { PROVIDER_TYPES } from "@/config/constants";
import {
  isManagedAccountBindingSource,
  managedAuthProvidersMatch,
} from "@/lib/authBinding";
import type { Provider, ProviderMeta } from "@/types";

export function hasManagedAuthBinding(
  meta: ProviderMeta | undefined,
  authProvider: string,
): boolean {
  const binding = meta?.authBinding;
  return (
    !!binding &&
    isManagedAccountBindingSource(binding.source) &&
    managedAuthProvidersMatch(binding.authProvider, authProvider) &&
    typeof binding.accountId === "string" &&
    binding.accountId.trim() !== ""
  );
}

function isGoogleGeminiOauthProviderType(
  providerType?: string | null,
): boolean {
  return (
    providerType === PROVIDER_TYPES.GOOGLE_GEMINI_OAUTH ||
    providerType === "gemini_cli"
  );
}

export function isCodexOfficialWithManagedAuth(
  provider: Pick<Provider, "category" | "meta">,
): boolean {
  return (
    provider.category === "official" &&
    hasManagedAuthBinding(provider.meta, "codex_oauth")
  );
}

export function isGoogleGeminiOfficialWithManagedAuth(
  provider: Pick<Provider, "category" | "meta">,
): boolean {
  return (
    provider.category === "official" &&
    hasManagedAuthBinding(provider.meta, "google_gemini_oauth")
  );
}

export function isCursorOauthWithManagedAuth(
  provider: Pick<Provider, "meta">,
): boolean {
  return (
    provider.meta?.providerType === PROVIDER_TYPES.CURSOR_OAUTH ||
    hasManagedAuthBinding(provider.meta, PROVIDER_TYPES.CURSOR_OAUTH)
  );
}

function isOpenAIOAuthProviderType(providerType?: string | null): boolean {
  return (
    providerType === PROVIDER_TYPES.CODEX_OAUTH ||
    providerType === "codex_oauth"
  );
}

export function isManagedOauthProvider(
  provider: Pick<Provider, "category" | "meta">,
  appId: AppId,
): boolean {
  const isAntigravityFamily =
    provider.meta?.providerType === PROVIDER_TYPES.ANTIGRAVITY_OAUTH ||
    provider.meta?.providerType === PROVIDER_TYPES.AGY_OAUTH;

  return (
    provider.meta?.providerType === PROVIDER_TYPES.GITHUB_COPILOT ||
    isOpenAIOAuthProviderType(provider.meta?.providerType) ||
    provider.meta?.providerType === PROVIDER_TYPES.GROK_OAUTH ||
    provider.meta?.providerType === PROVIDER_TYPES.CLAUDE_OAUTH ||
    isGoogleGeminiOauthProviderType(provider.meta?.providerType) ||
    isAntigravityFamily ||
    isCursorOauthWithManagedAuth(provider) ||
    provider.meta?.providerType === PROVIDER_TYPES.KIRO_OAUTH ||
    provider.meta?.providerType === PROVIDER_TYPES.KIMI_CODE ||
    provider.meta?.providerType === PROVIDER_TYPES.DEEPSEEK_ACCOUNT ||
    (appId === "codex" && isCodexOfficialWithManagedAuth(provider)) ||
    (appId === "gemini" && isGoogleGeminiOfficialWithManagedAuth(provider))
  );
}

export function canTestModelProvider(
  provider: Pick<Provider, "category" | "meta">,
  appId: AppId,
): boolean {
  const isAntigravityFamily =
    provider.meta?.providerType === PROVIDER_TYPES.ANTIGRAVITY_OAUTH ||
    provider.meta?.providerType === PROVIDER_TYPES.AGY_OAUTH;

  if (provider.meta?.providerType === PROVIDER_TYPES.CLAUDE_OAUTH) {
    return true;
  }

  if (provider.meta?.providerType === PROVIDER_TYPES.DEEPSEEK_ACCOUNT) {
    return true;
  }

  if (provider.meta?.providerType === PROVIDER_TYPES.OLLAMA_CLOUD) {
    return true;
  }

  if (
    provider.meta?.providerType === PROVIDER_TYPES.GITHUB_COPILOT ||
    isOpenAIOAuthProviderType(provider.meta?.providerType) ||
    provider.meta?.providerType === PROVIDER_TYPES.CURSOR_APIKEY ||
    isAntigravityFamily ||
    isCursorOauthWithManagedAuth(provider) ||
    provider.meta?.providerType === PROVIDER_TYPES.KIRO_OAUTH ||
    provider.meta?.providerType === PROVIDER_TYPES.KIMI_CODE
  ) {
    return true;
  }

  if (
    (appId === "codex" || appId === "claude") &&
    isCodexOfficialWithManagedAuth(provider)
  ) {
    return true;
  }

  if (
    isGoogleGeminiOauthProviderType(provider.meta?.providerType) ||
    isAntigravityFamily ||
    (appId === "gemini" && isGoogleGeminiOfficialWithManagedAuth(provider))
  ) {
    return true;
  }

  if (provider.category === "official") {
    return false;
  }

  return true;
}

/// HTTP reachability probe ("测试链接"). Official providers intentionally leave
/// base_url empty and route through the client's default/OAuth endpoint, so
/// cc-switch has no reliable reachability target for them.
export function canTestLinkProvider(
  provider: Pick<Provider, "category" | "meta">,
  _appId: AppId,
): boolean {
  return provider.category !== "official";
}

/** @deprecated Use [`canTestModelProvider`] for model tests or [`canTestLinkProvider`] for link tests. */
export function canTestProvider(
  provider: Pick<Provider, "category" | "meta">,
  appId: AppId,
): boolean {
  return canTestModelProvider(provider, appId);
}

export type ProviderQuotaSource =
  | "copilot"
  | "codex_oauth"
  | "grok_oauth"
  | "claude_oauth"
  | "google_gemini_oauth"
  | "antigravity_oauth"
  | "agy_oauth"
  | "cursor_oauth"
  | "cursor_apikey"
  | "kiro_oauth"
  | "ollama_cloud"
  | "official"
  | "none";

export function getProviderQuotaSource(
  provider: Pick<Provider, "category" | "meta">,
  appId: AppId,
): ProviderQuotaSource {
  if (provider.meta?.providerType === PROVIDER_TYPES.GITHUB_COPILOT) {
    return "copilot";
  }

  if (provider.meta?.providerType === PROVIDER_TYPES.CLAUDE_OAUTH) {
    return "claude_oauth";
  }

  if (
    isOpenAIOAuthProviderType(provider.meta?.providerType) ||
    (appId === "codex" && isCodexOfficialWithManagedAuth(provider))
  ) {
    return "codex_oauth";
  }

  if (provider.meta?.providerType === PROVIDER_TYPES.GROK_OAUTH) {
    return "grok_oauth";
  }

  if (isGoogleGeminiOauthProviderType(provider.meta?.providerType)) {
    return "google_gemini_oauth";
  }

  if (provider.meta?.providerType === PROVIDER_TYPES.ANTIGRAVITY_OAUTH) {
    return "antigravity_oauth";
  }

  if (provider.meta?.providerType === PROVIDER_TYPES.AGY_OAUTH) {
    return "agy_oauth";
  }

  if (isCursorOauthWithManagedAuth(provider)) {
    return "cursor_oauth";
  }

  if (provider.meta?.providerType === PROVIDER_TYPES.CURSOR_APIKEY) {
    return "cursor_apikey";
  }

  if (provider.meta?.providerType === PROVIDER_TYPES.KIRO_OAUTH) {
    return "kiro_oauth";
  }

  if (provider.meta?.providerType === PROVIDER_TYPES.OLLAMA_CLOUD) {
    return "ollama_cloud";
  }

  if (appId === "gemini" && isGoogleGeminiOfficialWithManagedAuth(provider)) {
    return "google_gemini_oauth";
  }

  if (provider.category === "official") {
    return "official";
  }

  return "none";
}
