import type { Settings } from "@/types";

export const DEFAULT_OAUTH_QUOTA_REFRESH_INTERVAL_MINUTES = 30;

export function getOauthQuotaRefreshIntervalMinutes(
  settings: Settings | undefined,
): number {
  const raw = settings?.oauthQuotaRefreshIntervalMinutes;
  if (!Number.isFinite(raw) || raw == null) {
    return DEFAULT_OAUTH_QUOTA_REFRESH_INTERVAL_MINUTES;
  }
  return Math.max(1, Math.floor(raw));
}

export function getOauthQuotaRefreshIntervalMs(
  settings: Settings | undefined,
): number {
  return getOauthQuotaRefreshIntervalMinutes(settings) * 60 * 1000;
}
