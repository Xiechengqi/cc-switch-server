import { useEffect, useState } from "react";
import { Shield } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import {
  loadUpgradePolicy,
  saveUpgradePolicy,
  type UpgradePolicy,
} from "@/lib/server-legacy-api";

export function ServerUpgradePolicySettings() {
  const { t } = useTranslation();
  const [policy, setPolicy] = useState<UpgradePolicy | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    loadUpgradePolicy()
      .then(setPolicy)
      .catch((error) => {
        console.error("Failed to load upgrade policy:", error);
      });
  }, []);

  async function updatePolicy(patch: Partial<UpgradePolicy>) {
    if (!policy) return;
    const previous = policy;
    const next = { ...policy, ...patch };
    setPolicy(next);
    setBusy(true);
    try {
      const saved = await saveUpgradePolicy(next);
      setPolicy(saved);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
      setPolicy(previous);
    } finally {
      setBusy(false);
    }
  }

  if (!policy) return null;

  return (
    <section className="space-y-4 rounded-xl border border-border bg-card/50 p-4">
      <div className="flex items-center gap-2">
        <Shield className="h-4 w-4 text-muted-foreground" />
        <div>
          <h3 className="text-sm font-medium">
            {t("settings.upgradePolicy.title", { defaultValue: "升级策略" })}
          </h3>
          <p className="text-xs text-muted-foreground">
            {t("settings.upgradePolicy.description", {
              defaultValue:
                "控制 Router 代升级与后台自动检查更新行为。",
            })}
          </p>
        </div>
      </div>

      <div className="flex items-center justify-between gap-4">
        <div className="space-y-1">
          <Label>
            {t("settings.upgradePolicy.delegateTitle", {
              defaultValue: "授权 Router Owner 代升级",
            })}
          </Label>
          <p className="text-xs text-muted-foreground">
            {t("settings.upgradePolicy.delegateDescription", {
              defaultValue:
                "开启后，Router Web 中 client owner 可对这台 server 执行强制升级。",
            })}
          </p>
        </div>
        <Switch
          checked={policy.delegateUpgradeToRouterOwner}
          disabled={busy}
          onCheckedChange={(checked) =>
            updatePolicy({ delegateUpgradeToRouterOwner: checked })
          }
        />
      </div>

      <div className="flex items-center justify-between gap-4">
        <div className="space-y-1">
          <Label>
            {t("settings.upgradePolicy.autoUpgradeTitle", {
              defaultValue: "自动升级",
            })}
          </Label>
          <p className="text-xs text-muted-foreground">
            {t("settings.upgradePolicy.autoUpgradeDescription", {
              defaultValue:
                "定时检查 GitHub Releases，仅当本地版本落后于最新版本时才执行升级。",
            })}
          </p>
        </div>
        <Switch
          checked={policy.autoUpgradeEnabled}
          disabled={busy}
          onCheckedChange={(checked) =>
            updatePolicy({ autoUpgradeEnabled: checked })
          }
        />
      </div>

      <div className="flex items-center justify-between gap-4">
        <div className="space-y-1">
          <Label>
            {t("settings.upgradePolicy.intervalTitle", {
              defaultValue: "自动检查间隔（分钟）",
            })}
          </Label>
          <p className="text-xs text-muted-foreground">
            {t("settings.upgradePolicy.intervalDescription", {
              defaultValue: "默认 60 分钟，最小 5 分钟。",
            })}
          </p>
        </div>
        <Input
          type="number"
          min={5}
          max={1440}
          className="w-28"
          disabled={busy || !policy.autoUpgradeEnabled}
          value={policy.autoUpgradeCheckIntervalMinutes}
          onChange={(event) => {
            const value = Number.parseInt(event.target.value, 10);
            if (!Number.isFinite(value)) return;
            setPolicy({
              ...policy,
              autoUpgradeCheckIntervalMinutes: value,
            });
          }}
          onBlur={() => {
            const minutes = Math.min(
              1440,
              Math.max(5, policy.autoUpgradeCheckIntervalMinutes || 60),
            );
            if (minutes !== policy.autoUpgradeCheckIntervalMinutes) {
              void updatePolicy({ autoUpgradeCheckIntervalMinutes: minutes });
            }
          }}
        />
      </div>
    </section>
  );
}
