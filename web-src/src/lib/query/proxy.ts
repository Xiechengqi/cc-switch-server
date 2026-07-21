import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { proxyApi } from "@/lib/api/proxy";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";

// ========== 代理服务器状态 Hooks ==========

/**
 * 获取代理服务器状态
 */
export function useProxyStatus(options?: { enabled?: boolean }) {
  const enabled = options?.enabled ?? true;
  return useQuery({
    queryKey: ["proxyStatus"],
    queryFn: () => proxyApi.getProxyStatus(),
    enabled,
    refetchInterval: enabled ? 5000 : false,
    refetchIntervalInBackground: false,
  });
}

/**
 * 检查代理服务器是否运行
 */
export function useIsProxyRunning() {
  return useQuery({
    queryKey: ["proxyRunning"],
    queryFn: () => proxyApi.isProxyRunning(),
    refetchInterval: 2000,
  });
}

/**
 * 检查是否处于接管模式
 */
export function useIsLiveTakeoverActive() {
  return useQuery({
    queryKey: ["liveTakeoverActive"],
    queryFn: () => proxyApi.isLiveTakeoverActive(),
    refetchInterval: 2000,
  });
}

/**
 * 获取各应用接管状态
 */
export function useProxyTakeoverStatus(options?: { enabled?: boolean }) {
  const enabled = options?.enabled ?? true;
  return useQuery({
    queryKey: ["proxyTakeoverStatus"],
    queryFn: () => proxyApi.getProxyTakeoverStatus(),
    enabled,
    refetchInterval: enabled ? 2000 : false,
    refetchIntervalInBackground: false,
  });
}

// ========== 代理服务器控制 Hooks ==========

/**
 * 启动代理服务器
 */
export function useStartProxyServer() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: () => proxyApi.startProxyServer(),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["proxyStatus"] });
      queryClient.invalidateQueries({ queryKey: ["proxyRunning"] });
      queryClient.invalidateQueries({ queryKey: ["liveTakeoverActive"] });
      queryClient.invalidateQueries({ queryKey: ["proxyTakeoverStatus"] });
    },
  });
}

/**
 * 停止代理服务器
 */
export function useStopProxyServer() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: () => proxyApi.stopProxyWithRestore(),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["proxyStatus"] });
      queryClient.invalidateQueries({ queryKey: ["proxyRunning"] });
      queryClient.invalidateQueries({ queryKey: ["liveTakeoverActive"] });
      queryClient.invalidateQueries({ queryKey: ["proxyTakeoverStatus"] });
    },
  });
}

/**
 * 设置应用接管状态
 */
export function useSetProxyTakeoverForApp() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ appType, enabled }: { appType: string; enabled: boolean }) =>
      proxyApi.setProxyTakeoverForApp(appType, enabled),
    onMutate: async ({ appType, enabled }) => {
      await queryClient.cancelQueries({ queryKey: ["proxyTakeoverStatus"] });
      const previous = queryClient.getQueryData<Record<string, boolean>>([
        "proxyTakeoverStatus",
      ]);
      queryClient.setQueryData<Record<string, boolean> | undefined>(
        ["proxyTakeoverStatus"],
        (current) => (current ? { ...current, [appType]: enabled } : current),
      );
      return { previous };
    },
    onError: (_error, _variables, context) => {
      if (context?.previous) {
        queryClient.setQueryData(["proxyTakeoverStatus"], context.previous);
      }
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["proxyTakeoverStatus"] });
      queryClient.invalidateQueries({ queryKey: ["liveTakeoverActive"] });
    },
  });
}

/**
 * 代理模式下切换供应商
 */
export function useSwitchProxyProvider() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    mutationFn: ({
      appType,
      providerId,
    }: {
      appType: string;
      providerId: string;
    }) => proxyApi.switchProxyProvider(appType, providerId),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({ queryKey: ["proxyStatus"] });
      queryClient.invalidateQueries({
        queryKey: ["providers", variables.appType],
      });
    },
    onError: (error: Error) => {
      toast.error(t("proxy.switchFailed", { error: error.message }));
    },
  });
}
