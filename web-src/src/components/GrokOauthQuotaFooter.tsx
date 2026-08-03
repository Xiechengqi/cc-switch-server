import React from "react";
import type { ProviderMeta } from "@/types";
import { useGrokOauthQuota } from "@/lib/query/subscription";
import { subscriptionApi } from "@/lib/api/subscription";
import { resolveManagedAccountIdentity } from "@/lib/authBinding";
import { PROVIDER_TYPES } from "@/config/constants";
import type { AppId } from "@/lib/api";
import { SubscriptionQuotaView } from "@/components/SubscriptionQuotaFooter";
import { refreshOauthQuotaAndReload } from "@/lib/query/oauthQuotaSnapshot";

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
  const identity = resolveManagedAccountIdentity(
    meta,
    PROVIDER_TYPES.GROK_OAUTH,
  );
  const accountId = identity?.accountId ?? null;
  const handleRefresh = React.useCallback(
    () =>
      refreshOauthQuotaAndReload(
        () =>
          subscriptionApi.refreshOauthQuota(
            PROVIDER_TYPES.GROK_OAUTH,
            accountId,
            meta?.providerType,
            undefined,
            undefined,
            true,
            identity?.authIdentityGeneration,
          ),
        () => refetch(),
      ),
    [
      accountId,
      identity?.authIdentityGeneration,
      meta?.providerType,
      refetch,
    ],
  );

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
