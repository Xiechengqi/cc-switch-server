import { useQuery } from "@tanstack/react-query";
import { providerHealthApi } from "@/lib/api/providerHealth";
import type { ProviderHealth } from "@/types/proxy";

export const providerHealthKeys = {
  all: ["providerHealth"] as const,
  app: (appType: string) => ["providerHealth", "app", appType] as const,
};

export function useProviderHealthMap(appType: string, enabled = true) {
  return useQuery({
    queryKey: providerHealthKeys.app(appType),
    queryFn: async () => {
      const providers = await providerHealthApi.list(appType);
      return providers.reduce<Record<string, ProviderHealth>>((healthById, health) => {
        healthById[health.provider_id] = health;
        return healthById;
      }, {});
    },
    enabled: enabled && Boolean(appType),
    staleTime: 30_000,
    refetchInterval: 60_000,
    refetchIntervalInBackground: false,
    retry: false,
  });
}

export function useProviderHealth(providerId: string, appType: string) {
  const query = useProviderHealthMap(appType, Boolean(providerId && appType));
  return {
    ...query,
    data: providerId ? query.data?.[providerId] : undefined,
  };
}
