import { useQueryClient } from "@tanstack/react-query";

import { providerHealthKeys } from "@/lib/query/providerHealth";
import { useServerEvent } from "./useServerEvent";

interface ProviderHealthChangedPayload {
  app?: string;
}

export function useProviderHealthRefreshBridge(): void {
  const queryClient = useQueryClient();

  useServerEvent<ProviderHealthChangedPayload>(
    "provider-health.changed",
    (payload) => {
      const app = payload?.app;
      void queryClient.invalidateQueries({
        queryKey: app ? providerHealthKeys.app(app) : providerHealthKeys.all,
      });
    },
  );
}
