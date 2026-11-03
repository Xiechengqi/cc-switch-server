import React from "react";
import { useTranslation } from "react-i18next";

import { SubscriptionQuotaView } from "@/components/SubscriptionQuotaFooter";
import { PROVIDER_TYPES } from "@/config/constants";
import { resolveManagedAccountIdentity } from "@/lib/authBinding";
import { subscriptionApi } from "@/lib/api/subscription";
import { refreshOauthQuotaAndReload } from "@/lib/query/oauthQuotaSnapshot";
import { useCodeBuddyOauthQuota } from "@/lib/query/subscription";
import type { ProviderMeta } from "@/types";
import type { SubscriptionQuota } from "@/types/subscription";

export function formatBuddyCollectedAt(timestamp?: number | null, timezone?: string): string | null {
  if (typeof timestamp !== "number" || timestamp <= 0 || !Number.isFinite(new Date(timestamp).getTime())
    || (timezone !== "UTC" && timezone !== "Asia/Shanghai")) return null;
  return `${new Intl.DateTimeFormat("sv-SE", {
    timeZone: timezone, year: "numeric", month: "2-digit", day: "2-digit",
    hour: "2-digit", minute: "2-digit", second: "2-digit", hourCycle: "h23",
  }).format(timestamp)} ${timezone === "UTC" ? "UTC" : "UTC+8"}`;
}

export function hasCompleteBuddyUsage(usage: SubscriptionQuota["providerUsage"]): boolean {
  return usage?.status === "complete"
    && [usage.usageToday, usage.usage7Days, usage.usageThisMonth, usage.usageTotal]
      .every((value) => typeof value === "number" && Number.isFinite(value) && value >= 0)
    && Number.isSafeInteger(usage.requestCount) && usage.requestCount! >= 0
    && /^\d{4}-\d{2}-\d{2}$/.test(usage.rangeStart ?? "")
    && /^\d{4}-\d{2}-\d{2}$/.test(usage.rangeEnd ?? "")
    && !!formatBuddyCollectedAt(usage.collectedAt, usage.timezone);
}

interface CodeBuddyOauthQuotaFooterProps {
  meta?: ProviderMeta;
  inline?: boolean;
}

const CodeBuddyOauthQuotaFooter: React.FC<CodeBuddyOauthQuotaFooterProps> = ({
  meta,
  inline = false,
}) => {
  const { t } = useTranslation();
  const product = meta?.providerType === PROVIDER_TYPES.WORKBUDDY_OAUTH
    ? PROVIDER_TYPES.WORKBUDDY_OAUTH : PROVIDER_TYPES.CODEBUDDY_OAUTH;
  const {
    data: quota,
    isFetching: loading,
    refetch,
  } = useCodeBuddyOauthQuota(meta, { enabled: true });
  const identity = resolveManagedAccountIdentity(
    meta,
    product,
  );
  const handleRefresh = React.useCallback(
    () => {
      if (!identity) return Promise.resolve();
      return refreshOauthQuotaAndReload(
        () =>
          subscriptionApi.refreshOauthQuota(
            product,
            identity?.accountId ?? null,
            meta?.providerType,
            undefined,
            undefined,
            true,
            identity?.authIdentityGeneration,
          ),
        () => refetch(),
      );
    },
    [identity?.accountId, identity?.authIdentityGeneration, product, meta?.providerType, refetch],
  );
  const usage = quota?.providerUsage;
  const usageTime = formatBuddyCollectedAt(usage?.collectedAt, usage?.timezone);
  const packageTime = formatBuddyCollectedAt(quota?.queriedAt, usage?.timezone ?? "UTC");
  const complete = hasCompleteBuddyUsage(usage);
  const usageText =
    complete
      ? t("provider.codeBuddyUsageSummary", {
          today: usage!.usageToday!.toFixed(2),
          week: usage!.usage7Days!.toFixed(2),
          month: usage!.usageThisMonth!.toFixed(2),
        })
      : usage
        ? t("provider.codeBuddyUsageUnavailable", {
            defaultValue: "官方逐请求用量暂不可用",
          })
        : null;

  return (
    <div className="space-y-1">
      {quota?.success && quota.tiers.some((tier) => /^codebuddy_package_[1-9]\d*$/.test(tier.name)) ? (
        <div className="text-xs text-muted-foreground">{t("provider.buddyPackageHeading")}</div>
      ) : null}
      <SubscriptionQuotaView
        quota={quota}
        loading={loading}
        refetch={handleRefresh}
        appIdForExpiredHint={product}
        inline={inline}
      />
      {quota?.success ? (
        <div className="text-xs text-muted-foreground">
          {t("provider.buddyPackageCollectedAt", { time: packageTime ?? t("provider.buddyTimeUnknown") })}
        </div>
      ) : null}
      {usageText ? (
        <div className="space-y-1 text-xs text-muted-foreground" title={usage?.error}>
          {usageText}
          {complete ? <div>{t("provider.buddyUsageRange", {
            start: usage!.rangeStart, end: usage!.rangeEnd, count: usage!.requestCount,
            timezone: usage!.timezone,
          })}</div> : null}
          <div>{t("provider.buddyUsageCollectedAt", { time: usageTime ?? t("provider.buddyTimeUnknown") })}</div>
          <div>{t("provider.buddyUsageScope")}</div>
        </div>
      ) : null}
    </div>
  );
};

export default CodeBuddyOauthQuotaFooter;
