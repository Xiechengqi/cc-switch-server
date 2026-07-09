import {
  useQuery,
  type UseQueryResult,
  keepPreviousData,
} from "@tanstack/react-query";
import {
  providersApi,
  settingsApi,
  sessionsApi,
  type AppId,
} from "@/lib/api";
import type {
  Provider,
  Settings,
  SessionMeta,
  SessionMessage,
} from "@/types";
import { isServerWebRuntime } from "@/lib/runtime";
import { SERVER_DEFAULT_SETTINGS } from "@/lib/serverDefaultSettings";

const sortProviders = (
  providers: Record<string, Provider>,
): Record<string, Provider> => {
  const sortedEntries = Object.values(providers)
    .sort((a, b) => {
      const indexA = a.sortIndex ?? Number.MAX_SAFE_INTEGER;
      const indexB = b.sortIndex ?? Number.MAX_SAFE_INTEGER;
      if (indexA !== indexB) {
        return indexA - indexB;
      }

      const timeA = a.createdAt ?? 0;
      const timeB = b.createdAt ?? 0;
      if (timeA === timeB) {
        return a.name.localeCompare(b.name, "zh-CN");
      }
      return timeA - timeB;
    })
    .map((provider) => [provider.id, provider] as const);

  return Object.fromEntries(sortedEntries);
};

export interface ProvidersQueryData {
  providers: Record<string, Provider>;
  currentProviderId: string;
}

export interface UseProvidersQueryOptions {
  isProxyRunning?: boolean; // 代理服务是否运行中
  enabled?: boolean;
}

export const useProvidersQuery = (
  appId: AppId,
  options?: UseProvidersQueryOptions,
): UseQueryResult<ProvidersQueryData> => {
  const { isProxyRunning = false, enabled = true } = options || {};

  return useQuery({
    queryKey: ["providers", appId],
    enabled,
    placeholderData: keepPreviousData,
    // 当代理服务运行时，每 10 秒刷新一次供应商列表
    // 这样可以自动反映后端熔断器自动禁用代理目标的变更
    refetchInterval: enabled && isProxyRunning ? 10000 : false,
    refetchIntervalInBackground: false,
    queryFn: async () => {
      let providers: Record<string, Provider> = {};
      let currentProviderId = "";

      try {
        providers = await providersApi.getAll(appId);
      } catch (error) {
        if (import.meta.env.DEV) {
          console.warn("获取供应商列表失败:", error);
        }
      }

      try {
        currentProviderId = await providersApi.getCurrent(appId);
      } catch (error) {
        if (import.meta.env.DEV) {
          console.warn("获取当前供应商失败:", error);
        }
      }

      return {
        providers: sortProviders(providers),
        currentProviderId,
      };
    },
  });
};

export const useSettingsQuery = (): UseQueryResult<Settings> => {
  return useQuery({
    queryKey: ["settings"],
    queryFn: async () => {
      try {
        return await settingsApi.get();
      } catch (error) {
        if (isServerWebRuntime()) {
          console.warn("[settings] get_settings failed, using server defaults", error);
          return SERVER_DEFAULT_SETTINGS;
        }
        throw error;
      }
    },
    placeholderData: isServerWebRuntime() ? SERVER_DEFAULT_SETTINGS : undefined,
  });
};

export const useSessionsQuery = () => {
  return useQuery<SessionMeta[]>({
    queryKey: ["sessions"],
    queryFn: async () => sessionsApi.list(),
    staleTime: 30 * 1000,
  });
};

export const useSessionMessagesQuery = (
  providerId?: string,
  sourcePath?: string,
) => {
  return useQuery<SessionMessage[]>({
    queryKey: ["sessionMessages", providerId, sourcePath],
    queryFn: async () => sessionsApi.getMessages(providerId!, sourcePath!),
    enabled: Boolean(providerId && sourcePath),
    staleTime: 30 * 1000,
  });
};
