import { invokeCommand } from "@/lib/runtime";
import type {
  UsageSummary,
  UsageSummaryByApp,
  DailyStats,
  ProviderStats,
  ModelStats,
  RequestLog,
  LogFilters,
  PaginatedLogs,
  SessionSyncResult,
  DataSourceSummary,
} from "@/types/usage";
import type { AppId } from "./types";

export const usageApi = {
  // Proxy usage statistics methods
  getUsageSummary: async (
    startDate?: number,
    endDate?: number,
    appType?: string,
    providerName?: string,
    model?: string,
  ): Promise<UsageSummary> => {
    return invokeCommand("get_usage_summary", {
      startDate,
      endDate,
      appType,
      providerName,
      model,
    });
  },

  getUsageSummaryByApp: async (
    startDate?: number,
    endDate?: number,
    providerName?: string,
    model?: string,
  ): Promise<UsageSummaryByApp[]> => {
    return invokeCommand("get_usage_summary_by_app", {
      startDate,
      endDate,
      providerName,
      model,
    });
  },

  getUsageTrends: async (
    startDate?: number,
    endDate?: number,
    appType?: string,
    providerName?: string,
    model?: string,
  ): Promise<DailyStats[]> => {
    return invokeCommand("get_usage_trends", {
      startDate,
      endDate,
      appType,
      providerName,
      model,
    });
  },

  getProviderStats: async (
    startDate?: number,
    endDate?: number,
    appType?: string,
    providerName?: string,
    model?: string,
  ): Promise<ProviderStats[]> => {
    return invokeCommand("get_provider_stats", {
      startDate,
      endDate,
      appType,
      providerName,
      model,
    });
  },

  getModelStats: async (
    startDate?: number,
    endDate?: number,
    appType?: string,
    providerName?: string,
    model?: string,
  ): Promise<ModelStats[]> => {
    return invokeCommand("get_model_stats", {
      startDate,
      endDate,
      appType,
      providerName,
      model,
    });
  },

  getRequestLogs: async (
    filters: LogFilters,
    page: number = 0,
    pageSize: number = 20,
  ): Promise<PaginatedLogs> => {
    return invokeCommand("get_request_logs", {
      filters,
      page,
      pageSize,
    });
  },

  getRequestDetail: async (requestId: string): Promise<RequestLog | null> => {
    return invokeCommand("get_request_detail", { requestId });
  },

  // Session usage sync
  syncSessionUsage: async (): Promise<SessionSyncResult> => {
    return invokeCommand("sync_session_usage");
  },

  getDataSourceBreakdown: async (): Promise<DataSourceSummary[]> => {
    return invokeCommand("get_usage_data_sources");
  },
};
