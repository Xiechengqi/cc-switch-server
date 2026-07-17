import type { Settings, UpgradePolicy } from "@/types";

/** Mirrors `UpgradePolicyConfig::default()` in `src/domain/settings/config.rs`. */
export const DEFAULT_UPGRADE_POLICY: UpgradePolicy = {
  delegateUpgradeToRouterOwner: true,
  autoUpgradeEnabled: false,
  autoUpgradeCheckIntervalMinutes: 60,
};

export function selectUpgradePolicy(
  settings?: Pick<Settings, "upgradePolicy"> | null,
): UpgradePolicy {
  return settings?.upgradePolicy ?? DEFAULT_UPGRADE_POLICY;
}
