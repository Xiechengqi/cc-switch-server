import React from "react";
import type { ProviderMeta } from "@/types";
import {
  resolveCodexQuotaAuthProvider,
  useCodexOauthQuota,
} from "@/lib/query/subscription";
import { subscriptionApi } from "@/lib/api/subscription";
import { resolveManagedAccountIdentity } from "@/lib/authBinding";
import type { AppId } from "@/lib/api";
import { SubscriptionQuotaView } from "@/components/SubscriptionQuotaFooter";
import { refreshOauthQuotaAndReload } from "@/lib/query/oauthQuotaSnapshot";

interface CodexOauthQuotaFooterProps {
  meta?: ProviderMeta;
  appId?: AppId;
  providerId?: string;
  inline?: boolean;
  /** 是否为当前激活的供应商 */
  isCurrent?: boolean;
}

/**
 * Codex OAuth (ChatGPT Plus/Pro 反代) 订阅额度 footer
 *
 * 复用 SubscriptionQuotaView 的全部渲染逻辑（5 状态 × inline/expanded）。
 * 数据源切换为 cc-switch 自管的 OAuth token 而非 Codex CLI 凭据。
 */
const CodexOauthQuotaFooter: React.FC<CodexOauthQuotaFooterProps> = ({
  meta,
  inline = false,
}) => {
  const {
    data: quota,
    isFetching: loading,
    refetch,
  } = useCodexOauthQuota(meta, { enabled: true });
  const authProvider = resolveCodexQuotaAuthProvider();
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

export default CodexOauthQuotaFooter;
