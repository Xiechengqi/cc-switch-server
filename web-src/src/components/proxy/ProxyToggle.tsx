/**
 * Header share switch.
 *
 * The local routing proxy is managed as always-on infrastructure. This switch
 * only controls whether the current app has an active share tunnel.
 */

import { useMemo, useState } from "react";
import { Loader2, Share2 } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import { Switch } from "@/components/ui/switch";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import {
  shareApi,
  type AppId,
  type ShareRecord,
} from "@/lib/api";
import {
  useClientTunnelQuery,
  useCreateShareMutation,
  useProvidersQuery,
  useSharesQuery,
} from "@/lib/query";
import { shareKeys } from "@/lib/query/share";
import {
  findShareForProvider,
  isShareableApp,
  resolveShareOwnerEmail,
} from "@/hooks/useProviderShare";
import { cn } from "@/lib/utils";
import { extractErrorMessage } from "@/utils/errorUtils";
import {
  permanentExpiresInSecs,
  UNLIMITED_PARALLEL_LIMIT,
  UNLIMITED_TOKEN_LIMIT,
} from "@/utils/shareUtils";

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
  const { data: shares = [] } = useSharesQuery();
  const createShareMutation = useCreateShareMutation();
  const [stage, setStage] = useState<ShareToggleStage>("idle");
  const [startTarget, setStartTarget] = useState<ShareRecord | null>(null);
  const [pendingIntent, setPendingIntent] = useState<PendingIntent | null>(
    null,
  );

  const { data: clientTunnel } = useClientTunnelQuery();
  const providersQuery = useProvidersQuery(activeApp);
  const currentProviderId = providersQuery.data?.currentProviderId;
  const selectedShare = useMemo(
    () => selectBestShare(shares, activeApp, currentProviderId),
    [shares, activeApp, currentProviderId],
  );
  const shareEnabled = Boolean(selectedShare && isShareRunning(selectedShare));
  const appLabel = getAppLabel(activeApp);

  const pending =
    stage !== "idle" || createShareMutation.isPending;

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

  const createShareForCurrentProvider = async () => {
    if (!isShareableApp(activeApp)) {
      toast.error(
        t("provider.share.unsupportedApp", {
          defaultValue: "当前应用不支持远程分享",
        }),
      );
      return;
    }
    const providerId = providersQuery.data?.currentProviderId;
    if (!providerId) {
      toast.error(
        t("provider.share.noActiveProvider", {
          defaultValue: "请先启用一个 Provider",
        }),
      );
      return;
    }
    const ownerEmail = resolveShareOwnerEmail(
      clientTunnel?.config?.ownerEmail,
      shares,
    );
    if (!ownerEmail) {
      toast.error(
        t("provider.share.ownerRequired", {
          defaultValue: "请先在分享页配置 Client Tunnel Owner 邮箱",
        }),
      );
      return;
    }
    try {
      setStage("creating-share");
      const created = await createShareMutation.mutateAsync({
        ownerEmail,
        bindings: { [activeApp]: providerId },
        forSale: "No",
        tokenLimit: UNLIMITED_TOKEN_LIMIT,
        parallelLimit: UNLIMITED_PARALLEL_LIMIT,
        expiresInSecs: permanentExpiresInSecs(),
      });
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
      const share = selectBestShare(
        latestShares,
        activeApp,
        providersQuery.data?.currentProviderId,
      );
      if (!share) {
        await createShareForCurrentProvider();
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

function selectBestShare(
  shares: ShareRecord[],
  activeApp: AppId,
  currentProviderId?: string | null,
) {
  if (isShareableApp(activeApp) && currentProviderId) {
    const bound = findShareForProvider(shares, activeApp, currentProviderId);
    if (bound) {
      return (
        shares.find((share) => share.id === bound.id && isShareRunning(share)) ??
        bound
      );
    }
  }
  const hasBinding = (share: ShareRecord) =>
    Boolean(share.bindings?.[activeApp as "claude" | "codex" | "gemini"]);
  return (
    shares.find((share) => hasBinding(share) && isShareRunning(share)) ??
    shares.find(hasBinding) ??
    null
  );
}
