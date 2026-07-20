import React from "react";
import type { ProviderMeta } from "@/types";
import { useGrokOauthQuota } from "@/lib/query/subscription";
import { subscriptionApi } from "@/lib/api/subscription";
import { resolveManagedAccountId } from "@/lib/authBinding";
import { PROVIDER_TYPES } from "@/config/constants";
import type { AppId } from "@/lib/api";
import { SubscriptionQuotaView } from "@/components/SubscriptionQuotaFooter";

interface GrokOauthQuotaFooterProps {
  meta?: ProviderMeta;
  appId?: AppId;
  providerId?: string;
  inline?: boolean;
  isCurrent?: boolean;
}

const GrokOauthQuotaFooter: React.FC<GrokOauthQuotaFooterProps> = ({
  meta,
  inline = false,
}) => {
  const {
    data: quota,
    isFetching: loading,
    refetch,
  } = useGrokOauthQuota(meta, { enabled: true });
  const accountId = resolveManagedAccountId(meta, PROVIDER_TYPES.GROK_OAUTH);
  const handleRefresh = React.useCallback(async () => {
    await subscriptionApi.refreshOauthQuota(
      PROVIDER_TYPES.GROK_OAUTH,
      accountId,
      meta?.providerType,
    );
    await refetch();
  }, [accountId, meta?.providerType, refetch]);

  return (
    <SubscriptionQuotaView
      quota={quota}
      loading={loading}
      refetch={handleRefresh}
      appIdForExpiredHint={PROVIDER_TYPES.GROK_OAUTH}
      inline={inline}
    />
  );
};

export default GrokOauthQuotaFooter;
