import { useQuery } from "@tanstack/react-query";
import { subscriptionApi } from "@/lib/api/subscription";
import type { AppId } from "@/lib/api/types";
import type { ProviderMeta } from "@/types";
import { resolveManagedAccountId } from "@/lib/authBinding";
import { PROVIDER_TYPES } from "@/config/constants";

const REFETCH_INTERVAL = 5 * 60 * 1000; // 5 minutes

export const subscriptionKeys = {
  all: ["subscription"] as const,
  quota: (appId: AppId) => [...subscriptionKeys.all, "quota", appId] as const,
};

/**
 * 读取缓存的 OAuth 用量；若缓存未命中（后台刷新尚未覆盖该 provider），
 * 主动触发一次强制刷新拉取新数据。后台事件仍是主刷新通道，此处仅兜底首次加载。
 */
async function fetchOauthQuotaWithFallback(
  authProvider: string,
  accountId: string | null,
  providerType?: string | null,
  appId?: AppId | null,
  providerId?: string | null,
) {
  const cached = await subscriptionApi.getCachedOauthQuota(
    authProvider,
    accountId,
    appId,
    providerId,
  );
  if (cached?.quota) return cached.quota;
  const refreshed = await subscriptionApi.refreshOauthQuota(
    authProvider,
    accountId,
    providerType,
    appId,
    providerId,
  );
  return refreshed?.quota;
}

export function useSubscriptionQuota(
  appId: AppId,
  enabled: boolean,
  autoQuery = false,
  autoQueryIntervalMinutes = 5,
) {
  const refetchInterval =
    autoQuery && autoQueryIntervalMinutes > 0
      ? Math.max(autoQueryIntervalMinutes, 1) * 60 * 1000
      : false;

  return useQuery({
    queryKey: subscriptionKeys.quota(appId),
    queryFn: () => subscriptionApi.getQuota(appId),
    enabled: enabled && ["claude", "codex", "gemini"].includes(appId),
    refetchInterval,
    refetchIntervalInBackground: Boolean(refetchInterval),
    refetchOnWindowFocus: Boolean(refetchInterval),
    staleTime:
      autoQueryIntervalMinutes > 0
        ? Math.max(autoQueryIntervalMinutes, 1) * 60 * 1000
        : REFETCH_INTERVAL,
    retry: 1,
  });
}

export interface UseCodexOauthQuotaOptions {
  enabled?: boolean;
  autoQuery?: boolean;
}

export interface UseClaudeOauthQuotaOptions {
  enabled?: boolean;
  autoQuery?: boolean;
}

export interface UseGeminiOauthQuotaOptions {
  enabled?: boolean;
  autoQuery?: boolean;
}

export interface UseKiroOauthQuotaOptions {
  enabled?: boolean;
  autoQuery?: boolean;
}

export function resolveCodexQuotaAuthProvider(): string {
  return PROVIDER_TYPES.CODEX_OAUTH;
}

export function useClaudeOauthQuota(
  meta: ProviderMeta | undefined,
  options: UseClaudeOauthQuotaOptions = {},
) {
  const { enabled = true } = options;
  const accountId = resolveManagedAccountId(meta, PROVIDER_TYPES.CLAUDE_OAUTH);
  return useQuery({
    queryKey: ["claude_oauth", "quota", accountId ?? "default"],
    queryFn: async () => fetchOauthQuotaWithFallback("claude_oauth", accountId),
    enabled,
    refetchInterval: false,
    refetchOnWindowFocus: false,
    staleTime: Infinity,
    retry: false,
  });
}

/**
 * Codex OAuth (ChatGPT Plus/Pro 反代) 订阅额度查询 hook
 *
 * 与 `useSubscriptionQuota` 平行：数据走 cc-switch 自管的 OAuth token，
 * 而不是 Codex CLI 的 ~/.codex/auth.json。
 *
 * Query key 包含 accountId，多张卡片绑定到同一账号时会自动去重共享请求。
 * accountId 为 null 时使用 "default" 占位，让后端 fallback 到默认账号。
 */
export function useCodexOauthQuota(
  meta: ProviderMeta | undefined,
  options: UseCodexOauthQuotaOptions = {},
) {
  const { enabled = true } = options;
  const authProvider = resolveCodexQuotaAuthProvider();
  const accountId = resolveManagedAccountId(meta, authProvider);
  return useQuery({
    queryKey: [authProvider, "quota", accountId ?? "default"],
    queryFn: async () =>
      fetchOauthQuotaWithFallback(authProvider, accountId, meta?.providerType),
    enabled,
    refetchInterval: false,
    refetchOnWindowFocus: false,
    staleTime: Infinity,
    retry: false,
  });
}

export function useGeminiOauthQuota(
  meta: ProviderMeta | undefined,
  options: UseGeminiOauthQuotaOptions = {},
) {
  const { enabled = true } = options;
  const accountId = resolveManagedAccountId(
    meta,
    PROVIDER_TYPES.GOOGLE_GEMINI_OAUTH,
  );
  return useQuery({
    queryKey: ["google_gemini_oauth", "quota", accountId ?? "default"],
    queryFn: async () =>
      fetchOauthQuotaWithFallback("google_gemini_oauth", accountId),
    enabled,
    refetchInterval: false,
    refetchOnWindowFocus: false,
    staleTime: Infinity,
    retry: false,
  });
}

export function useKiroOauthQuota(
  meta: ProviderMeta | undefined,
  options: UseKiroOauthQuotaOptions = {},
) {
  const { enabled = true } = options;
  const accountId = resolveManagedAccountId(meta, PROVIDER_TYPES.KIRO_OAUTH);
  return useQuery({
    queryKey: ["kiro_oauth", "quota", accountId ?? "default"],
    queryFn: async () => fetchOauthQuotaWithFallback("kiro_oauth", accountId),
    enabled,
    refetchInterval: false,
    refetchOnWindowFocus: false,
    staleTime: Infinity,
    retry: false,
  });
}

export interface UseAntigravityOauthQuotaOptions {
  enabled?: boolean;
  autoQuery?: boolean;
}

export interface UseCursorOauthQuotaOptions {
  enabled?: boolean;
  autoQuery?: boolean;
  appId?: AppId;
  providerId?: string;
}

export function useAntigravityOauthQuota(
  meta: ProviderMeta | undefined,
  options: UseAntigravityOauthQuotaOptions = {},
) {
  const { enabled = true } = options;
  const accountId = resolveManagedAccountId(
    meta,
    PROVIDER_TYPES.ANTIGRAVITY_OAUTH,
  );
  return useQuery({
    queryKey: ["antigravity_oauth", "quota", accountId ?? "default"],
    queryFn: async () =>
      fetchOauthQuotaWithFallback(
        "antigravity_oauth",
        accountId,
        meta?.providerType,
      ),
    enabled,
    refetchInterval: false,
    refetchOnWindowFocus: false,
    staleTime: Infinity,
    retry: false,
  });
}

export function useCursorOauthQuota(
  meta: ProviderMeta | undefined,
  options: UseCursorOauthQuotaOptions = {},
) {
  const { enabled = true, appId, providerId } = options;
  const isCursorApiKey = meta?.providerType === PROVIDER_TYPES.CURSOR_APIKEY;
  const authProvider = isCursorApiKey
    ? PROVIDER_TYPES.CURSOR_APIKEY
    : PROVIDER_TYPES.CURSOR_OAUTH;
  const accountId = isCursorApiKey
    ? null
    : resolveManagedAccountId(meta, PROVIDER_TYPES.CURSOR_OAUTH);
  return useQuery({
    queryKey: [
      authProvider,
      "quota",
      accountId ?? providerId ?? "default",
      appId ?? "unknown",
    ],
    queryFn: async () =>
      fetchOauthQuotaWithFallback(
        authProvider,
        accountId,
        meta?.providerType,
        appId,
        providerId,
      ),
    enabled: enabled && (!isCursorApiKey || Boolean(appId && providerId)),
    refetchInterval: false,
    refetchOnWindowFocus: false,
    refetchOnMount: isCursorApiKey ? "always" : true,
    staleTime: isCursorApiKey ? 30 * 1000 : Infinity,
    retry: isCursorApiKey ? 1 : false,
  });
}
