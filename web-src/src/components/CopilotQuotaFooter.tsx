import React from "react";
import { RefreshCw, AlertCircle, Clock } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import type { ProviderMeta } from "@/types";
import { useCopilotQuota } from "@/lib/query/copilot";
import { subscriptionApi } from "@/lib/api/subscription";
import type { AppId } from "@/lib/api";
import { resolveManagedAccountId } from "@/lib/authBinding";
import { PROVIDER_TYPES } from "@/config/constants";
import {
  TierBadge,
  utilizationColor,
} from "@/components/SubscriptionQuotaFooter";
import {
  PROVIDER_REFRESH_TITLE_KEY,
  resolveQuotaQueriedAt,
} from "@/utils/providerQuotaUi";
import { ProviderQuotaMetaRow } from "@/components/providers/ProviderQuotaMetaRow";
import { extractErrorMessage } from "@/utils/errorUtils";

interface CopilotQuotaFooterProps {
  meta?: ProviderMeta;
  appId?: AppId;
  providerId?: string;
  inline?: boolean;
  /** 是否为当前激活的供应商 */
  isCurrent?: boolean;
}

/** 格式化相对时间 */
function formatRelativeTime(
  timestamp: number,
  now: number,
  t: (key: string, options?: { count?: number }) => string,
): string {
  const diff = Math.floor((now - timestamp) / 1000);
  if (diff < 60) return t("usage.justNow");
  if (diff < 3600)
    return t("usage.minutesAgo", { count: Math.floor(diff / 60) });
  if (diff < 86400)
    return t("usage.hoursAgo", { count: Math.floor(diff / 3600) });
  return t("usage.daysAgo", { count: Math.floor(diff / 86400) });
}

const CopilotQuotaFooter: React.FC<CopilotQuotaFooterProps> = ({
  meta,
  inline = false,
}) => {
  const { t } = useTranslation();
  const refreshTitle = t(PROVIDER_REFRESH_TITLE_KEY, {
    defaultValue: "供应商信息刷新",
  });
  const [lastManualRefreshAt, setLastManualRefreshAt] = React.useState<
    number | null
  >(null);
  const [manualRefreshLoading, setManualRefreshLoading] =
    React.useState(false);
  const accountId = resolveManagedAccountId(
    meta,
    PROVIDER_TYPES.GITHUB_COPILOT,
  );

  const {
    data: quota,
    isFetching: loading,
    refetch,
  } = useCopilotQuota(accountId, { enabled: true });
  const handleRefresh = React.useCallback(async () => {
    if (manualRefreshLoading) return;
    setManualRefreshLoading(true);
    try {
      await subscriptionApi.refreshOauthQuota("github_copilot", accountId);
      await refetch();
      setLastManualRefreshAt(Date.now());
    } finally {
      setManualRefreshLoading(false);
    }
  }, [accountId, manualRefreshLoading, refetch]);
  const effectiveLoading = loading || manualRefreshLoading;
  const reportRefreshError = React.useCallback(
    (error: unknown) =>
      toast.error(extractErrorMessage(error) || t("subscription.queryFailed")),
    [t],
  );

  const displayQueriedAt = resolveQuotaQueriedAt(
    quota?.queriedAt,
    lastManualRefreshAt,
  );

  const [now, setNow] = React.useState(Date.now());
  React.useEffect(() => {
    if (quota?.queriedAt && quota.queriedAt > 0) {
      setLastManualRefreshAt(null);
    }
  }, [quota?.queriedAt]);

  React.useEffect(() => {
    if (!displayQueriedAt) return;
    const interval = setInterval(() => setNow(Date.now()), 30000);
    return () => clearInterval(interval);
  }, [displayQueriedAt]);

  if (!quota) return null;

  // API 调用失败
  if (!quota.success) {
    if (inline) {
      return (
        <div className="inline-flex items-center gap-2 text-xs rounded-lg border border-border-default bg-card px-3 py-2 shadow-sm">
          <div className="flex items-center gap-1.5 text-red-500 dark:text-red-400">
            <AlertCircle size={12} />
            <span>{quota.error || t("subscription.queryFailed")}</span>
          </div>
          <button
            onClick={() => void handleRefresh().catch(reportRefreshError)}
            disabled={effectiveLoading}
            className="p-1 rounded hover:bg-muted transition-colors disabled:opacity-50 flex-shrink-0"
            title={refreshTitle}
          >
            <RefreshCw
              size={12}
              className={effectiveLoading ? "animate-spin" : ""}
            />
          </button>
        </div>
      );
    }
    return null;
  }

  const tiers = quota.tiers;
  if (tiers.length === 0) return null;

  if (inline) {
    return (
      <div className="flex flex-col items-end gap-1 text-xs whitespace-nowrap flex-shrink-0">
        <ProviderQuotaMetaRow
          timeLabel={
            displayQueriedAt
              ? formatRelativeTime(displayQueriedAt, now, t)
              : t("provider.quotaNeverUpdated", { defaultValue: "从未更新" })
          }
          loading={effectiveLoading}
          onRefresh={(event) => {
            event.stopPropagation();
            void handleRefresh().catch(reportRefreshError);
          }}
          refreshTitle={refreshTitle}
          leading={
            quota.plan ? (
              <span className="text-[10px] text-muted-foreground/70">
                {quota.plan}
              </span>
            ) : undefined
          }
        />

        <div className="flex items-center gap-2">
          {tiers.map((tier) => (
            <TierBadge key={tier.name} tier={tier} t={t} />
          ))}
        </div>
      </div>
    );
  }

  // 展开模式
  return (
    <div className="mt-3 rounded-xl border border-border-default bg-card px-4 py-3 shadow-sm">
      <div className="flex items-center justify-between mb-2">
        <span className="text-xs text-gray-500 dark:text-gray-400 font-medium">
          {quota.plan || t("subscription.title")}
        </span>
        <div className="flex items-center gap-2">
          {displayQueriedAt && (
            <span className="text-[10px] text-muted-foreground/70 flex items-center gap-1">
              <Clock size={10} />
              {formatRelativeTime(displayQueriedAt, now, t)}
            </span>
          )}
          <button
            onClick={() => void handleRefresh().catch(reportRefreshError)}
            disabled={effectiveLoading}
            className="p-1 rounded hover:bg-muted transition-colors disabled:opacity-50"
            title={refreshTitle}
          >
            <RefreshCw
              size={12}
              className={effectiveLoading ? "animate-spin" : ""}
            />
          </button>
        </div>
      </div>

      <div className="flex flex-col gap-2">
        {tiers.map((tier) => {
          const label = t("subscription.copilotPremium", {
            defaultValue: "Premium",
          });
          return (
            <div key={tier.name} className="flex items-center gap-3 text-xs">
              <span
                className="text-gray-500 dark:text-gray-400 min-w-0 font-medium"
                style={{ width: "25%" }}
              >
                {label}
              </span>
              <div className="flex-1 h-2 bg-gray-100 dark:bg-gray-800 rounded-full overflow-hidden">
                <div
                  className={`h-full rounded-full transition-all ${
                    tier.utilization >= 90
                      ? "bg-red-500"
                      : tier.utilization >= 70
                        ? "bg-orange-500"
                        : "bg-green-500"
                  }`}
                  style={{
                    width: `${Math.min(tier.utilization, 100)}%`,
                  }}
                />
              </div>
              <span
                className={`font-semibold tabular-nums ${utilizationColor(tier.utilization)}`}
              >
                {Math.round(tier.utilization)}%
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
};

export default CopilotQuotaFooter;
