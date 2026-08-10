import { useQueryClient } from "@tanstack/react-query";

import { usageKeys } from "@/lib/query/usage";
import { useServerEvent } from "./useServerEvent";

export function useUsageEventBridge() {
  const queryClient = useQueryClient();
  useServerEvent("usage.created", () => {
    void queryClient.invalidateQueries({ queryKey: usageKeys.all });
  });
  useServerEvent("usage.updated", () => {
    void queryClient.invalidateQueries({ queryKey: usageKeys.all });
  });
}
