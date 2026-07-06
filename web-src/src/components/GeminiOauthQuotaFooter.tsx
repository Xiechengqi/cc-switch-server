import React from "react";
import type { ProviderMeta } from "@/types";
import { useGeminiOauthQuota } from "@/lib/query/subscription";
import { subscriptionApi } from "@/lib/api/subscription";
import { resolveManagedAccountId } from "@/lib/authBinding";
import { PROVIDER_TYPES } from "@/config/constants";
import type { AppId } from "@/lib/api";
import { SubscriptionQuotaView } from "@/components/SubscriptionQuotaFooter";

interface GeminiOauthQuotaFooterProps {
  meta?: ProviderMeta;
  appId?: AppId;
  providerId?: string;
  inline?: boolean;
  isCurrent?: boolean;
}

const GeminiOauthQuotaFooter: React.FC<GeminiOauthQuotaFooterProps> = ({
  meta,
  inline = false,
}) => {
  const {
    data: quota,
    isFetching: loading,
    refetch,
  } = useGeminiOauthQuota(meta, { enabled: true });
  const accountId = resolveManagedAccountId(
    meta,
    PROVIDER_TYPES.GOOGLE_GEMINI_OAUTH,
  );
  const handleRefresh = React.useCallback(async () => {
    await subscriptionApi.refreshOauthQuota("google_gemini_oauth", accountId);
    await refetch();
  }, [accountId, refetch]);

  return (
    <SubscriptionQuotaView
      quota={quota}
      loading={loading}
      refetch={handleRefresh}
      appIdForExpiredHint="google_gemini_oauth"
      inline={inline}
    />
  );
};

export default GeminiOauthQuotaFooter;
