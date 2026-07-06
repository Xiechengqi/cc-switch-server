import React from "react";
import { RefreshCw, AlertCircle, Clock } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ProviderMeta } from "@/types";
import type { AppId } from "@/lib/api";
import { useOllamaQuota } from "@/lib/query/ollama";
import { subscriptionApi } from "@/lib/api/subscription";
import { useQueryClient } from "@tanstack/react-query";
import { formatExpireDistance } from "@/components/SubscriptionQuotaFooter";

interface OllamaQuotaFooterProps {
  meta?: ProviderMeta;
  providerId: string;
  appId: AppId;
  inline?: boolean;
  isCurrent?: boolean;
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
}) => {
  const { t } = useTranslation();
  const queryClient = useQueryClient();

  const {
    data: cached,
    isFetching: loading,
    refetch,
  } = useOllamaQuota(providerId, { enabled: true, appId });

  const handleRefresh = React.useCallback(async () => {
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
  }, [refetch, queryClient, providerId]);

  const [now, setNow] = React.useState(Date.now());
  React.useEffect(() => {
    if (
      !cached?.refreshedAt &&
      !cached?.quota?.subscription?.expiresAt &&
      !cached?.quota?.tiers?.some((tier) => tier.resetsAt)
    ) {
      return;
    }
    const interval = setInterval(() => setNow(Date.now()), 30000);
    return () => clearInterval(interval);
  }, [
    cached?.refreshedAt,
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
            title={t("subscription.refresh")}
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
        <div className="flex items-center gap-2 justify-end">
          <span className="text-[10px] text-muted-foreground/70 flex items-center gap-1">
            <Clock size={10} />
            {cached.refreshedAt
              ? formatRelativeTime(cached.refreshedAt, now, t)
              : t("usage.never", { defaultValue: "Never" })}
          </span>
          <button
            onClick={(e) => {
              e.stopPropagation();
              handleRefresh();
            }}
            disabled={loading}
            className="p-1 rounded hover:bg-muted transition-colors disabled:opacity-50 flex-shrink-0 text-muted-foreground"
            title={t("subscription.refresh")}
          >
            <RefreshCw size={12} className={loading ? "animate-spin" : ""} />
          </button>
        </div>
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
          {cached.refreshedAt && (
            <span className="text-[10px] text-muted-foreground/70 flex items-center gap-1">
              <Clock size={10} />
              {formatRelativeTime(cached.refreshedAt, now, t)}
            </span>
          )}
          <button
            onClick={() => handleRefresh()}
            disabled={loading}
            className="p-1 rounded hover:bg-muted transition-colors disabled:opacity-50"
            title={t("subscription.refresh")}
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
