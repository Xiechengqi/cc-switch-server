import type { CachedOauthQuota } from "@/lib/api/subscription";
import type { SubscriptionQuota } from "@/types/subscription";

export interface OauthQuotaSnapshot extends SubscriptionQuota {
  authProvider: string;
  accountId: string;
  authIdentityGeneration: number;
  refreshedAt: number | null;
  nextRefreshAt: number | null;
}

export interface ExpectedOauthQuotaIdentity {
  accountId: string;
  authIdentityGeneration: number;
}

export function oauthQuotaSnapshotFromEnvelope(
  envelope: CachedOauthQuota | null | undefined,
  expectedIdentity?: ExpectedOauthQuotaIdentity,
): OauthQuotaSnapshot | undefined {
  if (!envelope) return undefined;
  if (
    expectedIdentity &&
    (envelope.accountId !== expectedIdentity.accountId ||
      envelope.authIdentityGeneration !==
        expectedIdentity.authIdentityGeneration)
  ) {
    throw new Error("OAuth quota identity changed while loading");
  }

  return {
    ...envelope.quota,
    authProvider: envelope.authProvider,
    accountId: envelope.accountId,
    authIdentityGeneration: envelope.authIdentityGeneration,
    queriedAt: envelope.quota.queriedAt ?? envelope.refreshedAt,
    refreshedAt: envelope.refreshedAt,
    nextRefreshAt: envelope.nextRefreshAt,
  };
}

export function formatOauthQuotaRetryDelay(
  nextRefreshAt: number | null | undefined,
  nowMs = Date.now(),
): string | null {
  if (!nextRefreshAt || !Number.isFinite(nextRefreshAt)) return null;
  const remainingMs = nextRefreshAt - nowMs;
  if (remainingMs <= 0) return null;

  const seconds = Math.ceil(remainingMs / 1000);
  if (seconds < 60) return `${seconds}s`;

  const minutes = Math.ceil(seconds / 60);
  if (minutes < 60) return `${minutes}m`;

  const hours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  return remainingMinutes > 0 ? `${hours}h${remainingMinutes}m` : `${hours}h`;
}

export async function refreshOauthQuotaAndReload(
  refresh: () => Promise<unknown>,
  reload: () => Promise<unknown>,
): Promise<void> {
  let refreshFailed = false;
  let refreshError: unknown;

  try {
    await refresh();
  } catch (error) {
    refreshFailed = true;
    refreshError = error;
  }

  try {
    const result = await reload();
    if (
      result &&
      typeof result === "object" &&
      "isError" in result &&
      result.isError === true
    ) {
      throw (
        ("error" in result && result.error) ||
        new Error("OAuth quota reload failed")
      );
    }
  } catch (error) {
    if (!refreshFailed) throw error;
  }

  if (refreshFailed) throw refreshError;
}
