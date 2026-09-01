import type { Settings } from "@/types";
import { DEFAULT_UPGRADE_POLICY } from "@/lib/upgradePolicyDefaults";

/** Mirrors `default_ui_settings()` in `src/core/ui_settings.rs` for server web fallback. */
export const SERVER_DEFAULT_SETTINGS: Settings = {
  oauthQuotaRefreshIntervalMinutes: 30,
  oauthQuotaRefreshTimeoutSeconds: 10,
  language: "zh",
  backupIntervalHours: 12,
  backupRetainCount: 3,
  upgradePolicy: DEFAULT_UPGRADE_POLICY,
};
