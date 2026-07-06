import type { AppId } from "@/lib/api/types";
import { PROVIDER_TYPES } from "@/config/constants";
import type { CustomEndpoint, Provider, ProviderMeta } from "@/types";

/**
 * 合并供应商元数据中的自定义端点。
 * - 当 customEndpoints 为空对象时，明确删除自定义端点但保留其它元数据。
 * - 当 customEndpoints 为 null/undefined 时，不修改端点（保留原有端点）。
 * - 当 customEndpoints 存在时，覆盖原有自定义端点。
 * - 若结果为空对象且非明确清空场景则返回 undefined，避免写入空 meta。
 */
export function mergeProviderMeta(
  initialMeta: ProviderMeta | undefined,
  customEndpoints: Record<string, CustomEndpoint> | null | undefined,
): ProviderMeta | undefined {
  const hasCustomEndpoints =
    !!customEndpoints && Object.keys(customEndpoints).length > 0;

  // 明确清空：传入空对象（非 null/undefined）表示用户想要删除所有端点
  const isExplicitClear =
    customEndpoints !== null &&
    customEndpoints !== undefined &&
    Object.keys(customEndpoints).length === 0;

  if (hasCustomEndpoints) {
    return {
      ...(initialMeta ? { ...initialMeta } : {}),
      custom_endpoints: customEndpoints!,
    };
  }

  // 明确清空端点
  if (isExplicitClear) {
    if (!initialMeta) {
      // 新供应商且用户没有添加端点（理论上不会到这里）
      return undefined;
    }

    if ("custom_endpoints" in initialMeta) {
      const { custom_endpoints, ...rest } = initialMeta;
      // 保留其他字段（如 usage_script）
      // 即使 rest 为空，也要返回空对象（让后端知道要清空 meta）
      return Object.keys(rest).length > 0 ? rest : {};
    }

    // initialMeta 中本来就没有 custom_endpoints
    return { ...initialMeta };
  }

  // null/undefined：用户没有修改端点，保持不变
  if (!initialMeta) {
    return undefined;
  }

  if ("custom_endpoints" in initialMeta) {
    const { custom_endpoints, ...rest } = initialMeta;
    return Object.keys(rest).length > 0 ? rest : undefined;
  }

  return { ...initialMeta };
}

export function hasManagedAuthBinding(
  meta: ProviderMeta | undefined,
  authProvider: string,
): boolean {
  const binding = meta?.authBinding;
  return (
    binding?.source === "managed_account" &&
    binding.authProvider === authProvider &&
    typeof binding.accountId === "string" &&
    binding.accountId.trim() !== ""
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
    provider.meta?.providerType === PROVIDER_TYPES.CLAUDE_OAUTH ||
    provider.meta?.providerType === PROVIDER_TYPES.GOOGLE_GEMINI_OAUTH ||
    isAntigravityFamily ||
    isCursorOauthWithManagedAuth(provider) ||
    provider.meta?.providerType === PROVIDER_TYPES.KIRO_OAUTH ||
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
    provider.meta?.providerType === PROVIDER_TYPES.KIRO_OAUTH
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
    provider.meta?.providerType === PROVIDER_TYPES.GOOGLE_GEMINI_OAUTH ||
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
  appId: AppId,
): boolean {
  if (appId === "claude-desktop" && provider.category === "official") {
    return false;
  }
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
  | "claude_oauth"
  | "google_gemini_oauth"
  | "antigravity_oauth"
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

  if (provider.meta?.usage_script?.templateType === "github_copilot") {
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

  if (provider.meta?.providerType === PROVIDER_TYPES.GOOGLE_GEMINI_OAUTH) {
    return "google_gemini_oauth";
  }

  if (
    provider.meta?.providerType === PROVIDER_TYPES.ANTIGRAVITY_OAUTH ||
    provider.meta?.providerType === PROVIDER_TYPES.AGY_OAUTH
  ) {
    return "antigravity_oauth";
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
