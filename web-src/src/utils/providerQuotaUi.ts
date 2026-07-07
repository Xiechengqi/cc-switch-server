/** Tooltip for provider card quota/usage refresh buttons. */
export const PROVIDER_REFRESH_TITLE_KEY = "provider.refreshProviderInfo";

export function resolveQuotaQueriedAt(
  queriedAt: number | null | undefined,
  manualRefreshAt: number | null,
): number | null {
  if (typeof queriedAt === "number" && Number.isFinite(queriedAt) && queriedAt > 0) {
    return queriedAt;
  }
  return manualRefreshAt;
}
