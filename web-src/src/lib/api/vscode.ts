import { invokeCommand } from "@/lib/runtime";
import type { CustomEndpoint } from "@/types";
import type { AppId } from "./types";

export interface EndpointLatencyResult {
  url: string;
  latency: number | null;
  status?: number;
  error?: string;
}

export const vscodeApi = {
  async getLiveProviderSettings(appId: AppId) {
    return await invokeCommand("read_live_provider_settings", { app: appId });
  },

  async testApiEndpoints(
    urls: string[],
    options?: { timeoutSecs?: number },
  ): Promise<EndpointLatencyResult[]> {
    return await invokeCommand("test_api_endpoints", {
      urls,
      timeoutSecs: options?.timeoutSecs,
    });
  },

  async getCustomEndpoints(
    appId: AppId,
    providerId: string,
  ): Promise<CustomEndpoint[]> {
    return await invokeCommand("get_custom_endpoints", {
      app: appId,
      providerId: providerId,
    });
  },

  async addCustomEndpoint(
    appId: AppId,
    providerId: string,
    url: string,
  ): Promise<void> {
    await invokeCommand("add_custom_endpoint", {
      app: appId,
      providerId: providerId,
      url,
    });
  },

  async removeCustomEndpoint(
    appId: AppId,
    providerId: string,
    url: string,
  ): Promise<void> {
    await invokeCommand("remove_custom_endpoint", {
      app: appId,
      providerId: providerId,
      url,
    });
  },

  async updateEndpointLastUsed(
    appId: AppId,
    providerId: string,
    url: string,
  ): Promise<void> {
    await invokeCommand("update_endpoint_last_used", {
      app: appId,
      providerId: providerId,
      url,
    });
  },

  async exportConfigToFile(filePath: string) {
    return await invokeCommand("export_config_to_file", {
      filePath,
    });
  },

  async importConfigFromFile(filePath: string) {
    return await invokeCommand("import_config_from_file", {
      filePath,
    });
  },

  async saveFileDialog(defaultName: string): Promise<string | null> {
    return await invokeCommand("save_file_dialog", {
      defaultName,
    });
  },

  async openFileDialog(): Promise<string | null> {
    return await invokeCommand("open_file_dialog");
  },
};
