import React from "react";
import { Clock, RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ProviderMeta } from "@/types";
import { useKiroOauthQuota } from "@/lib/query/subscription";
import { subscriptionApi } from "@/lib/api/subscription";
import { resolveManagedAccountId } from "@/lib/authBinding";
import { PROVIDER_TYPES } from "@/config/constants";
import type { AppId } from "@/lib/api";
import {
  countdownStr,
  formatRelativeTime,
  SubscriptionQuotaView,
  utilizationColor,
} from "@/components/SubscriptionQuotaFooter";

interface KiroOauthQuotaFooterProps {
  meta?: ProviderMeta;
  appId?: AppId;
  providerId?: string;
  inline?: boolean;
  isCurrent?: boolean;
}

const KiroOauthQuotaFooter: React.FC<KiroOauthQuotaFooterProps> = ({
  meta,
  inline = false,
}) => {
  const { t, i18n } = useTranslation();
  const {
    data: quota,
    isFetching: loading,
    refetch,
  } = useKiroOauthQuota(meta, { enabled: true });
  const accountId = resolveManagedAccountId(meta, PROVIDER_TYPES.KIRO_OAUTH);
  const handleRefresh = React.useCallback(async () => {
    await subscriptionApi.refreshOauthQuota("kiro_oauth", accountId);
    await refetch();
  }, [accountId, refetch]);

  const [now, setNow] = React.useState(Date.now());
  React.useEffect(() => {
    if (!quota?.queriedAt) return;
    const interval = setInterval(() => setNow(Date.now()), 30000);
    return () => clearInterval(interval);
  }, [quota?.queriedAt]);

  const tier = quota?.tiers?.find(
    (item) => item.name === "kiro_agentic_requests",
  );
  const creditUsed = tier?.used;
  const creditLimit = tier?.limit;

  if (
    !quota?.success ||
    !tier ||
    typeof creditUsed !== "number" ||
    typeof creditLimit !== "number"
  ) {
    return (
      <SubscriptionQuotaView
        quota={quota}
        loading={loading}
        refetch={handleRefresh}
        appIdForExpiredHint="kiro_oauth"
        inline={inline}
        visibleTierNames={["kiro_agentic_requests"]}
      />
    );
  }

  const used = formatCredits(creditUsed, i18n.language);
  const limit = formatCredits(creditLimit, i18n.language);
  const utilization = Math.round(tier.utilization);
  const resetCountdown = countdownStr(tier.resetsAt);
  const resetDate = formatResetDate(tier.resetsAt, i18n.language);
  const credentialMessage = quota.credentialMessage?.trim();
  const usageText = t("subscription.kiroCreditsUsage", {
    used,
    limit,
    defaultValue: `${used} used / ${limit} covered in plan`,
  });
  const planTitle = credentialMessage
    ? formatKiroPlanTitle(credentialMessage)
    : null;

  if (inline) {
    return (
      <div className="flex flex-col items-end gap-1 text-xs whitespace-nowrap flex-shrink-0">
        <div className="flex items-center gap-2 justify-end">
          <span className="text-[10px] text-muted-foreground/70 flex items-center gap-1">
            <Clock size={10} />
            {quota.queriedAt
              ? formatRelativeTime(quota.queriedAt, now, t)
              : t("usage.never", { defaultValue: "从未更新" })}
          </span>
          <button
            onClick={(event) => {
              event.stopPropagation();
              handleRefresh();
            }}
            disabled={loading}
            className="p-1 rounded hover:bg-muted transition-colors disabled:opacity-50 flex-shrink-0 text-muted-foreground"
            title={t("subscription.refresh")}
          >
            <RefreshCw size={12} className={loading ? "animate-spin" : ""} />
          </button>
        </div>
        <div className="flex items-center gap-1.5">
          {planTitle && (
            <KiroPlanBadge title={credentialMessage ?? planTitle} label={planTitle} />
          )}
          <span className="font-medium tabular-nums text-foreground">
            {used}/{limit}
          </span>
          <span
            className={`font-semibold tabular-nums ${utilizationColor(tier.utilization)}`}
          >
            {t("subscription.utilization", { value: utilization })}
          </span>
          {resetCountdown && (
            <span className="text-muted-foreground/60 flex items-center gap-px">
              <Clock size={10} />
              {resetCountdown}
            </span>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="mt-3 rounded-xl border border-border-default bg-card px-4 py-3 shadow-sm">
      <div className="flex items-center justify-between mb-2">
        <div className="min-w-0">
          <div className="flex min-w-0 items-center gap-2">
            {planTitle && (
              <KiroPlanBadge title={credentialMessage ?? planTitle} label={planTitle} />
            )}
            <span className="min-w-0 truncate text-xs font-medium text-gray-500 dark:text-gray-400">
              {t("subscription.kiroEstimatedUsage", {
                defaultValue: "Estimated Usage",
              })}
            </span>
            {resetDate && (
              <span className="flex-shrink-0 text-xs font-normal text-muted-foreground/70">
                {t("subscription.kiroResetsOn", {
                  date: resetDate,
                  defaultValue: `resets on ${resetDate}`,
                })}
              </span>
            )}
          </div>
        </div>
        <div className="flex items-center gap-2">
          {quota.queriedAt && (
            <span className="text-[10px] text-muted-foreground/70 flex items-center gap-1">
              <Clock size={10} />
              {formatRelativeTime(quota.queriedAt, now, t)}
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

      <div className="flex items-center gap-3 text-xs">
        <span className="text-gray-500 dark:text-gray-400 font-medium w-20 flex-shrink-0">
          {t("subscription.kiroCredits", { defaultValue: "Credits" })}
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
            {usageText}
          </div>
        </div>
        <span
          className={`font-semibold tabular-nums flex-shrink-0 ${utilizationColor(tier.utilization)}`}
        >
          {t("subscription.utilization", { value: utilization })}
        </span>
      </div>
    </div>
  );
};

function KiroPlanBadge({ title, label }: { title: string; label: string }) {
  return (
    <span
      className="inline-flex max-w-28 flex-shrink-0 items-center rounded-md border border-blue-200 bg-blue-50 px-1.5 py-0.5 text-[10px] font-medium text-blue-700 dark:border-blue-700 dark:bg-blue-900/30 dark:text-blue-300"
      title={title}
    >
      <span className="min-w-0 truncate">{label}</span>
    </span>
  );
}

function formatKiroPlanTitle(value: string): string {
  return value
    .trim()
    .split(/\s+/)
    .map((word) => {
      const lower = word.toLowerCase();
      if (lower === "kiro") return "Kiro";
      if (lower === "oauth") return "OAuth";
      if (lower.startsWith("pro")) return `Pro${word.slice(3)}`;
      return `${word.charAt(0).toUpperCase()}${word.slice(1).toLowerCase()}`;
    })
    .join(" ");
}

function formatCredits(value: number, locale: string): string {
  return new Intl.NumberFormat(locale, {
    maximumFractionDigits: value % 1 === 0 ? 0 : 2,
    useGrouping: false,
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

export default KiroOauthQuotaFooter;
