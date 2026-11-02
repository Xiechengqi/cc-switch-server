import React from "react";
import { useTranslation } from "react-i18next";

import { SubscriptionQuotaView } from "@/components/SubscriptionQuotaFooter";
import { PROVIDER_TYPES } from "@/config/constants";
import { resolveManagedAccountIdentity } from "@/lib/authBinding";
import { subscriptionApi } from "@/lib/api/subscription";
import { refreshOauthQuotaAndReload } from "@/lib/query/oauthQuotaSnapshot";
import { useCodeBuddyOauthQuota } from "@/lib/query/subscription";
import type { ProviderMeta } from "@/types";

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
  const usageText =
    usage?.status === "complete"
      ? t("provider.codeBuddyUsageSummary", {
          defaultValue:
            "官方用量：今日 {{today}} · 7 日 {{week}} · 本月 {{month}} credits（{{count}} 次）",
          today: (usage.usageToday ?? 0).toFixed(2),
          week: (usage.usage7Days ?? 0).toFixed(2),
          month: (usage.usageThisMonth ?? 0).toFixed(2),
          count: usage.requestCount ?? 0,
        })
      : usage?.status === "unavailable"
        ? t("provider.codeBuddyUsageUnavailable", {
            defaultValue: "官方逐请求用量暂不可用",
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
