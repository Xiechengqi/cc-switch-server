import { useQueryClient } from "@tanstack/react-query";
import {
  isStaleOauthQuotaAccountKey,
  oauthQuotaInvalidationKeys,
} from "@/lib/query/oauthQuotaKeys";

import { useServerEvent } from "./useServerEvent";

interface OauthQuotaUpdatedPayload {
  authProvider?: string;
  accountId?: string;
  authIdentityGeneration?: number | null;
  providerId?: string | null;
  appType?: string | null;
}

/**
 * Desktop emits `oauth-quota-updated` after background quota refresh; server uses
 * the same event name over SSE so provider footers invalidate cached quota.
 */
export function useOauthQuotaRefreshBridge() {
  const queryClient = useQueryClient();

  useServerEvent<OauthQuotaUpdatedPayload>("oauth-quota-updated", (payload) => {
    const authProvider = payload?.authProvider;
    if (!authProvider) {
      return;
    }

    if (
      payload.accountId &&
      payload.authIdentityGeneration != null
    ) {
      queryClient.removeQueries({
        predicate: (query) =>
          isStaleOauthQuotaAccountKey(
            query.queryKey,
            authProvider,
            payload.accountId!,
            payload.authIdentityGeneration!,
          ),
      });
    }

    for (const queryKey of oauthQuotaInvalidationKeys({
      authProvider,
      accountId: payload?.accountId,
      authIdentityGeneration: payload?.authIdentityGeneration,
      providerId: payload?.providerId,
      appType: payload?.appType,
    })) {
      void queryClient.invalidateQueries({
        queryKey,
      });
    }
  });
}
