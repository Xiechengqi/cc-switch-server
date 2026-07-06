import React from "react";
import type { ProviderMeta } from "@/types";
import { useAntigravityOauthQuota } from "@/lib/query/subscription";
import { subscriptionApi } from "@/lib/api/subscription";
import { resolveManagedAccountId } from "@/lib/authBinding";
import { PROVIDER_TYPES } from "@/config/constants";
import type { AppId } from "@/lib/api";
import { SubscriptionQuotaView } from "@/components/SubscriptionQuotaFooter";

interface AntigravityOauthQuotaFooterProps {
  meta?: ProviderMeta;
  appId?: AppId;
  providerId?: string;
  inline?: boolean;
  isCurrent?: boolean;
}

const AntigravityOauthQuotaFooter: React.FC<
  AntigravityOauthQuotaFooterProps
> = ({ meta, inline = false }) => {
  const {
    data: quota,
    isFetching: loading,
    refetch,
  } = useAntigravityOauthQuota(meta, { enabled: true });
  const accountId = resolveManagedAccountId(
    meta,
    PROVIDER_TYPES.ANTIGRAVITY_OAUTH,
  );
  const handleRefresh = React.useCallback(async () => {
    await subscriptionApi.refreshOauthQuota(
      "antigravity_oauth",
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
      appIdForExpiredHint="antigravity_oauth"
      inline={inline}
    />
  );
};

export default AntigravityOauthQuotaFooter;
