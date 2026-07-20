import { invokeCommand } from "@/lib/runtime";
import type { SubscriptionQuota } from "@/types/subscription";

export interface CachedOauthQuota {
  authProvider: string;
  accountId: string;
  providerId?: string | null;
  providerName?: string | null;
  appType?: string | null;
  quota: SubscriptionQuota;
  refreshedAt: number;
  nextRefreshAt?: number | null;
  source: string;
}

export const subscriptionApi = {
  getQuota: (tool: string, force = false): Promise<SubscriptionQuota> =>
    invokeCommand("get_subscription_quota", { tool, force }),
  getClaudeOauthQuota: (accountId: string | null): Promise<SubscriptionQuota> =>
    invokeCommand("get_claude_oauth_quota", { accountId }),
  getCodexOauthQuota: (accountId: string | null): Promise<SubscriptionQuota> =>
    invokeCommand("get_codex_oauth_quota", { accountId }),
  getCachedOauthQuota: (
    authProvider: string,
    accountId: string | null,
    appType?: string | null,
    providerId?: string | null,
  ): Promise<CachedOauthQuota | null> =>
    invokeCommand("get_cached_oauth_quota", {
      authProvider,
      accountId,
      appType: appType || null,
      providerId: providerId || null,
    }),
  refreshOauthQuota: (
    authProvider: string,
    accountId: string | null,
    providerType?: string | null,
    appType?: string | null,
    providerId?: string | null,
    force = true,
  ): Promise<CachedOauthQuota | null> =>
    invokeCommand("refresh_oauth_quota", {
      authProvider,
      accountId,
      providerType: providerType || null,
      appType: appType || null,
      providerId: providerId || null,
      force,
    }),
};
