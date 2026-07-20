import React from "react";
import { Clock, RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import type { ProviderMeta } from "@/types";
import { useCursorOauthQuota } from "@/lib/query/subscription";
import { subscriptionApi } from "@/lib/api/subscription";
import { resolveManagedAccountId } from "@/lib/authBinding";
import { PROVIDER_TYPES } from "@/config/constants";
import type { AppId } from "@/lib/api";
import {
  formatQuotaSummary,
  formatRelativeTime,
  SubscriptionQuotaView,
  utilizationColor,
} from "@/components/SubscriptionQuotaFooter";
import {
  PROVIDER_REFRESH_TITLE_KEY,
  resolveQuotaQueriedAt,
} from "@/utils/providerQuotaUi";
import { ProviderQuotaMetaRow } from "@/components/providers/ProviderQuotaMetaRow";
import { extractErrorMessage } from "@/utils/errorUtils";

interface CursorOauthQuotaFooterProps {
  meta?: ProviderMeta;
  appId?: AppId;
  providerId?: string;
  inline?: boolean;
  isCurrent?: boolean;
}

const CursorOauthQuotaFooter: React.FC<CursorOauthQuotaFooterProps> = ({
  meta,
  appId,
  providerId,
  inline = false,
}) => {
  const { t, i18n } = useTranslation();
  const refreshTitle = t(PROVIDER_REFRESH_TITLE_KEY, {
    defaultValue: "供应商信息刷新",
  });
  const [lastManualRefreshAt, setLastManualRefreshAt] = React.useState<
    number | null
  >(null);
  const [manualRefreshLoading, setManualRefreshLoading] =
    React.useState(false);
  const isCursorApiKey = meta?.providerType === PROVIDER_TYPES.CURSOR_APIKEY;
  const authProvider = isCursorApiKey
    ? PROVIDER_TYPES.CURSOR_APIKEY
    : PROVIDER_TYPES.CURSOR_OAUTH;
  const {
    data: quota,
    isFetching: loading,
    refetch,
  } = useCursorOauthQuota(meta, { enabled: true, appId, providerId });
  const accountId = isCursorApiKey
    ? null
    : resolveManagedAccountId(meta, PROVIDER_TYPES.CURSOR_OAUTH);
  const handleRefresh = React.useCallback(async () => {
    if (manualRefreshLoading) return;
    setManualRefreshLoading(true);
    try {
      await subscriptionApi.refreshOauthQuota(
        authProvider,
        accountId,
        meta?.providerType,
        appId,
        providerId,
      );
      await refetch();
      setLastManualRefreshAt(Date.now());
    } finally {
      setManualRefreshLoading(false);
    }
  }, [
    accountId,
    appId,
    authProvider,
    manualRefreshLoading,
    meta?.providerType,
    providerId,
    refetch,
  ]);
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
    if (
      !displayQueriedAt &&
      !quota?.subscription?.expiresAt &&
      !quota?.tiers?.some((item) => item.resetsAt)
    ) {
      return;
    }
    const interval = setInterval(() => setNow(Date.now()), 30000);
    return () => clearInterval(interval);
  }, [displayQueriedAt, quota?.subscription?.expiresAt, quota?.tiers]);

  const membership = quota?.credentialMessage ?? undefined;
  const tier = quota?.tiers?.find(
    (item) =>
      item.name === "cursor_credits" || item.name === "cursor_included_usage",
  );
  const creditUsed = tier?.used;
  const creditLimit = tier?.limit;
  const hasCreditRange =
    typeof creditUsed === "number" &&
    Number.isFinite(creditUsed) &&
    typeof creditLimit === "number" &&
    Number.isFinite(creditLimit);

  // 无 usage tier 时（如 Stripe 成功但 Usage 接口失败），仍展示会员等级标签
  if (!quota?.success || !tier) {
    return (
      <SubscriptionQuotaView
        quota={quota && membership ? { ...quota, tiers: [] } : quota}
        loading={effectiveLoading}
        refetch={handleRefresh}
        appIdForExpiredHint="cursor_oauth"
        inline={inline}
      />
    );
  }

  const summaryText = formatQuotaSummary(quota, [tier], t, now);
  const used =
    typeof creditUsed === "number" && Number.isFinite(creditUsed)
      ? formatUsd(creditUsed, i18n.language)
      : null;
  const limit =
    typeof creditLimit === "number" && Number.isFinite(creditLimit)
      ? formatUsd(creditLimit, i18n.language)
      : null;
  const utilization = Math.round(tier.utilization);
  const resetDate = formatResetDate(tier.resetsAt, i18n.language);

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
            membership ? (
              <span className="text-[10px] font-medium text-foreground">
                {membership}
              </span>
            ) : undefined
          }
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
        <div className="min-w-0">
          <div className="text-xs text-gray-500 dark:text-gray-400 font-medium flex items-center gap-2">
            {membership && (
              <span className="rounded bg-muted px-1.5 py-0.5 text-[10px] font-semibold text-foreground">
                {membership}
              </span>
            )}
            {resetDate && (
              <span className="font-normal text-muted-foreground/70">
                {t("subscription.kiroResetsOn", {
                  date: resetDate,
                  defaultValue: `resets on ${resetDate}`,
                })}
              </span>
            )}
          </div>
        </div>
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

      <div className="mb-2 text-xs font-medium text-gray-700 dark:text-gray-200 break-words">
        {summaryText}
      </div>

      {hasCreditRange && (
        <div className="flex items-center gap-3 text-xs">
          <span className="text-gray-500 dark:text-gray-400 font-medium w-20 flex-shrink-0">
            {t("subscription.cursorCredits", { defaultValue: "Usage" })}
          </span>
          <div className="flex-1 min-w-0">
            <div className="h-2 bg-gray-100 dark:bg-gray-800 rounded-full overflow-hidden">
              <div
                className={`h-full rounded-full transition-all ${
                  tier.utilization >= 90
                    ? "bg-red-500"
                    : tier.utilization >= 70
                      ? "bg-orange-500"
                      : "bg-green-500"
                }`}
                style={{ width: `${Math.min(tier.utilization, 100)}%` }}
              />
            </div>
            <div className="mt-1 text-[10px] text-muted-foreground tabular-nums truncate">
              {used} / {limit}
            </div>
          </div>
          <span
            className={`font-semibold tabular-nums flex-shrink-0 ${utilizationColor(tier.utilization)}`}
          >
            {t("subscription.utilization", { value: utilization })}
          </span>
        </div>
      )}
    </div>
  );
};

function formatUsd(value: number, locale: string): string {
  return new Intl.NumberFormat(locale, {
    style: "currency",
    currency: "USD",
    maximumFractionDigits: value % 1 === 0 ? 0 : 2,
  }).format(value);
}

function formatResetDate(
  value: string | null | undefined,
  locale: string,
): string | null {
  if (!value) return null;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return null;
  return new Intl.DateTimeFormat(locale, {
    month: "2-digit",
    day: "2-digit",
  }).format(date);
}

export default CursorOauthQuotaFooter;
