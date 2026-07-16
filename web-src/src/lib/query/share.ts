import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import {
  shareApi,
  type ClientTunnelState,
  type ClientTunnelUpdateParams,
  type ConnectInfo,
  type CreateShareParams,
  type PublicMarket,
  type PayoutProfileState,
  type SavePayoutProfileParams,
  type SaveProviderShareParams,
  type ShareRecord,
  type ShareHealthStatus,
  type ShareTunnelStatus,
  type TunnelConfig,
} from "@/lib/api";
import type { Settings } from "@/types";
import { extractErrorMessage } from "@/utils/errorUtils";

export const SHARE_REFRESH_INTERVAL_MS = 10000;
const TUNNEL_POLL_INTERVAL_MS = SHARE_REFRESH_INTERVAL_MS;
const SHARE_POLL_INTERVAL_MS = SHARE_REFRESH_INTERVAL_MS;

export const shareKeys = {
  all: ["share"] as const,
  lists: () => [...shareKeys.all, "list"] as const,
  list: () => [...shareKeys.lists()] as const,
  detail: (shareId: string) => [...shareKeys.all, "detail", shareId] as const,
  tunnelStatus: (shareId: string) =>
    [...shareKeys.all, "tunnel-status", shareId] as const,
  connectInfo: (shareId: string) =>
    [...shareKeys.all, "connect-info", shareId] as const,
  markets: () => [...shareKeys.all, "markets"] as const,
  clientTunnel: () => [...shareKeys.all, "client-tunnel"] as const,
  clientTunnelStatus: () => [...shareKeys.all, "client-tunnel-status"] as const,
  health: () => [...shareKeys.all, "health"] as const,
  payoutProfile: () => [...shareKeys.all, "payout-profile"] as const,
};

type ShareMutationMessages = {
  successKey: string;
  successDefault: string;
  errorKey: string;
  errorDefault: string;
};

function useShareMutationMessages() {
  const { t } = useTranslation();

  return (
    messages: ShareMutationMessages,
    detail?: string,
  ): { success: string; error: string } => ({
    success: t(messages.successKey, { defaultValue: messages.successDefault }),
    error: t(messages.errorKey, {
      defaultValue: messages.errorDefault,
      error: detail ?? t("common.unknown"),
    }),
  });
}

export function useSharesQuery() {
  return useQuery<ShareRecord[]>({
    queryKey: shareKeys.list(),
    queryFn: shareApi.list,
    refetchInterval: SHARE_POLL_INTERVAL_MS,
    refetchIntervalInBackground: true,
  });
}

export function useShareDetailQuery(shareId?: string | null) {
  return useQuery({
    queryKey: shareId
      ? shareKeys.detail(shareId)
      : [...shareKeys.all, "detail"],
    queryFn: () => shareApi.getDetail(shareId!),
    enabled: Boolean(shareId),
  });
}

export function useShareTunnelStatusQuery(
  shareId?: string | null,
  enabled = false,
  options?: {
    refetchInterval?: number | false;
    refetchIntervalInBackground?: boolean;
  },
) {
  return useQuery({
    queryKey: shareId
      ? shareKeys.tunnelStatus(shareId)
      : [...shareKeys.all, "tunnel-status"],
    queryFn: () => shareApi.getTunnelStatus(shareId!),
    enabled: Boolean(shareId) && enabled,
    refetchInterval: enabled
      ? (options?.refetchInterval ?? TUNNEL_POLL_INTERVAL_MS)
      : false,
    refetchIntervalInBackground: options?.refetchIntervalInBackground ?? true,
  });
}

export function useShareConnectInfoQuery(
  shareId?: string | null,
  enabled = false,
) {
  return useQuery<ConnectInfo>({
    queryKey: shareId
      ? shareKeys.connectInfo(shareId)
      : [...shareKeys.all, "connect-info"],
    queryFn: () => shareApi.getConnectInfo(shareId!),
    enabled: Boolean(shareId) && enabled,
  });
}

export function useShareMarketsQuery(enabled = true) {
  return useQuery<PublicMarket[]>({
    queryKey: shareKeys.markets(),
    queryFn: shareApi.listMarkets,
    enabled,
    staleTime: 60_000,
  });
}

export function useClientTunnelQuery(enabled = true) {
  return useQuery<ClientTunnelState>({
    queryKey: shareKeys.clientTunnel(),
    queryFn: shareApi.getClientTunnel,
    enabled,
    refetchInterval: enabled ? TUNNEL_POLL_INTERVAL_MS : false,
    refetchIntervalInBackground: true,
  });
}

export function useShareHealthQuery(enabled = true) {
  return useQuery<ShareHealthStatus>({
    queryKey: shareKeys.health(),
    queryFn: shareApi.getShareHealthStatus,
    enabled,
    refetchInterval: enabled ? SHARE_POLL_INTERVAL_MS : false,
    refetchIntervalInBackground: true,
  });
}

export function usePayoutProfileQuery(enabled = true) {
  return useQuery<PayoutProfileState>({
    queryKey: shareKeys.payoutProfile(),
    queryFn: shareApi.getOwnerPayoutProfile,
    enabled,
    refetchInterval: enabled ? SHARE_POLL_INTERVAL_MS : false,
    refetchIntervalInBackground: true,
  });
}

export function useSavePayoutProfileMutation() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  return useMutation({
    mutationFn: (profile: SavePayoutProfileParams) =>
      shareApi.saveOwnerPayoutProfile(profile),
    onSuccess: (state) => {
      queryClient.setQueryData(shareKeys.payoutProfile(), state);
      toast.success(t("settings.share.payout.saveSuccess", { defaultValue: "收款信息已保存" }));
    },
    onError: (error: Error) => {
      toast.error(t("settings.share.payout.saveError", { defaultValue: "保存收款信息失败: {{error}}", error: extractErrorMessage(error) }));
    },
  });
}

export function useClearPayoutProfileMutation() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  return useMutation({
    mutationFn: shareApi.clearOwnerPayoutProfile,
    onSuccess: (state) => {
      queryClient.setQueryData(shareKeys.payoutProfile(), state);
      toast.success(t("settings.share.payout.clearSuccess", { defaultValue: "收款信息已清除" }));
    },
    onError: (error: Error) => {
      toast.error(t("settings.share.payout.clearError", { defaultValue: "清除收款信息失败: {{error}}", error: extractErrorMessage(error) }));
    },
  });
}

function useClientTunnelWriteMutation(
  mutationFn: (params: ClientTunnelUpdateParams) => Promise<ClientTunnelState>,
) {
  const queryClient = useQueryClient();
  const buildMessages = useShareMutationMessages();

  return useMutation({
    mutationFn,
    onSuccess: async (state) => {
      queryClient.setQueryData(shareKeys.clientTunnel(), state);
      await queryClient.invalidateQueries({
        queryKey: shareKeys.clientTunnel(),
      });
      toast.success(
        buildMessages({
          successKey: "share.clientTunnel.saveSuccess",
          successDefault: "Client tunnel 已保存",
          errorKey: "",
          errorDefault: "",
        }).success,
      );
    },
    onError: (error: Error) => {
      toast.error(
        buildMessages(
          {
            successKey: "",
            successDefault: "",
            errorKey: "share.clientTunnel.saveError",
            errorDefault: "保存 Client tunnel 失败: {{error}}",
          },
          extractErrorMessage(error),
        ).error,
      );
    },
  });
}

export function useClaimClientTunnelMutation() {
  return useClientTunnelWriteMutation(shareApi.claimClientTunnel);
}

export function useUpdateClientTunnelMutation() {
  return useClientTunnelWriteMutation(shareApi.updateClientTunnel);
}

export function useStartClientTunnelMutation() {
  const queryClient = useQueryClient();
  const buildMessages = useShareMutationMessages();

  return useMutation({
    mutationFn: shareApi.startClientTunnel,
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: shareKeys.clientTunnel(),
      });
      toast.success(
        buildMessages({
          successKey: "share.clientTunnel.startSuccess",
          successDefault: "Client tunnel 已启动",
          errorKey: "",
          errorDefault: "",
        }).success,
      );
    },
    onError: (error: Error) => {
      toast.error(
        buildMessages(
          {
            successKey: "",
            successDefault: "",
            errorKey: "share.clientTunnel.startError",
            errorDefault: "启动 Client tunnel 失败: {{error}}",
          },
          extractErrorMessage(error),
        ).error,
      );
    },
  });
}

export function useStopClientTunnelMutation() {
  const queryClient = useQueryClient();
  const buildMessages = useShareMutationMessages();

  return useMutation({
    mutationFn: shareApi.stopClientTunnel,
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: shareKeys.clientTunnel(),
      });
      toast.success(
        buildMessages({
          successKey: "share.clientTunnel.stopSuccess",
          successDefault: "Client tunnel 已停止",
          errorKey: "",
          errorDefault: "",
        }).success,
      );
    },
    onError: (error: Error) => {
      toast.error(
        buildMessages(
          {
            successKey: "",
            successDefault: "",
            errorKey: "share.clientTunnel.stopError",
            errorDefault: "停止 Client tunnel 失败: {{error}}",
          },
          extractErrorMessage(error),
        ).error,
      );
    },
  });
}

function invalidateShareDetail(
  queryClient: ReturnType<typeof useQueryClient>,
  shareId?: string,
) {
  if (!shareId) return Promise.resolve();
  return Promise.all([
    queryClient.invalidateQueries({ queryKey: shareKeys.detail(shareId) }),
    queryClient.invalidateQueries({
      queryKey: shareKeys.tunnelStatus(shareId),
    }),
    queryClient.invalidateQueries({ queryKey: shareKeys.connectInfo(shareId) }),
  ]);
}

export function useCreateShareMutation() {
  const queryClient = useQueryClient();
  const buildMessages = useShareMutationMessages();

  return useMutation({
    mutationFn: (params: CreateShareParams) => shareApi.create(params),
    onSuccess: async (created) => {
      await queryClient.invalidateQueries({ queryKey: shareKeys.list() });
      toast.success(
        buildMessages({
          successKey: "share.toast.createSuccess",
          successDefault: "分享已创建",
          errorKey: "",
          errorDefault: "",
        }).success,
      );
      return created;
    },
    onError: (error: Error) => {
      toast.error(
        buildMessages(
          {
            successKey: "",
            successDefault: "",
            errorKey: "share.toast.createError",
            errorDefault: "创建分享失败: {{error}}",
          },
          extractErrorMessage(error),
        ).error,
      );
    },
  });
}

export function useSaveProviderShareMutation() {
  return useShareActionMutation(
    (params: SaveProviderShareParams) => shareApi.saveProviderShare(params),
    {
      successKey: "provider.share.saveSuccess",
      successDefault: "分享配置已保存",
      errorKey: "provider.share.saveError",
      errorDefault: "保存分享配置失败: {{error}}",
    },
    ({ shareId }) => shareId,
  );
}

function useShareActionMutation<TVariables>(
  mutationFn: (variables: TVariables) => Promise<unknown>,
  messages: ShareMutationMessages,
  getShareId: (variables: TVariables) => string | undefined,
) {
  const queryClient = useQueryClient();
  const buildMessages = useShareMutationMessages();

  return useMutation({
    mutationFn,
    onSuccess: async (_data, variables) => {
      const shareId = getShareId(variables);
      await queryClient.invalidateQueries({ queryKey: shareKeys.list() });
      await invalidateShareDetail(queryClient, shareId);
      toast.success(buildMessages(messages).success);
    },
    onError: async (error: Error, variables) => {
      const shareId = getShareId(variables);
      await queryClient.invalidateQueries({ queryKey: shareKeys.list() });
      await invalidateShareDetail(queryClient, shareId);
      toast.error(buildMessages(messages, extractErrorMessage(error)).error);
    },
  });
}

export function useDeleteShareMutation() {
  return useShareActionMutation(
    (shareId: string) => shareApi.delete(shareId),
    {
      successKey: "share.toast.deleteSuccess",
      successDefault: "分享已删除",
      errorKey: "share.toast.deleteError",
      errorDefault: "删除分享失败: {{error}}",
    },
    (shareId) => shareId,
  );
}

export function usePauseShareMutation() {
  return useShareActionMutation(
    (shareId: string) => shareApi.pause(shareId),
    {
      successKey: "share.toast.pauseSuccess",
      successDefault: "分享已暂停",
      errorKey: "share.toast.pauseError",
      errorDefault: "暂停分享失败: {{error}}",
    },
    (shareId) => shareId,
  );
}

export function useResumeShareMutation() {
  return useShareActionMutation(
    (shareId: string) => shareApi.resume(shareId),
    {
      successKey: "share.toast.resumeSuccess",
      successDefault: "分享已恢复",
      errorKey: "share.toast.resumeError",
      errorDefault: "恢复分享失败: {{error}}",
    },
    (shareId) => shareId,
  );
}

export function useEnableShareMutation() {
  const queryClient = useQueryClient();
  const buildMessages = useShareMutationMessages();

  return useMutation({
    mutationFn: (shareId: string) => shareApi.enable(shareId),
    onSuccess: async (enabledShare, shareId) => {
      const applyEnabledShare = (
        current: ShareRecord | null | undefined,
      ): ShareRecord | null | undefined => {
        if (!current) return enabledShare;
        return current.id === shareId ? enabledShare : current;
      };

      queryClient.setQueryData<ShareTunnelStatus | null>(
        shareKeys.tunnelStatus(shareId),
        () => ({
          info: enabledShare.tunnelUrl
            ? {
                tunnelUrl: enabledShare.tunnelUrl,
                subdomain: enabledShare.subdomain ?? "",
                remotePort: 0,
                healthy: true,
              }
            : null,
          lastError: null,
          requiresOwnerLogin: false,
        }),
      );
      queryClient.setQueryData<ShareRecord[] | undefined>(
        shareKeys.list(),
        (current) => {
          const next = current?.map((share) =>
            share.id === shareId ? enabledShare : share,
          );
          if (next?.some((share) => share.id === shareId)) {
            return next;
          }
          return [...(current ?? []), enabledShare];
        },
      );
      queryClient.setQueryData<ShareRecord | null | undefined>(
        shareKeys.detail(shareId),
        applyEnabledShare,
      );

      await queryClient.invalidateQueries({ queryKey: shareKeys.list() });
      await invalidateShareDetail(queryClient, shareId);
      toast.success(
        buildMessages({
          successKey: "share.toast.enableSuccess",
          successDefault: "分享已开启",
          errorKey: "share.toast.enableError",
          errorDefault: "开启分享失败: {{error}}",
        }).success,
      );
    },
    onError: (error: Error) => {
      toast.error(
        buildMessages(
          {
            successKey: "",
            successDefault: "",
            errorKey: "share.toast.enableError",
            errorDefault: "开启分享失败: {{error}}",
          },
          extractErrorMessage(error),
        ).error,
      );
    },
  });
}

export function useDisableShareMutation() {
  const queryClient = useQueryClient();
  const buildMessages = useShareMutationMessages();

  return useMutation({
    mutationFn: (shareId: string) => shareApi.disable(shareId),
    onSuccess: async (_data, shareId) => {
      queryClient.setQueryData(shareKeys.tunnelStatus(shareId), null);
      queryClient.setQueryData<ShareRecord[] | undefined>(
        shareKeys.list(),
        (current) =>
          current?.map((share) =>
            share.id === shareId
              ? {
                  ...share,
                  status: "paused",
                  tunnelUrl: null,
                  autoStart: false,
                }
              : share,
          ),
      );
      queryClient.setQueryData<ShareRecord | null | undefined>(
        shareKeys.detail(shareId),
        (current) =>
          current
            ? {
                ...current,
                status: "paused",
                tunnelUrl: null,
                autoStart: false,
              }
            : current,
      );

      await queryClient.invalidateQueries({ queryKey: shareKeys.list() });
      await invalidateShareDetail(queryClient, shareId);
      toast.success(
        buildMessages({
          successKey: "share.toast.disableSuccess",
          successDefault: "分享已关闭",
          errorKey: "share.toast.disableError",
          errorDefault: "关闭分享失败: {{error}}",
        }).success,
      );
    },
    onError: (error: Error) => {
      toast.error(
        buildMessages(
          {
            successKey: "",
            successDefault: "",
            errorKey: "share.toast.disableError",
            errorDefault: "关闭分享失败: {{error}}",
          },
          extractErrorMessage(error),
        ).error,
      );
    },
  });
}

export function useResetShareUsageMutation() {
  return useShareActionMutation(
    (shareId: string) => shareApi.resetUsage(shareId),
    {
      successKey: "share.toast.resetUsageSuccess",
      successDefault: "Token 计数已重置",
      errorKey: "share.toast.resetUsageError",
      errorDefault: "重置 Token 计数失败: {{error}}",
    },
    (shareId) => shareId,
  );
}

export function useUpdateShareTokenLimitMutation() {
  return useShareActionMutation(
    ({ shareId, tokenLimit }: { shareId: string; tokenLimit: number }) =>
      shareApi.updateTokenLimit({ shareId, tokenLimit }),
    {
      successKey: "share.toast.updateTokenLimitSuccess",
      successDefault: "Token 上限已更新",
      errorKey: "share.toast.updateTokenLimitError",
      errorDefault: "更新 Token 上限失败: {{error}}",
    },
    ({ shareId }) => shareId,
  );
}

export function useUpdateShareParallelLimitMutation() {
  return useShareActionMutation(
    ({ shareId, parallelLimit }: { shareId: string; parallelLimit: number }) =>
      shareApi.updateParallelLimit({ shareId, parallelLimit }),
    {
      successKey: "share.toast.updateParallelLimitSuccess",
      successDefault: "最大并发数已更新",
      errorKey: "share.toast.updateParallelLimitError",
      errorDefault: "更新最大并发数失败: {{error}}",
    },
    ({ shareId }) => shareId,
  );
}

export function useUpdateShareSubdomainMutation() {
  return useShareActionMutation(
    ({ shareId, subdomain }: { shareId: string; subdomain: string }) =>
      shareApi.updateSubdomain({ shareId, subdomain }),
    {
      successKey: "share.toast.updateSubdomainSuccess",
      successDefault: "Share slug 已更新",
      errorKey: "share.toast.updateSubdomainError",
      errorDefault: "更新 Share slug 失败: {{error}}",
    },
    ({ shareId }) => shareId,
  );
}

export function useUpdateShareDescriptionMutation() {
  return useShareActionMutation(
    ({ shareId, description }: { shareId: string; description: string }) =>
      shareApi.updateDescription({ shareId, description }),
    {
      successKey: "share.toast.updateDescriptionSuccess",
      successDefault: "说明已更新",
      errorKey: "share.toast.updateDescriptionError",
      errorDefault: "更新说明失败: {{error}}",
    },
    ({ shareId }) => shareId,
  );
}

export function useUpdateShareForSaleMutation() {
  return useShareActionMutation(
    ({
      shareId,
      forSale,
    }: {
      shareId: string;
      forSale: "Yes" | "No" | "Free";
    }) => shareApi.updateForSale({ shareId, forSale }),
    {
      successKey: "share.toast.updateForSaleSuccess",
      successDefault: "For Sale 已更新",
      errorKey: "share.toast.updateForSaleError",
      errorDefault: "更新 For Sale 失败: {{error}}",
    },
    ({ shareId }) => shareId,
  );
}

export function useUpdateShareForSaleOfficialPricePercentMutation() {
  return useShareActionMutation(
    ({
      shareId,
      pricing,
    }: {
      shareId: string;
      pricing: Record<string, number>;
    }) => shareApi.updateForSaleOfficialPricePercent({ shareId, pricing }),
    {
      successKey: "share.toast.updateForSalePricingSuccess",
      successDefault: "模型定价已更新",
      errorKey: "share.toast.updateForSalePricingError",
      errorDefault: "更新模型定价失败: {{error}}",
    },
    ({ shareId }) => shareId,
  );
}

export function useUpdateShareExpirationMutation() {
  return useShareActionMutation(
    ({ shareId, expiresAt }: { shareId: string; expiresAt: string }) =>
      shareApi.updateExpiration({ shareId, expiresAt }),
    {
      successKey: "share.toast.updateExpirationSuccess",
      successDefault: "到期时间已更新",
      errorKey: "share.toast.updateExpirationError",
      errorDefault: "更新到期时间失败: {{error}}",
    },
    ({ shareId }) => shareId,
  );
}

export function useUpdateShareAclMutation() {
  return useShareActionMutation(
    ({
      shareId,
      sharedWithEmails,
      marketAccessMode,
      accessByApp,
      appSettings,
      saleMarketKind,
    }: {
      shareId: string;
      sharedWithEmails: string[];
      marketAccessMode: "selected" | "all";
      accessByApp?: import("@/lib/api").ShareAccessByApp;
      appSettings?: import("@/lib/api").ShareAppSettingsByApp;
      saleMarketKind?: import("@/lib/api").ShareSaleMarketKind;
    }) =>
      shareApi.updateAcl({
        shareId,
        sharedWithEmails,
        marketAccessMode,
        accessByApp,
        appSettings,
        saleMarketKind,
      }),
    {
      successKey: "share.toast.updateAclSuccess",
      successDefault: "分享名单已更新",
      errorKey: "share.toast.updateAclError",
      errorDefault: "更新分享名单失败: {{error}}",
    },
    ({ shareId }) => shareId,
  );
}

export function useAuthorizeShareMarketMutation() {
  return useShareActionMutation(
    ({ shareId, marketEmail }: { shareId: string; marketEmail: string }) =>
      shareApi.authorizeMarket(shareId, marketEmail),
    {
      successKey: "share.toast.authorizeShareMarketSuccess",
      successDefault: "账号市场委托已更新",
      errorKey: "share.toast.authorizeShareMarketError",
      errorDefault: "更新账号市场委托失败: {{error}}",
    },
    ({ shareId }) => shareId,
  );
}

export function useStartShareTunnelMutation() {
  return useShareActionMutation(
    (shareId: string) => shareApi.startTunnel(shareId),
    {
      successKey: "share.toast.startTunnelSuccess",
      successDefault: "隧道已启动",
      errorKey: "share.toast.startTunnelError",
      errorDefault: "启动隧道失败: {{error}}",
    },
    (shareId) => shareId,
  );
}

export function useStopShareTunnelMutation() {
  return useShareActionMutation(
    (shareId: string) => shareApi.stopTunnel(shareId),
    {
      successKey: "share.toast.stopTunnelSuccess",
      successDefault: "隧道已停止",
      errorKey: "share.toast.stopTunnelError",
      errorDefault: "停止隧道失败: {{error}}",
    },
    (shareId) => shareId,
  );
}

export function useConfigureTunnelMutation() {
  const queryClient = useQueryClient();
  const buildMessages = useShareMutationMessages();

  return useMutation({
    mutationFn: (config: TunnelConfig) => shareApi.configureTunnel(config),
    onSuccess: async (_data, config) => {
      queryClient.setQueryData<Settings | undefined>(
        ["settings"],
        (current) => {
          if (!current) {
            return current;
          }
          return {
            ...current,
            shareRouterDomain: config.domain,
          };
        },
      );
      await queryClient.invalidateQueries({ queryKey: ["settings"] });
      toast.success(
        buildMessages({
          successKey: "share.tunnel.configSaved",
          successDefault: "隧道配置已保存",
          errorKey: "",
          errorDefault: "",
        }).success,
      );
    },
    onError: (error: Error) => {
      toast.error(
        buildMessages(
          {
            successKey: "",
            successDefault: "",
            errorKey: "share.toast.configureTunnelError",
            errorDefault: "保存隧道配置失败: {{error}}",
          },
          extractErrorMessage(error),
        ).error,
      );
    },
  });
}
