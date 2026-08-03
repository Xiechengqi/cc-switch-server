import React from "react";
import type { ProviderMeta } from "@/types";
import {
  useAntigravityOauthQuota,
  type AntigravityQuotaAuthProvider,
} from "@/lib/query/subscription";
import { subscriptionApi } from "@/lib/api/subscription";
import { resolveManagedAccountIdentity } from "@/lib/authBinding";
import type { AppId } from "@/lib/api";
import { SubscriptionQuotaView } from "@/components/SubscriptionQuotaFooter";
import { refreshOauthQuotaAndReload } from "@/lib/query/oauthQuotaSnapshot";

interface AntigravityOauthQuotaFooterProps {
  meta?: ProviderMeta;
  appId?: AppId;
  providerId?: string;
  inline?: boolean;
  isCurrent?: boolean;
  authProvider: AntigravityQuotaAuthProvider;
}

const AntigravityOauthQuotaFooter: React.FC<
  AntigravityOauthQuotaFooterProps
> = ({ meta, inline = false, authProvider }) => {
  const {
    data: quota,
    isFetching: loading,
    refetch,
  } = useAntigravityOauthQuota(meta, authProvider, { enabled: true });
  const identity = resolveManagedAccountIdentity(meta, authProvider);
  const accountId = identity?.accountId ?? null;
  const handleRefresh = React.useCallback(
    () =>
      refreshOauthQuotaAndReload(
        () =>
          subscriptionApi.refreshOauthQuota(
            authProvider,
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
      authProvider,
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
      appIdForExpiredHint={authProvider}
      inline={inline}
    />
  );
};

export default AntigravityOauthQuotaFooter;
