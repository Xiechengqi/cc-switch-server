import { useQuery } from "@tanstack/react-query";
import type { QuotaTier } from "@/types/subscription";
import { subscriptionApi } from "@/lib/api/subscription";
import { oauthQuotaSnapshotFromEnvelope } from "@/lib/query/oauthQuotaSnapshot";
import { oauthQuotaAccountKey } from "@/lib/query/oauthQuotaKeys";

export interface CopilotQuota {
  success: boolean;
  plan: string | null;
  resetDate: string | null;
  tiers: QuotaTier[];
  error: string | null;
  queriedAt: number | null;
  refreshedAt: number | null;
  nextRefreshAt: number | null;
}

export interface UseCopilotQuotaOptions {
  enabled?: boolean;
  /** 是否启用自动轮询与窗口 focus 重取，间隔由认证页统一配置 */
  autoQuery?: boolean;
}

export function useCopilotQuota(
  accountId: string | null,
  authIdentityGeneration: number | null,
  options: UseCopilotQuotaOptions = {},
) {
  const { enabled = true } = options;
  return useQuery<CopilotQuota>({
    queryKey: oauthQuotaAccountKey(
      "github_copilot",
      accountId,
      authIdentityGeneration,
    ),
    queryFn: async (): Promise<CopilotQuota> => {
      const cached = await subscriptionApi.getCachedOauthQuota(
        "github_copilot",
        accountId,
        undefined,
        undefined,
        authIdentityGeneration,
      );
      const quota = oauthQuotaSnapshotFromEnvelope(
        cached,
        accountId != null && authIdentityGeneration != null
          ? { accountId, authIdentityGeneration }
          : undefined,
      );

      return {
        success: quota?.success ?? false,
        plan: quota?.credentialMessage ?? null,
        resetDate: quota?.tiers?.[0]?.resetsAt ?? null,
        tiers: quota?.tiers ?? [],
        error: quota?.error ?? null,
        queriedAt: quota?.queriedAt ?? null,
        refreshedAt: quota?.refreshedAt ?? null,
        nextRefreshAt: quota?.nextRefreshAt ?? null,
      };
    },
    enabled:
      enabled && accountId != null && authIdentityGeneration != null,
    refetchInterval: false,
    refetchOnWindowFocus: false,
    staleTime: Infinity,
    retry: false,
  });
}
