/**
 * Header share switch.
 *
 * The local routing proxy is managed as always-on infrastructure. This switch
 * only controls whether the current app has an active share tunnel.
 */

import { useMemo, useState } from "react";
import { Loader2, Share2 } from "lucide-react";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import { Switch } from "@/components/ui/switch";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { CreateShareDialog } from "@/components/share/CreateShareDialog";
import {
  authApi,
  shareApi,
  type AppId,
  type CreateShareParams,
  type ShareAccessByApp,
  type ShareRecord,
} from "@/lib/api";
import {
  useConfigureTunnelMutation,
  useCreateShareMutation,
  useProvidersQuery,
  useSettingsQuery,
  useShareMarketsQuery,
  useSharesQuery,
  useUpdateShareAclMutation,
} from "@/lib/query";
import type { Provider } from "@/types";
import { shareKeys } from "@/lib/query/share";
import {
  buildProviderOption,
  SHARE_PROVIDER_AUTH_PROVIDERS,
  type ManagedAuthStatusByProvider,
} from "@/components/share/providerOptions";
import { cn } from "@/lib/utils";
import { extractErrorMessage } from "@/utils/errorUtils";
import { getTunnelConfigFromSettings } from "@/utils/shareUtils";

interface ProxyToggleProps {
  className?: string;
  activeApp: AppId;
}

type ShareToggleStage =
  | "idle"
  | "checking"
  | "creating-share"
  | "confirm-start-share"
  | "starting-share"
  | "disabling-share";

type PendingIntent =
  | { type: "create-and-enable" }
  | { type: "start-and-enable"; shareId: string };

export function ProxyToggle({ className, activeApp }: ProxyToggleProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data: settings } = useSettingsQuery();
  const { data: shares = [] } = useSharesQuery();
  const createShareMutation = useCreateShareMutation();
  const updateAclMutation = useUpdateShareAclMutation();
  const configureTunnelMutation = useConfigureTunnelMutation();
  const [stage, setStage] = useState<ShareToggleStage>("idle");
  const [createOpen, setCreateOpen] = useState(false);
  const [startTarget, setStartTarget] = useState<ShareRecord | null>(null);
  const [pendingIntent, setPendingIntent] = useState<PendingIntent | null>(
    null,
  );

  const tunnelConfig = useMemo(
    () => getTunnelConfigFromSettings(settings),
    [settings],
  );
  const {
    data: markets = [],
    isLoading: marketsLoading,
    error: marketsError,
    refetch: refetchMarkets,
  } = useShareMarketsQuery(createOpen);
  const selectedShare = useMemo(
    () => selectBestShare(shares, activeApp),
    [shares, activeApp],
  );
  const shareEnabled = Boolean(selectedShare && isShareRunning(selectedShare));
  const appLabel = getAppLabel(activeApp);
  const providersQuery = useProvidersQuery(activeApp);
  const managedAuthStatusResults = useQueries({
    queries: SHARE_PROVIDER_AUTH_PROVIDERS.map((authProvider) => ({
      queryKey: ["managed-auth-status", authProvider],
      queryFn: () => authApi.authGetStatus(authProvider),
      staleTime: 30000,
    })),
  });
  const deepSeekAccountStatusResult = useQuery({
    queryKey: ["deepseek-account-status"],
    queryFn: () => authApi.deepseekAccountStatus(),
    staleTime: 30000,
  });
  const managedAuthStatusVersion = managedAuthStatusResults
    .map((result) => String(result.dataUpdatedAt))
    .concat(String(deepSeekAccountStatusResult.dataUpdatedAt))
    .join(":");
  const managedAuthStatuses = useMemo<ManagedAuthStatusByProvider>(() => {
    const result: ManagedAuthStatusByProvider = {};
    SHARE_PROVIDER_AUTH_PROVIDERS.forEach((authProvider, index) => {
      const status = managedAuthStatusResults[index]?.data;
      if (status) result[authProvider] = status;
    });
    if (deepSeekAccountStatusResult.data) {
      result.deepseek_account = deepSeekAccountStatusResult.data;
    }
    return result;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [managedAuthStatusVersion]);

  // P8 多 app share：ProxyToggle 进入 Create 对话框时仍把 activeApp 当成"默认聚焦的 slot"。
  // 这里只有 activeApp 对应的 providers 数据，其它 slot 给空数组——CreateShareDialog 会
  // 显示三个 slot，用户在 ProxyToggle 流程里通常只挂当前 app，其它 slot 留空。
  const providersByApp = useMemo(() => {
    const data = providersQuery.data;
    const options = data
      ? Object.values(data.providers ?? {})
          .filter((provider): provider is Provider => Boolean(provider))
          .map((provider) =>
            buildProviderOption(provider, false, managedAuthStatuses),
          )
      : [];
    const result: Record<"claude" | "codex" | "gemini", typeof options> = {
      claude: [],
      codex: [],
      gemini: [],
    };
    const app = (activeApp as "claude" | "codex" | "gemini") ?? "claude";
    result[app] = options;
    return result;
  }, [activeApp, providersQuery.data, managedAuthStatuses]);
  const pending =
    stage !== "idle" ||
    createShareMutation.isPending ||
    configureTunnelMutation.isPending ||
    updateAclMutation.isPending;

  const fetchShares = async () =>
    queryClient.fetchQuery({
      queryKey: shareKeys.list(),
      queryFn: shareApi.list,
    });

  const invalidateShareState = async (shareId?: string) => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: shareKeys.list() }),
      shareId
        ? queryClient.invalidateQueries({ queryKey: shareKeys.detail(shareId) })
        : Promise.resolve(),
      shareId
        ? queryClient.invalidateQueries({
            queryKey: shareKeys.tunnelStatus(shareId),
          })
        : Promise.resolve(),
      shareId
        ? queryClient.invalidateQueries({
            queryKey: shareKeys.connectInfo(shareId),
          })
        : Promise.resolve(),
    ]);
  };

  const disableShare = async () => {
    if (!selectedShare) return;
    try {
      setStage("disabling-share");
      await shareApi.disable(selectedShare.id);
      await invalidateShareState(selectedShare.id);
      toast.success(
        t("share.toggle.disabled", {
          defaultValue: "分享已关闭",
        }),
        { closeButton: true },
      );
    } finally {
      setStage("idle");
    }
  };

  const startShareAndEnable = async (share: ShareRecord) => {
    try {
      setStage("starting-share");
      await shareApi.enable(share.id);
      await invalidateShareState(share.id);
      setPendingIntent(null);
      toast.success(
        t("share.toggle.enabled", {
          defaultValue: "分享已开启",
        }),
        { closeButton: true },
      );
      setStage("idle");
    } catch (error) {
      toast.error(
        t("share.toggle.enableFailed", {
          defaultValue: "开启分享失败：{{error}}",
          error: extractErrorMessage(error),
        }),
      );
      throw error;
    } finally {
      setStage("idle");
    }
  };

  const createAndEnable = async (
    params: CreateShareParams,
    extras: {
      sharedWithEmails: string[];
      marketAccessMode: "selected" | "all";
      saleMarketKind?: "token" | "share";
      accessByApp?: ShareAccessByApp;
    },
  ) => {
    try {
      setStage("creating-share");
      const createParams =
        extras.saleMarketKind === "share"
          ? { ...params, saleMarketKind: "token" as const }
          : params;
      const created = await createShareMutation.mutateAsync(createParams);
      try {
        if (
          extras.saleMarketKind === "share" ||
          extras.marketAccessMode === "all" ||
          extras.sharedWithEmails.length > 0 ||
          (!!extras.accessByApp && Object.keys(extras.accessByApp).length > 0)
        ) {
          await updateAclMutation.mutateAsync({
            shareId: created.id,
            sharedWithEmails: extras.sharedWithEmails,
            marketAccessMode: extras.marketAccessMode,
            saleMarketKind: extras.saleMarketKind ?? "token",
            accessByApp: extras.accessByApp,
          });
        }
      } catch (error) {
        try {
          await shareApi.delete(created.id);
          await invalidateShareState(created.id);
        } catch (rollbackError) {
          throw new Error(
            `${extractErrorMessage(error)}；回滚删除失败：${extractErrorMessage(rollbackError)}`,
          );
        }
        throw error;
      }
      setCreateOpen(false);
      await startShareAndEnable(created);
    } finally {
      setStage("idle");
    }
  };

  const handleEnable = async () => {
    let flowContinuesInDialog = false;
    setStage("checking");
    try {
      const latestShares = await fetchShares();
      const share = selectBestShare(latestShares, activeApp);
      if (!share) {
        flowContinuesInDialog = true;
        setPendingIntent({ type: "create-and-enable" });
        setCreateOpen(true);
        setStage("creating-share");
        return;
      }

      if (!isShareRunning(share)) {
        flowContinuesInDialog = true;
        setPendingIntent({ type: "start-and-enable", shareId: share.id });
        setStartTarget(share);
        setStage("confirm-start-share");
        return;
      }

      await startShareAndEnable(share);
      setStage("idle");
    } catch (error) {
      toast.error(
        t("share.toggle.enableFailed", {
          defaultValue: "开启分享失败：{{error}}",
          error: extractErrorMessage(error),
        }),
      );
    } finally {
      if (!flowContinuesInDialog) {
        setStage("idle");
      }
    }
  };

  const handleToggle = async (checked: boolean) => {
    if (pending) return;
    try {
      if (!checked) {
        await disableShare();
        return;
      }
      await handleEnable();
    } catch (error) {
      toast.error(
        checked
          ? t("share.toggle.enableFailed", {
              defaultValue: "开启分享失败：{{error}}",
              error: extractErrorMessage(error),
            })
          : t("share.toggle.disableFailed", {
              defaultValue: "关闭分享失败：{{error}}",
              error: extractErrorMessage(error),
            }),
      );
    }
  };

  const tooltipText = shareEnabled
    ? t("share.toggle.tooltipActive", {
        app: appLabel,
        defaultValue: "{{app}} 分享已开启，点击关闭",
      })
    : t("share.toggle.tooltipInactive", {
        app: appLabel,
        defaultValue: "开启 {{app}} 分享",
      });

  return (
    <>
      <div
        className={cn(
          "flex items-center gap-1 px-1.5 h-8 rounded-lg bg-muted/50 transition-all",
          className,
        )}
        title={tooltipText}
        aria-label={
          shareEnabled
            ? t("share.toggle.disable", { defaultValue: "关闭分享" })
            : t("share.toggle.enable", { defaultValue: "开启分享" })
        }
      >
        {pending ? (
          <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
        ) : (
          <Share2
            className={cn(
              "h-4 w-4 transition-colors",
              shareEnabled
                ? "text-emerald-500 animate-pulse"
                : "text-muted-foreground",
            )}
          />
        )}
        <Switch
          aria-label={
            shareEnabled
              ? t("share.toggle.disable", { defaultValue: "关闭分享" })
              : t("share.toggle.enable", { defaultValue: "开启分享" })
          }
          checked={shareEnabled}
          onCheckedChange={(checked) => void handleToggle(checked)}
          disabled={pending}
        />
      </div>

      <CreateShareDialog
        open={createOpen}
        onOpenChange={(open) => {
          setCreateOpen(open);
          if (!open) {
            if (pendingIntent?.type === "create-and-enable") {
              setPendingIntent(null);
            }
            setStage("idle");
          }
        }}
        ownerEmail={null}
        defaultApp={activeApp}
        providersByApp={providersByApp}
        isSubmitting={
          createShareMutation.isPending || stage === "creating-share"
        }
        markets={markets}
        marketsLoading={marketsLoading}
        marketsError={marketsError ? extractErrorMessage(marketsError) : null}
        tunnelConfig={tunnelConfig}
        tunnelConfigSaving={configureTunnelMutation.isPending}
        submitLabel={t("share.toggle.createAndEnable", {
          defaultValue: "创建并开启分享",
        })}
        onSaveTunnelConfig={(config) =>
          configureTunnelMutation.mutateAsync(config)
        }
        onRetryMarkets={() => void refetchMarkets()}
        onSubmit={createAndEnable}
      />

      <ConfirmDialog
        isOpen={Boolean(startTarget)}
        variant="info"
        title={t("share.toggle.startTitle", {
          defaultValue: "启动分享",
        })}
        message={t("share.toggle.startDescription", {
          defaultValue: "当前分享尚未启动，启动后即可对外访问。",
        })}
        confirmText={t("share.toggle.startAndEnable", {
          defaultValue: "启动并开启分享",
        })}
        onCancel={() => {
          setStartTarget(null);
          setPendingIntent(null);
          setStage("idle");
        }}
        onConfirm={() => {
          const share = startTarget;
          setStartTarget(null);
          if (!share) return;
          void startShareAndEnable(share).catch(() => undefined);
        }}
      />
    </>
  );
}

function getAppLabel(app: AppId) {
  if (app === "claude") return "Claude";
  if (app === "codex") return "Codex";
  if (app === "gemini") return "Gemini";
  return app;
}

function isShareRunning(share: ShareRecord) {
  return share.status === "active" && Boolean(share.tunnelUrl);
}

function selectBestShare(shares: ShareRecord[], activeApp: AppId) {
  // P8 多 app share：share 现在可以同时支持多个 app；
  // "当前 app 的 share" = share 在该 app 上有 binding。
  const hasBinding = (share: ShareRecord) =>
    Boolean(share.bindings?.[activeApp as "claude" | "codex" | "gemini"]);
  return (
    shares.find((share) => hasBinding(share) && isShareRunning(share)) ??
    shares.find(hasBinding) ??
    null
  );
}
