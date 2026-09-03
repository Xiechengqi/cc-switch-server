import { useQuery } from "@tanstack/react-query";
import { subscriptionApi } from "@/lib/api/subscription";
import type { AppId } from "@/lib/api/types";
import type { ProviderMeta } from "@/types";
import { resolveManagedAccountIdentity } from "@/lib/authBinding";
import { PROVIDER_TYPES } from "@/config/constants";
import { oauthQuotaAccountKey } from "@/lib/query/oauthQuotaKeys";
import type { ManagedAuthProvider } from "@/lib/api/auth";
import {
  oauthQuotaSnapshotFromEnvelope,
  type OauthQuotaSnapshot,
} from "@/lib/query/oauthQuotaSnapshot";

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
  authIdentityGeneration?: number,
): Promise<OauthQuotaSnapshot | undefined> {
  const cached = await subscriptionApi.getCachedOauthQuota(
    authProvider,
    accountId,
    appId,
    providerId,
    authIdentityGeneration,
  );
  const expectedIdentity =
    accountId != null && authIdentityGeneration != null
      ? { accountId, authIdentityGeneration }
      : undefined;
  const hasActiveCooldown = (cached?.nextRefreshAt ?? 0) > Date.now();
  if (
    cached?.quota &&
    (cached.quota.credentialStatus !== "not_found" || hasActiveCooldown)
  ) {
    return oauthQuotaSnapshotFromEnvelope(cached, expectedIdentity);
  }
  try {
    const refreshed = await subscriptionApi.refreshOauthQuota(
      authProvider,
      accountId,
      providerType,
      appId,
      providerId,
      false,
      authIdentityGeneration,
    );
    return oauthQuotaSnapshotFromEnvelope(refreshed, expectedIdentity);
  } catch (error) {
    const failedSnapshot = await subscriptionApi.getCachedOauthQuota(
      authProvider,
      accountId,
      appId,
      providerId,
      authIdentityGeneration,
    );
    const quota = oauthQuotaSnapshotFromEnvelope(
      failedSnapshot,
      expectedIdentity,
    );
    if (quota) return quota;
    throw error;
  }
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

export interface UseGrokOauthQuotaOptions {
  enabled?: boolean;
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

export interface UseCodeBuddyOauthQuotaOptions {
  enabled?: boolean;
}

export function resolveCodexQuotaAuthProvider(): string {
  return PROVIDER_TYPES.CODEX_OAUTH;
}

export function useClaudeOauthQuota(
  meta: ProviderMeta | undefined,
  options: UseClaudeOauthQuotaOptions = {},
) {
  const { enabled = true } = options;
  const identity = resolveManagedAccountIdentity(
    meta,
    PROVIDER_TYPES.CLAUDE_OAUTH,
  );
  return useQuery({
    queryKey: oauthQuotaAccountKey(
      "claude_oauth",
      identity?.accountId,
      identity?.authIdentityGeneration,
    ),
    queryFn: async () =>
      fetchOauthQuotaWithFallback(
        "claude_oauth",
        identity!.accountId,
        undefined,
        undefined,
        undefined,
        identity!.authIdentityGeneration,
      ),
    enabled: enabled && identity != null,
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
  const identity = resolveManagedAccountIdentity(meta, authProvider);
  return useQuery({
    queryKey: oauthQuotaAccountKey(
      authProvider,
      identity?.accountId,
      identity?.authIdentityGeneration,
    ),
    queryFn: async () =>
      fetchOauthQuotaWithFallback(
        authProvider,
        identity!.accountId,
        meta?.providerType,
        undefined,
        undefined,
        identity!.authIdentityGeneration,
      ),
    enabled: enabled && identity != null,
    refetchInterval: false,
    refetchOnWindowFocus: false,
    staleTime: Infinity,
    retry: false,
  });
}

export function useGrokOauthQuota(
  meta: ProviderMeta | undefined,
  options: UseGrokOauthQuotaOptions = {},
) {
  const { enabled = true } = options;
  const identity = resolveManagedAccountIdentity(
    meta,
    PROVIDER_TYPES.GROK_OAUTH,
  );
  return useQuery({
    queryKey: oauthQuotaAccountKey(
      PROVIDER_TYPES.GROK_OAUTH,
      identity?.accountId,
      identity?.authIdentityGeneration,
    ),
    queryFn: async () =>
      fetchOauthQuotaWithFallback(
        PROVIDER_TYPES.GROK_OAUTH,
        identity!.accountId,
        meta?.providerType,
        undefined,
        undefined,
        identity!.authIdentityGeneration,
      ),
    enabled: enabled && identity != null,
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
  const identity = resolveManagedAccountIdentity(
    meta,
    PROVIDER_TYPES.GOOGLE_GEMINI_OAUTH,
  );
  return useQuery({
    queryKey: oauthQuotaAccountKey(
      "google_gemini_oauth",
      identity?.accountId,
      identity?.authIdentityGeneration,
    ),
    queryFn: async () =>
      fetchOauthQuotaWithFallback(
        "google_gemini_oauth",
        identity!.accountId,
        undefined,
        undefined,
        undefined,
        identity!.authIdentityGeneration,
      ),
    enabled: enabled && identity != null,
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
  const identity = resolveManagedAccountIdentity(
    meta,
    PROVIDER_TYPES.KIRO_OAUTH,
  );
  return useQuery({
    queryKey: oauthQuotaAccountKey(
      "kiro_oauth",
      identity?.accountId,
      identity?.authIdentityGeneration,
    ),
    queryFn: async () =>
      fetchOauthQuotaWithFallback(
        "kiro_oauth",
        identity!.accountId,
        undefined,
        undefined,
        undefined,
        identity!.authIdentityGeneration,
      ),
    enabled: enabled && identity != null,
    refetchInterval: false,
    refetchOnWindowFocus: false,
    staleTime: Infinity,
    retry: false,
  });
}

export function useCodeBuddyOauthQuota(
  meta: ProviderMeta | undefined,
  options: UseCodeBuddyOauthQuotaOptions = {},
) {
  const { enabled = true } = options;
  const identity = resolveManagedAccountIdentity(
    meta,
    PROVIDER_TYPES.CODEBUDDY_OAUTH,
  );
  return useQuery({
    queryKey: oauthQuotaAccountKey(
      PROVIDER_TYPES.CODEBUDDY_OAUTH,
      identity?.accountId,
      identity?.authIdentityGeneration,
    ),
    queryFn: async () =>
      fetchOauthQuotaWithFallback(
        PROVIDER_TYPES.CODEBUDDY_OAUTH,
        identity!.accountId,
        meta?.providerType,
        undefined,
        undefined,
        identity!.authIdentityGeneration,
      ),
    enabled: enabled && identity != null,
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

export type AntigravityQuotaAuthProvider = Extract<
  ManagedAuthProvider,
  "antigravity_oauth" | "agy_oauth"
>;

export interface UseCursorOauthQuotaOptions {
  enabled?: boolean;
  autoQuery?: boolean;
  appId?: AppId;
  providerId?: string;
}

export function useAntigravityOauthQuota(
  meta: ProviderMeta | undefined,
  authProvider: AntigravityQuotaAuthProvider,
  options: UseAntigravityOauthQuotaOptions = {},
) {
  const { enabled = true } = options;
  const identity = resolveManagedAccountIdentity(meta, authProvider);
  return useQuery({
    queryKey: oauthQuotaAccountKey(
      authProvider,
      identity?.accountId,
      identity?.authIdentityGeneration,
    ),
    queryFn: async () =>
      fetchOauthQuotaWithFallback(
        authProvider,
        identity!.accountId,
        meta?.providerType,
        undefined,
        undefined,
        identity!.authIdentityGeneration,
      ),
    enabled: enabled && identity != null,
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
  const identity = isCursorApiKey
    ? null
    : resolveManagedAccountIdentity(meta, PROVIDER_TYPES.CURSOR_OAUTH);
  const accountId = identity?.accountId ?? null;
  return useQuery({
    queryKey: isCursorApiKey
      ? [
          authProvider,
          "quota",
          providerId ?? "default",
          appId ?? "unknown",
        ]
      : oauthQuotaAccountKey(
          authProvider,
          identity?.accountId,
          identity?.authIdentityGeneration,
        ),
    queryFn: async () =>
      fetchOauthQuotaWithFallback(
        authProvider,
        accountId,
        meta?.providerType,
        appId,
        providerId,
        identity?.authIdentityGeneration,
      ),
    enabled:
      enabled &&
      (isCursorApiKey ? Boolean(appId && providerId) : identity != null),
    refetchInterval: false,
    refetchOnWindowFocus: false,
    refetchOnMount: isCursorApiKey ? "always" : true,
    staleTime: isCursorApiKey ? 30 * 1000 : Infinity,
    retry: isCursorApiKey ? 1 : false,
  });
}
