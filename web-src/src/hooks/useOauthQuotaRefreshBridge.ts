import { useQueryClient } from "@tanstack/react-query";

import { useServerEvent } from "./useServerEvent";

interface OauthQuotaUpdatedPayload {
  authProvider?: string;
  accountId?: string;
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
    const accountId = payload?.accountId ?? "default";
    if (!authProvider) {
      return;
    }

    const key =
      authProvider === "github_copilot"
        ? ["copilot", "quota", accountId]
        : [authProvider, "quota", accountId];
    void queryClient.invalidateQueries({ queryKey: key });

    if (
      authProvider === "cursor_apikey" &&
      payload?.providerId &&
      payload?.appType
    ) {
      void queryClient.invalidateQueries({
        queryKey: [
          "cursor_apikey",
          "quota",
          payload.providerId,
          payload.appType,
        ],
      });
    }

    if (accountId !== "default") {
      const defaultKey =
        authProvider === "github_copilot"
          ? ["copilot", "quota", "default"]
          : [authProvider, "quota", "default"];
      void queryClient.invalidateQueries({ queryKey: defaultKey });
    }
  });
}
