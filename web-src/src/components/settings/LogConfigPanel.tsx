import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { settingsApi, type LogConfig } from "@/lib/api/settings";
import { isRemoteWebMode } from "@/lib/api/auth";

const LOG_LEVELS = ["error", "warn", "info", "debug", "trace"] as const;
const MAX_API_TAIL_LINES = 1000;

function normalizeLogConfig(config: LogConfig): LogConfig {
  return {
    enabled: config.enabled,
    level: config.level,
    apiEnabled: config.apiEnabled ?? false,
    apiTailLines: Math.min(
      MAX_API_TAIL_LINES,
      Math.max(1, config.apiTailLines ?? 100),
    ),
  };
}

export function LogConfigPanel() {
  const { t } = useTranslation();
  const [config, setConfig] = useState<LogConfig>({
    enabled: true,
    level: "info",
    apiEnabled: false,
    apiTailLines: 100,
  });
  const [isLoading, setIsLoading] = useState(true);

  useEffect(() => {
    settingsApi
      .getLogConfig()
      .then((loaded) => setConfig(normalizeLogConfig(loaded)))
      .catch((e) => console.error("Failed to load log config:", e))
      .finally(() => setIsLoading(false));
  }, []);

  const apiExample = useMemo(() => {
    const origin =
      typeof window !== "undefined" ? window.location.origin : "https://example.com";
    const lines = config.apiTailLines ?? 100;
    return `curl -fsS -H "Authorization: Bearer <token>" "${origin}/web-api/admin/logs/tail?lines=${lines}"`;
  }, [config.apiTailLines]);

  const handleChange = async (updates: Partial<LogConfig>) => {
    const previous = config;
    const newConfig = normalizeLogConfig({ ...config, ...updates });
    if (!newConfig.enabled) {
      newConfig.apiEnabled = false;
    }
    setConfig(newConfig);
    try {
      await settingsApi.setLogConfig(newConfig);
    } catch (e) {
      console.error("Failed to save log config:", e);
      toast.error(String(e));
      setConfig(previous);
    }
  };

  if (isLoading) return null;

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div className="space-y-0.5">
          <Label>{t("settings.advanced.logConfig.enabled")}</Label>
          <p className="text-xs text-muted-foreground">
            {t("settings.advanced.logConfig.enabledDescription")}
          </p>
        </div>
        <Switch
          checked={config.enabled}
          onCheckedChange={(checked) => handleChange({ enabled: checked })}
        />
      </div>

      <div className="flex items-center justify-between">
        <div className="space-y-0.5">
          <Label>{t("settings.advanced.logConfig.level")}</Label>
          <p className="text-xs text-muted-foreground">
            {t("settings.advanced.logConfig.levelDescription")}
          </p>
        </div>
        <Select
          value={config.level}
          disabled={!config.enabled}
          onValueChange={(value) =>
            handleChange({ level: value as LogConfig["level"] })
          }
        >
          <SelectTrigger className="w-[120px]">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {LOG_LEVELS.map((level) => (
              <SelectItem key={level} value={level}>
                {t(`settings.advanced.logConfig.levels.${level}`)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <div className="flex items-center justify-between">
        <div className="space-y-0.5">
          <Label>{t("settings.advanced.logConfig.apiEnabled")}</Label>
          <p className="text-xs text-muted-foreground">
            {t("settings.advanced.logConfig.apiEnabledDescription")}
          </p>
        </div>
        <Switch
          checked={Boolean(config.apiEnabled)}
          disabled={!config.enabled}
          onCheckedChange={(checked) => handleChange({ apiEnabled: checked })}
        />
      </div>

      {config.enabled && config.apiEnabled ? (
        <div className="space-y-2">
          <Label htmlFor="log-api-tail-lines">
            {t("settings.advanced.logConfig.apiTailLines")}
          </Label>
          <p className="text-xs text-muted-foreground">
            {t("settings.advanced.logConfig.apiTailLinesDescription")}
          </p>
          <Input
            id="log-api-tail-lines"
            type="number"
            min={1}
            max={MAX_API_TAIL_LINES}
            step={1}
            className="w-32"
            value={config.apiTailLines ?? 100}
            onChange={(event) => {
              const parsed = Number(event.currentTarget.value);
              if (!Number.isFinite(parsed)) return;
              void handleChange({ apiTailLines: parsed });
            }}
          />
          <div className="rounded-lg border border-border/60 bg-muted/30 p-3">
            <p className="text-xs font-medium text-muted-foreground">
              {t("settings.advanced.logConfig.apiExampleTitle")}
            </p>
            <pre className="mt-2 overflow-x-auto whitespace-pre-wrap break-all font-mono text-[11px] leading-5 text-foreground/90">
              {apiExample}
            </pre>
            {isRemoteWebMode() ? (
              <p className="mt-2 text-xs text-muted-foreground">
                {t("settings.advanced.logConfig.apiClientUrlHint")}
              </p>
            ) : null}
          </div>
        </div>
      ) : null}

      <div className="rounded-lg bg-muted/50 p-4 text-xs space-y-1.5">
        <p className="font-medium text-muted-foreground mb-2">
          {t("settings.advanced.logConfig.levelHint")}
        </p>
        <div className="grid gap-1 text-muted-foreground">
          <p>
            <span className="font-mono text-red-500">error</span> -{" "}
            {t("settings.advanced.logConfig.levelDesc.error")}
          </p>
          <p>
            <span className="font-mono text-orange-500">warn</span> -{" "}
            {t("settings.advanced.logConfig.levelDesc.warn")}
          </p>
          <p>
            <span className="font-mono text-blue-500">info</span> -{" "}
            {t("settings.advanced.logConfig.levelDesc.info")}
          </p>
          <p>
            <span className="font-mono text-green-500">debug</span> -{" "}
            {t("settings.advanced.logConfig.levelDesc.debug")}
          </p>
          <p>
            <span className="font-mono text-gray-500">trace</span> -{" "}
            {t("settings.advanced.logConfig.levelDesc.trace")}
          </p>
        </div>
      </div>
    </div>
  );
}
