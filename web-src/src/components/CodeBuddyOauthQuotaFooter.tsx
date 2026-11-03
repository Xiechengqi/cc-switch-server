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

export function hasCompleteBuddyUsage(usage: SubscriptionQuota["providerUsage"]): boolean {
  return usage?.status === "complete"
    && [usage.usageToday, usage.usage7Days, usage.usageThisMonth, usage.usageTotal]
      .every((value) => typeof value === "number" && Number.isFinite(value) && value >= 0)
    && Number.isSafeInteger(usage.requestCount) && usage.requestCount! >= 0
    && /^\d{4}-\d{2}-\d{2}$/.test(usage.rangeStart ?? "")
    && /^\d{4}-\d{2}-\d{2}$/.test(usage.rangeEnd ?? "")
    && typeof usage.collectedAt === "number" && usage.collectedAt > 0
    && Number.isFinite(new Date(usage.collectedAt).getTime())
    && (usage.timezone === "UTC" || usage.timezone === "Asia/Shanghai");
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
  const complete = hasCompleteBuddyUsage(usage);
  const usageText =
    complete
      ? t("provider.codeBuddyUsageSummary", {
          today: usage!.usageToday!.toFixed(2),
          month: usage!.usageThisMonth!.toFixed(2),
        })
      : usage
        ? t("provider.codeBuddyUsageUnavailable", {
            defaultValue: "官方用量暂不可用",
          })
        : null;

  return (
    <div className="space-y-1">
      <SubscriptionQuotaView
        quota={quota}
        loading={loading}
        refetch={handleRefresh}
        appIdForExpiredHint={product}
        inline={inline}
      />
      {usageText ? (
        <div className="text-xs text-muted-foreground" title={usage?.error}>
          {usageText}
        </div>
      ) : null}
    </div>
  );
};

export default CodeBuddyOauthQuotaFooter;
