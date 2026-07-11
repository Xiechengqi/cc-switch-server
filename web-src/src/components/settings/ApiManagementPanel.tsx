import { useEffect, useMemo, useState } from "react";
import {
  Check,
  Copy,
  KeyRound,
  RotateCcw,
  ShieldAlert,
  Trash2,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { settingsApi, type ApiManagementConfig } from "@/lib/api/settings";

const DEFAULT_CONFIG: ApiManagementConfig = {
  diagnosticsEnabled: true,
  logEnabled: false,
  restartEnabled: false,
  upgradeEnabled: false,
  logTailLines: 100,
  tokenConfigured: false,
  tokenExpiresAtMs: null,
};

type DangerousCapability = "restartEnabled" | "upgradeEnabled";

export function ApiManagementPanel() {
  const { t } = useTranslation();
  const [config, setConfig] = useState(DEFAULT_CONFIG);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [ttlHours, setTtlHours] = useState(1);
  const [visibleToken, setVisibleToken] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [confirmCapability, setConfirmCapability] =
    useState<DangerousCapability | null>(null);

  useEffect(() => {
    settingsApi
      .getApiManagement()
      .then((value) => setConfig({ ...DEFAULT_CONFIG, ...value }))
      .catch((error) => toast.error(String(error)))
      .finally(() => setLoading(false));
  }, []);

  const expiresLabel = config.tokenExpiresAtMs
    ? new Date(config.tokenExpiresAtMs).toLocaleString()
    : t("settings.advanced.apiManagement.notConfigured");
  const origin =
    typeof window === "undefined"
      ? "https://server.example"
      : window.location.origin;
  const examples = useMemo(
    () => [
      `curl -fsS -H "Authorization: Bearer <debug-token>" "${origin}/web-api/debug/diagnostics"`,
      `curl -fsS -H "Authorization: Bearer <debug-token>" "${origin}/web-api/debug/logs/tail?lines=${config.logTailLines}"`,
      `curl -fsS -X POST -H "Authorization: Bearer <debug-token>" "${origin}/web-api/debug/restart"`,
      `curl -fsS -X POST -H "Authorization: Bearer <debug-token>" -H "Content-Type: application/json" -d '{"restartAfter":true}' "${origin}/web-api/debug/upgrade"`,
    ],
    [config.logTailLines, origin],
  );

  const save = async (next: ApiManagementConfig) => {
    const previous = config;
    setConfig(next);
    setSaving(true);
    try {
      setConfig(await settingsApi.setApiManagement(next));
    } catch (error) {
      setConfig(previous);
      toast.error(String(error));
    } finally {
      setSaving(false);
    }
  };

  const toggle = (key: keyof ApiManagementConfig, enabled: boolean) => {
    if (enabled && (key === "restartEnabled" || key === "upgradeEnabled")) {
      setConfirmCapability(key);
      return;
    }
    void save({ ...config, [key]: enabled });
  };

  const generate = async () => {
    setSaving(true);
    try {
      const result = await settingsApi.generateDebugToken(ttlHours);
      setVisibleToken(result.token);
      setConfig({
        ...config,
        tokenConfigured: true,
        tokenExpiresAtMs: result.expiresAtMs,
      });
    } catch (error) {
      toast.error(String(error));
    } finally {
      setSaving(false);
    }
  };

  const revoke = async () => {
    setSaving(true);
    try {
      await settingsApi.revokeDebugToken();
      setVisibleToken(null);
      setConfig({ ...config, tokenConfigured: false, tokenExpiresAtMs: null });
    } catch (error) {
      toast.error(String(error));
    } finally {
      setSaving(false);
    }
  };

  const copyToken = async () => {
    if (!visibleToken) return;
    await navigator.clipboard.writeText(visibleToken);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };

  if (loading) return null;

  return (
    <div className="space-y-6">
      <div className="flex items-start gap-3 border-b border-border/60 pb-5">
        <ShieldAlert className="mt-0.5 h-5 w-5 shrink-0 text-amber-500" />
        <p className="text-sm leading-6 text-muted-foreground">
          {t("settings.advanced.apiManagement.securityHint")}
        </p>
      </div>

      <div className="space-y-4">
        <div className="flex flex-wrap items-end gap-3">
          <div className="space-y-2">
            <Label htmlFor="debug-token-ttl">
              {t("settings.advanced.apiManagement.ttl")}
            </Label>
            <Input
              id="debug-token-ttl"
              className="w-24"
              type="number"
              min={1}
              max={24}
              value={ttlHours}
              onChange={(event) =>
                setTtlHours(
                  Math.min(24, Math.max(1, Number(event.target.value) || 1)),
                )
              }
            />
          </div>
          <Button onClick={() => void generate()} disabled={saving}>
            {config.tokenConfigured ? (
              <RotateCcw className="mr-2 h-4 w-4" />
            ) : (
              <KeyRound className="mr-2 h-4 w-4" />
            )}
            {t(
              config.tokenConfigured
                ? "settings.advanced.apiManagement.rotateToken"
                : "settings.advanced.apiManagement.generateToken",
            )}
          </Button>
          {config.tokenConfigured ? (
            <Button
              variant="outline"
              onClick={() => void revoke()}
              disabled={saving}
            >
              <Trash2 className="mr-2 h-4 w-4" />
              {t("settings.advanced.apiManagement.revokeToken")}
            </Button>
          ) : null}
        </div>
        <p className="text-xs text-muted-foreground">
          {t("settings.advanced.apiManagement.expiresAt")}: {expiresLabel}
        </p>
        {visibleToken ? (
          <div className="flex items-center gap-2">
            <code className="min-w-0 flex-1 break-all border bg-muted/30 p-3 text-xs">
              {visibleToken}
            </code>
            <Button
              size="icon"
              variant="outline"
              title={t("common.copy")}
              onClick={() => void copyToken()}
            >
              {copied ? (
                <Check className="h-4 w-4" />
              ) : (
                <Copy className="h-4 w-4" />
              )}
            </Button>
          </div>
        ) : null}
      </div>

      <div className="divide-y divide-border/60 border-y border-border/60">
        <CapabilityRow
          label={t("settings.advanced.apiManagement.diagnosticsApi")}
          description={t(
            "settings.advanced.apiManagement.diagnosticsApiDescription",
          )}
          checked={config.diagnosticsEnabled}
          disabled={saving}
          onChange={(value) => toggle("diagnosticsEnabled", value)}
        />
        <CapabilityRow
          label={t("settings.advanced.apiManagement.logApi")}
          description={t("settings.advanced.apiManagement.logApiDescription")}
          checked={config.logEnabled}
          disabled={saving}
          onChange={(value) => toggle("logEnabled", value)}
        />
        {config.logEnabled ? (
          <div className="flex items-center justify-between gap-4 py-4 pl-4">
            <Label htmlFor="debug-log-lines">
              {t("settings.advanced.apiManagement.logTailLines")}
            </Label>
            <Input
              id="debug-log-lines"
              className="w-24"
              type="number"
              min={1}
              max={1000}
              value={config.logTailLines}
              onChange={(event) =>
                void save({
                  ...config,
                  logTailLines: Math.min(
                    1000,
                    Math.max(1, Number(event.target.value) || 1),
                  ),
                })
              }
            />
          </div>
        ) : null}
        <CapabilityRow
          label={t("settings.advanced.apiManagement.restartApi")}
          description={t(
            "settings.advanced.apiManagement.restartApiDescription",
          )}
          checked={config.restartEnabled}
          disabled={saving}
          onChange={(value) => toggle("restartEnabled", value)}
        />
        <CapabilityRow
          label={t("settings.advanced.apiManagement.upgradeApi")}
          description={t(
            "settings.advanced.apiManagement.upgradeApiDescription",
          )}
          checked={config.upgradeEnabled}
          disabled={saving}
          onChange={(value) => toggle("upgradeEnabled", value)}
        />
      </div>

      <div className="space-y-2">
        <Label>{t("settings.advanced.apiManagement.examples")}</Label>
        <pre className="max-h-56 overflow-auto whitespace-pre-wrap break-all border bg-muted/30 p-3 font-mono text-[11px] leading-5">
          {examples.join("\n\n")}
        </pre>
      </div>

      <ConfirmDialog
        isOpen={confirmCapability !== null}
        title={t("settings.advanced.apiManagement.dangerTitle")}
        message={t("settings.advanced.apiManagement.dangerMessage")}
        confirmText={t("common.confirm")}
        onCancel={() => setConfirmCapability(null)}
        onConfirm={() => {
          if (confirmCapability)
            void save({ ...config, [confirmCapability]: true });
          setConfirmCapability(null);
        }}
      />
    </div>
  );
}

function CapabilityRow({
  label,
  description,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  description: string;
  checked: boolean;
  disabled: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <div className="flex items-center justify-between gap-4 py-4">
      <div className="space-y-1">
        <Label>{label}</Label>
        <p className="text-xs text-muted-foreground">{description}</p>
      </div>
      <Switch
        checked={checked}
        disabled={disabled}
        onCheckedChange={onChange}
      />
    </div>
  );
}
