import { useQuery } from "@tanstack/react-query";
import { providerHealthApi } from "@/lib/api/providerHealth";

export function useProviderHealth(providerId: string, appType: string) {
  return useQuery({
    queryKey: ["providerHealth", providerId, appType],
    queryFn: () => providerHealthApi.get(providerId, appType),
    enabled: Boolean(providerId && appType),
    refetchInterval: 5000,
    refetchIntervalInBackground: false,
    retry: false,
  });
}
