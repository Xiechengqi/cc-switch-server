import { invokeCommand } from "@/lib/runtime";
import type { ProviderHealth } from "@/types/proxy";

export const providerHealthApi = {
  async get(providerId: string, appType: string): Promise<ProviderHealth> {
    return invokeCommand("get_provider_health", { providerId, appType });
  },
};
