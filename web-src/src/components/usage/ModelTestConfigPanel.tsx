import { useEffect, useState } from "react";
import { Loader2, Save } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  providersApi,
  type ProviderHealthCheckConfig,
} from "@/lib/api/providers";

type FormState = {
  timeoutSeconds: string;
  maxRetries: string;
  degradedThresholdSeconds: string;
  claude: string;
  codex: string;
  gemini: string;
};

function formFromConfig(config: ProviderHealthCheckConfig): FormState {
  return {
    timeoutSeconds: String(config.timeoutSeconds),
    maxRetries: String(config.maxRetries),
    degradedThresholdSeconds: String(config.degradedThresholdSeconds),
    claude: config.testModels.claude,
    codex: config.testModels.codex,
    gemini: config.testModels.gemini,
  };
}

function parseInteger(value: string, min: number, max: number): number | null {
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed >= min && parsed <= max
    ? parsed
    : null;
}

export function ModelTestConfigPanel() {
  const { t } = useTranslation();
  const [form, setForm] = useState<FormState | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [reloadGeneration, setReloadGeneration] = useState(0);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setLoadError(null);
    void providersApi
      .getHealthCheckConfig()
      .then((config) => {
        if (active) setForm(formFromConfig(config));
      })
      .catch((error) => {
        if (active) {
          setForm(null);
          setLoadError(error instanceof Error ? error.message : String(error));
        }
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [reloadGeneration]);

  const save = async () => {
    if (!form) return;
    const models = {
      claude: form.claude.trim(),
      codex: form.codex.trim(),
      gemini: form.gemini.trim(),
    };
    const config: ProviderHealthCheckConfig = {
      timeoutSeconds: parseInteger(form.timeoutSeconds, 2, 60) ?? 0,
      maxRetries: parseInteger(form.maxRetries, 0, 5) ?? 6,
      degradedThresholdSeconds:
        parseInteger(form.degradedThresholdSeconds, 1, 30) ?? 0,
      testModels: models,
    };
    if (
      config.timeoutSeconds === 0 ||
      config.maxRetries > 5 ||
      config.degradedThresholdSeconds === 0 ||
      Object.values(models).some((model) => !model || model.length > 256)
    ) {
      toast.error(
        t("settings.advanced.modelTest.invalid", {
          defaultValue: "请检查健康检查参数和测试模型",
        }),
      );
      return;
    }

    setSaving(true);
    try {
      await providersApi.saveHealthCheckConfig(config);
      setForm(formFromConfig(config));
      toast.success(t("streamCheck.configSaved"), { closeButton: true });
    } catch (error) {
      toast.error(
        `${t("streamCheck.configSaveFailed")}: ${
          error instanceof Error ? error.message : String(error)
        }`,
      );
    } finally {
      setSaving(false);
    }
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center p-4">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    );
  }

  if (loadError || !form) {
    return (
      <div className="space-y-3 rounded-md border border-destructive/40 p-4">
        <p className="text-sm text-destructive">
          {t("settings.loadFailed", { defaultValue: "加载失败" })}: {loadError}
        </p>
        <Button
          type="button"
          variant="outline"
          onClick={() => setReloadGeneration((generation) => generation + 1)}
        >
          {t("common.retry", { defaultValue: "重试" })}
        </Button>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <div className="space-y-4">
        <h4 className="text-sm font-medium text-muted-foreground">
          {t("streamCheck.checkParams")}
        </h4>
        <div className="grid gap-4 md:grid-cols-3">
          <div className="space-y-2">
            <Label htmlFor="health-timeout-seconds">
              {t("streamCheck.timeout")}
            </Label>
            <Input
              id="health-timeout-seconds"
              type="number"
              min={2}
              max={60}
              step={1}
              value={form.timeoutSeconds}
              onChange={(event) =>
                setForm({ ...form, timeoutSeconds: event.target.value })
              }
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="health-max-retries">
              {t("streamCheck.maxRetries")}
            </Label>
            <Input
              id="health-max-retries"
              type="number"
              min={0}
              max={5}
              step={1}
              value={form.maxRetries}
              onChange={(event) =>
                setForm({ ...form, maxRetries: event.target.value })
              }
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="health-degraded-threshold-seconds">
              {t("streamCheck.degradedThreshold")}
            </Label>
            <Input
              id="health-degraded-threshold-seconds"
              type="number"
              min={1}
              max={30}
              step={1}
              value={form.degradedThresholdSeconds}
              onChange={(event) =>
                setForm({
                  ...form,
                  degradedThresholdSeconds: event.target.value,
                })
              }
            />
          </div>
        </div>
      </div>

      <div className="space-y-4">
        <h4 className="text-sm font-medium text-muted-foreground">
          {t("streamCheck.testModels")}
        </h4>
        <div className="grid gap-4 md:grid-cols-3">
          {(["claude", "codex", "gemini"] as const).map((app) => (
            <div key={app} className="space-y-2">
              <Label htmlFor={`health-${app}-model`}>
                {t(`streamCheck.${app}Model`)}
              </Label>
              <Input
                id={`health-${app}-model`}
                value={form[app]}
                maxLength={256}
                onChange={(event) =>
                  setForm({ ...form, [app]: event.target.value })
                }
              />
            </div>
          ))}
        </div>
      </div>

      <div className="flex justify-end">
        <Button type="button" onClick={() => void save()} disabled={saving}>
          {saving ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          ) : (
            <Save className="mr-2 h-4 w-4" />
          )}
          {t("common.save")}
        </Button>
      </div>
    </div>
  );
}
