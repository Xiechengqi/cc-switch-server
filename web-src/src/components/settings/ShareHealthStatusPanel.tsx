import { useMemo } from "react";
import {
  Activity,
  AlertTriangle,
  CheckCircle2,
  ChevronRight,
  Loader2,
  XCircle,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type {
  ShareHealthItem,
  ShareHealthLevel,
  ShareHealthStatus,
} from "@/lib/api/share";
import { useShareHealthQuery } from "@/lib/query";

function healthLabel(level: ShareHealthLevel, t: TFunction): string {
  switch (level) {
    case "healthy":
      return t("settings.share.health.status.healthy", { defaultValue: "正常" });
    case "warning":
      return t("settings.share.health.status.warning", {
        defaultValue: "需关注",
      });
    case "unhealthy":
      return t("settings.share.health.status.unhealthy", {
        defaultValue: "异常",
      });
  }
}

function healthTone(level: ShareHealthLevel) {
  switch (level) {
    case "healthy":
      return {
        icon: CheckCircle2,
        border: "border-emerald-500/40",
        bg: "bg-emerald-500/5",
        text: "text-emerald-600 dark:text-emerald-400",
        dot: "bg-emerald-500",
      };
    case "warning":
      return {
        icon: AlertTriangle,
        border: "border-amber-500/40",
        bg: "bg-amber-500/5",
        text: "text-amber-600 dark:text-amber-400",
        dot: "bg-amber-500",
      };
    case "unhealthy":
      return {
        icon: XCircle,
        border: "border-red-500/40",
        bg: "bg-red-500/5",
        text: "text-red-600 dark:text-red-400",
        dot: "bg-red-500",
      };
  }
}

function formatRelativeTime(
  value?: number | null,
  t?: TFunction,
): string | null {
  if (value == null || value <= 0) return null;
  const deltaMs = Date.now() - value;
  if (deltaMs < 60_000) {
    return t?.("settings.share.health.justNow", { defaultValue: "刚刚" }) ?? "刚刚";
  }
  if (deltaMs < 3_600_000) {
    const minutes = Math.max(1, Math.round(deltaMs / 60_000));
    return (
      t?.("settings.share.health.minutesAgo", {
        defaultValue: "{{count}} 分钟前",
        count: minutes,
      }) ?? `${minutes} 分钟前`
    );
  }
  return new Date(value).toLocaleString();
}

export function formatShareHealthOverview(
  health: ShareHealthStatus | undefined,
  t: TFunction,
): string {
  if (!health) {
    return t("settings.share.sections.health.descriptionLoading", {
      defaultValue: "查看 Router、Client Tunnel 与 Share 链路健康状态",
    });
  }
  if (health.overall === "healthy") {
    return t("settings.share.health.overall.healthy", {
      defaultValue: "全部正常",
    });
  }
  if (health.overall === "warning") {
    return t("settings.share.health.overall.warning", {
      defaultValue: "{{count}} 项需关注",
      count: health.issueCount,
    });
  }
  return t("settings.share.health.overall.unhealthy", {
    defaultValue: "{{count}} 项异常",
    count: health.issueCount,
  });
}

function HealthBadge({
  level,
  t,
}: {
  level: ShareHealthLevel;
  t: TFunction;
}) {
  const tone = healthTone(level);
  const Icon = tone.icon;
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 rounded-full px-2.5 py-0.5 text-xs font-medium",
        tone.bg,
        tone.text,
      )}
    >
      <Icon className="h-3.5 w-3.5" />
      {healthLabel(level, t)}
    </span>
  );
}

function HealthLinkCard({
  title,
  level,
  detail,
  meta,
  error,
  t,
}: {
  title: string;
  level: ShareHealthLevel;
  detail?: string | null;
  meta?: string | null;
  error?: string | null;
  t: TFunction;
}) {
  const tone = healthTone(level);
  return (
    <div
      className={cn(
        "rounded-lg border px-4 py-3",
        tone.border,
        tone.bg,
      )}
    >
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 space-y-1">
          <div className="flex items-center gap-2">
            <span className={cn("h-2 w-2 shrink-0 rounded-full", tone.dot)} />
            <span className="text-sm font-medium">{title}</span>
          </div>
          {detail ? (
            <p className="truncate pl-4 text-sm text-muted-foreground">
              {detail}
            </p>
          ) : null}
          {meta ? (
            <p className="pl-4 text-xs text-muted-foreground">{meta}</p>
          ) : null}
          {error ? (
            <p className="pl-4 text-xs text-red-600 dark:text-red-400">
              {error}
            </p>
          ) : null}
        </div>
        <HealthBadge level={level} t={t} />
      </div>
    </div>
  );
}

export function ShareHealthStatusPanel() {
  const { t } = useTranslation();
  const { data, isLoading, isError, refetch, isFetching } =
    useShareHealthQuery();

  const overallTone = useMemo(
    () => healthTone(data?.overall ?? "warning"),
    [data?.overall],
  );
  const OverallIcon = overallTone.icon;

  if (isLoading && !data) {
    return (
      <div className="flex items-center justify-center py-8 text-sm text-muted-foreground">
        <Loader2 className="mr-2 h-4 w-4 animate-spin" />
        {t("settings.share.health.loading", { defaultValue: "正在检查链路状态…" })}
      </div>
    );
  }

  if (isError || !data) {
    return (
      <div className="space-y-3 rounded-lg border border-border/60 bg-muted/20 px-4 py-4">
        <p className="text-sm text-muted-foreground">
          {t("settings.share.health.loadFailed", {
            defaultValue: "无法加载健康状态，请稍后重试。",
          })}
        </p>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => void refetch()}
        >
          {t("common.refresh", { defaultValue: "刷新" })}
        </Button>
      </div>
    );
  }

  const routerMeta = data.router.registered
    ? [
        t("settings.share.health.router.registered", {
          defaultValue: "已注册",
        }),
        formatRelativeTime(data.router.lastHeartbeatMs, t),
      ]
        .filter(Boolean)
        .join(" · ")
    : t("settings.share.health.router.notRegistered", {
        defaultValue: "未注册",
      });

  const clientMeta = (() => {
    const claimStatus = data.clientTunnel.claimStatus;
    const connectivity = data.clientTunnel.connectivityStatus;
    if (claimStatus === "conflict") {
      return t("settings.share.health.clientTunnel.conflict", {
        defaultValue: "子域名冲突，未在 Router 注册",
      });
    }
    if (claimStatus === "error") {
      return t("settings.share.health.clientTunnel.claimFailed", {
        defaultValue: "Router 注册失败",
      });
    }
    if (claimStatus === "unclaimed") {
      return t("settings.share.health.clientTunnel.unclaimed", {
        defaultValue: "已配置，未在 Router 注册",
      });
    }
    if (connectivity === "connected") {
      return t("settings.share.health.clientTunnel.running", {
        defaultValue: "隧道已连接",
      });
    }
    if (connectivity === "connecting") {
      return t("settings.share.health.clientTunnel.connecting", {
        defaultValue: "已注册，隧道连接中",
      });
    }
    if (data.clientTunnel.expectedUrl || data.clientTunnel.subdomain) {
      return t("settings.share.health.clientTunnel.stopped", {
        defaultValue: "已注册但未连接",
      });
    }
    return t("settings.share.health.notConfigured", {
      defaultValue: "未配置",
    });
  })();

  const healthyShareCount = data.shares.filter(
    (share: ShareHealthItem) => share.status === "healthy",
  ).length;

  return (
    <div className="space-y-4">
      <div
        className={cn(
          "flex items-center justify-between gap-3 rounded-xl border px-4 py-3",
          overallTone.border,
          overallTone.bg,
        )}
      >
        <div className="flex items-center gap-3">
          <div
            className={cn(
              "flex h-10 w-10 items-center justify-center rounded-full",
              overallTone.bg,
            )}
          >
            <OverallIcon className={cn("h-5 w-5", overallTone.text)} />
          </div>
          <div>
            <p className="text-sm font-medium">
              {t("settings.share.health.overallTitle", {
                defaultValue: "链路总览",
              })}
            </p>
            <p className={cn("text-sm", overallTone.text)}>
              {formatShareHealthOverview(data, t)}
            </p>
          </div>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={isFetching}
          onClick={() => void refetch()}
        >
          {isFetching ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : (
            t("common.refresh", { defaultValue: "刷新" })
          )}
        </Button>
      </div>

      <div className="space-y-2">
        <div className="flex items-center gap-2 text-xs font-medium uppercase tracking-wide text-muted-foreground">
          <Activity className="h-3.5 w-3.5" />
          {t("settings.share.health.chainTitle", {
            defaultValue: "链路节点",
          })}
        </div>

        <HealthLinkCard
          title={t("settings.share.health.router.title", {
            defaultValue: "Router",
          })}
          level={data.router.status}
          detail={data.router.domain || undefined}
          meta={routerMeta}
          error={data.router.lastError}
          t={t}
        />

        <div className="flex justify-center">
          <ChevronRight className="h-4 w-4 rotate-90 text-muted-foreground/60" />
        </div>

        <HealthLinkCard
          title={t("settings.share.health.clientTunnel.title", {
            defaultValue: "Client Tunnel",
          })}
          level={data.clientTunnel.status}
          detail={
            data.clientTunnel.activeUrl ||
            data.clientTunnel.expectedUrl ||
            (data.clientTunnel.subdomain
              ? `${data.clientTunnel.subdomain}.${data.router.domain}`
              : undefined)
          }
          meta={clientMeta}
          error={data.clientTunnel.lastError}
          t={t}
        />

        <div className="flex justify-center">
          <ChevronRight className="h-4 w-4 rotate-90 text-muted-foreground/60" />
        </div>

        <div
          className={cn(
            "rounded-lg border px-4 py-3",
            data.shareIssueCount > 0
              ? healthTone(
                  data.shares.some((share: ShareHealthItem) => share.status === "unhealthy")
                    ? "unhealthy"
                    : "warning",
                ).border
              : "border-emerald-500/40",
            data.shareIssueCount > 0
              ? healthTone(
                  data.shares.some((share: ShareHealthItem) => share.status === "unhealthy")
                    ? "unhealthy"
                    : "warning",
                ).bg
              : "bg-emerald-500/5",
          )}
        >
          <div className="mb-3 flex items-center justify-between gap-3">
            <div className="flex items-center gap-2">
              <span
                className={cn(
                  "h-2 w-2 rounded-full",
                  data.shareIssueCount > 0
                    ? healthTone(
                        data.shares.some((share: ShareHealthItem) => share.status === "unhealthy")
                          ? "unhealthy"
                          : "warning",
                      ).dot
                    : "bg-emerald-500",
                )}
              />
              <span className="text-sm font-medium">
                {t("settings.share.health.shares.title", {
                  defaultValue: "Share",
                })}
              </span>
            </div>
            <HealthBadge
              level={
                data.shares.length === 0
                  ? "healthy"
                  : data.shares.some((share: ShareHealthItem) => share.status === "unhealthy")
                    ? "unhealthy"
                    : data.shareIssueCount > 0
                      ? "warning"
                      : "healthy"
              }
              t={t}
            />
          </div>

          {data.shares.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              {t("settings.share.health.noShares", {
                defaultValue: "暂无 Share",
              })}
            </p>
          ) : (
            <div className="space-y-2">
              <p className="text-xs text-muted-foreground">
                {t("settings.share.health.shares.summary", {
                  defaultValue: "{{healthy}}/{{total}} 正常",
                  healthy: healthyShareCount,
                  total: data.shares.length,
                })}
              </p>
              {data.shares.map((share: ShareHealthItem) => {
                const tone = healthTone(share.status);
                return (
                  <div
                    key={share.id}
                    className="rounded-md border border-border/50 bg-background/60 px-3 py-2"
                  >
                    <div className="flex items-center justify-between gap-2">
                      <span className="truncate text-sm font-medium">
                        {share.name}
                      </span>
                      <span className={cn("text-xs font-medium", tone.text)}>
                        {healthLabel(share.status, t)}
                      </span>
                    </div>
                    <p className="mt-1 text-xs text-muted-foreground">
                      {share.enabled
                        ? share.shareStatus
                        : t("settings.share.health.shareDisabled", {
                            defaultValue: "已停用",
                          })}
                      {share.tunnelStatus
                        ? ` · ${t("settings.share.health.tunnel", {
                            defaultValue: "隧道",
                          })} ${share.tunnelStatus}`
                        : ""}
                    </p>
                    {share.routerLastSyncError || share.tunnelError ? (
                      <p className="mt-1 text-xs text-red-600 dark:text-red-400">
                        {share.routerLastSyncError || share.tunnelError}
                      </p>
                    ) : null}
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
