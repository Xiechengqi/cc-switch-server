import { useQuery } from "@tanstack/react-query";
import type { AppId } from "@/lib/api";
import { subscriptionApi } from "@/lib/api/subscription";
import type { CachedOauthQuota } from "@/lib/api/subscription";

export interface UseOllamaQuotaOptions {
  enabled?: boolean;
  appId?: AppId;
}

/**
 * Read cached Ollama Cloud account info (Plan + Email) from the OauthQuotaService.
 *
 * The backend's `spawn_oauth_quota_refresher` loop automatically calls
 * `POST https://ollama.com/api/me` with the provider's API Key and caches
 * the result. This hook reads that cache; no manual configuration needed.
 */
export function useOllamaQuota(
  providerId: string,
  options: UseOllamaQuotaOptions = {},
) {
  const { enabled = true, appId } = options;
  return useQuery<CachedOauthQuota | null>({
    queryKey: ["ollama", "quota", providerId],
    queryFn: async () => {
      const cached = await subscriptionApi.getCachedOauthQuota(
        "ollama_cloud",
        null,
        appId,
        providerId,
      );
      return cached ?? null;
    },
    enabled,
    refetchInterval: false,
    refetchOnWindowFocus: false,
    staleTime: Infinity,
    retry: false,
  });
}
