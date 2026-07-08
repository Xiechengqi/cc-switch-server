import React from "react";
import { RefreshCw, AlertCircle, Clock } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ProviderMeta } from "@/types";
import type { AppId } from "@/lib/api";
import { useOllamaQuota } from "@/lib/query/ollama";
import { subscriptionApi } from "@/lib/api/subscription";
import { useQueryClient } from "@tanstack/react-query";
import { formatExpireDistance } from "@/components/SubscriptionQuotaFooter";
import {
  PROVIDER_REFRESH_TITLE_KEY,
  resolveQuotaQueriedAt,
} from "@/utils/providerQuotaUi";
import { ProviderQuotaMetaRow } from "@/components/providers/ProviderQuotaMetaRow";

interface OllamaQuotaFooterProps {
  meta?: ProviderMeta;
  providerId: string;
  appId: AppId;
  inline?: boolean;
  isCurrent?: boolean;
  showInUse?: boolean;
}

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

const OllamaQuotaFooter: React.FC<OllamaQuotaFooterProps> = ({
  providerId,
  appId,
  inline = false,
  showInUse = false,
}) => {
  const { t } = useTranslation();
  const refreshTitle = t(PROVIDER_REFRESH_TITLE_KEY, {
    defaultValue: "供应商信息刷新",
  });
  const [lastManualRefreshAt, setLastManualRefreshAt] = React.useState<
    number | null
  >(null);
  const queryClient = useQueryClient();

  const {
    data: cached,
    isFetching: loading,
    refetch,
  } = useOllamaQuota(providerId, { enabled: true, appId });

  const handleRefresh = React.useCallback(async () => {
    setLastManualRefreshAt(Date.now());
    await subscriptionApi.refreshOauthQuota(
      "ollama_cloud",
      providerId,
      "ollama_cloud",
      appId,
      providerId,
    );
    await refetch();
    queryClient.invalidateQueries({
      queryKey: ["ollama", "quota", providerId],
    });
  }, [refetch, queryClient, providerId, appId]);

  const displayQueriedAt = resolveQuotaQueriedAt(
    cached?.refreshedAt ?? cached?.quota?.queriedAt ?? null,
    lastManualRefreshAt,
  );

  const [now, setNow] = React.useState(Date.now());
  React.useEffect(() => {
    const serverQueriedAt = cached?.refreshedAt ?? cached?.quota?.queriedAt;
    if (serverQueriedAt && serverQueriedAt > 0) {
      setLastManualRefreshAt(null);
    }
  }, [cached?.refreshedAt, cached?.quota?.queriedAt]);

  React.useEffect(() => {
    if (
      !displayQueriedAt &&
      !cached?.quota?.subscription?.expiresAt &&
      !cached?.quota?.tiers?.some((tier) => tier.resetsAt)
    ) {
      return;
    }
    const interval = setInterval(() => setNow(Date.now()), 30000);
    return () => clearInterval(interval);
  }, [
    displayQueriedAt,
    cached?.quota?.subscription?.expiresAt,
    cached?.quota?.tiers,
  ]);

  if (!cached) return null;

  const quota = cached.quota;

  if (!quota.success) {
    if (inline) {
      return (
        <div className="inline-flex items-center gap-2 text-xs rounded-lg border border-border-default bg-card px-3 py-2 shadow-sm">
          <div className="flex items-center gap-1.5 text-red-500 dark:text-red-400">
            <AlertCircle size={12} />
            <span>{quota.error || t("subscription.queryFailed")}</span>
          </div>
          <button
            onClick={() => handleRefresh()}
            disabled={loading}
            className="p-1 rounded hover:bg-muted transition-colors disabled:opacity-50 flex-shrink-0"
            title={refreshTitle}
          >
            <RefreshCw size={12} className={loading ? "animate-spin" : ""} />
          </button>
        </div>
      );
    }
    return null;
  }

  const plan =
    quota.subscription?.planLabel || quota.credentialMessage || "unknown";
  const email = quota.tiers[0]?.name || "";
  const periodEnd = quota.subscription?.expiresAt || quota.tiers[0]?.resetsAt;
  const summaryText = [plan, formatExpireDistance(periodEnd)]
    .filter(Boolean)
    .join(" · ");

  if (inline) {
    return (
      <div className="flex flex-col items-end gap-1 text-xs whitespace-nowrap flex-shrink-0">
        <ProviderQuotaMetaRow
          showInUse={showInUse}
          timeLabel={
            displayQueriedAt
              ? formatRelativeTime(displayQueriedAt, now, t)
              : t("provider.quotaNeverUpdated", { defaultValue: "从未更新" })
          }
          loading={loading}
          onRefresh={(event) => {
            event.stopPropagation();
            void handleRefresh();
          }}
          refreshTitle={refreshTitle}
        />
        <div className="min-w-0 max-w-full text-right text-[10px] font-medium text-foreground break-words">
          {summaryText}
        </div>
      </div>
    );
  }

  return (
    <div className="mt-3 rounded-xl border border-border-default bg-card px-4 py-3 shadow-sm">
      <div className="flex items-center justify-between mb-2">
        <span className="text-xs text-gray-500 dark:text-gray-400 font-medium">
          {t("subscription.title", { defaultValue: "Subscription" })}
        </span>
        <div className="flex items-center gap-2">
          {displayQueriedAt && (
            <span className="text-[10px] text-muted-foreground/70 flex items-center gap-1">
              <Clock size={10} />
              {formatRelativeTime(displayQueriedAt, now, t)}
            </span>
          )}
          <button
            onClick={() => handleRefresh()}
            disabled={loading}
            className="p-1 rounded hover:bg-muted transition-colors disabled:opacity-50"
            title={refreshTitle}
          >
            <RefreshCw size={12} className={loading ? "animate-spin" : ""} />
          </button>
        </div>
      </div>
      <div className="flex flex-col gap-1 text-xs">
        <span className="font-semibold">{summaryText}</span>
        {email && (
          <span className="text-muted-foreground truncate" title={email}>
            {email}
          </span>
        )}
      </div>
    </div>
  );
};

export default OllamaQuotaFooter;
