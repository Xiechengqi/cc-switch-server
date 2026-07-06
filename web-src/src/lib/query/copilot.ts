import { useQuery } from "@tanstack/react-query";
import type { QuotaTier } from "@/types/subscription";
import { subscriptionApi } from "@/lib/api/subscription";

export interface CopilotQuota {
  success: boolean;
  plan: string | null;
  resetDate: string | null;
  tiers: QuotaTier[];
  error: string | null;
  queriedAt: number | null;
}

export interface UseCopilotQuotaOptions {
  enabled?: boolean;
  /** 是否启用自动轮询与窗口 focus 重取，间隔由认证页统一配置 */
  autoQuery?: boolean;
}

export function useCopilotQuota(
  accountId: string | null,
  options: UseCopilotQuotaOptions = {},
) {
  const { enabled = true } = options;
  return useQuery<CopilotQuota>({
    queryKey: ["copilot", "quota", accountId ?? "default"],
    queryFn: async (): Promise<CopilotQuota> => {
      const cached = await subscriptionApi.getCachedOauthQuota(
        "github_copilot",
        accountId,
      );
      const quota = cached?.quota;

      return {
        success: quota?.success ?? false,
        plan: quota?.credentialMessage ?? null,
        resetDate: quota?.tiers?.[0]?.resetsAt ?? null,
        tiers: quota?.tiers ?? [],
        error: quota?.error ?? null,
        queriedAt: quota?.queriedAt ?? null,
      };
    },
    enabled,
    refetchInterval: false,
    refetchOnWindowFocus: false,
    staleTime: Infinity,
    retry: false,
  });
}
