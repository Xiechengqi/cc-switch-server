import { useEffect, useState } from "react";
import { Loader2, Save } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  providersApi,
  type ProviderRuntimeDefaults,
} from "@/lib/api/providers";

const FALLBACK_DEFAULTS: ProviderRuntimeDefaults = {
  transport: {
    timeoutMs: 300_000,
    streamFirstByteTimeoutMs: 120_000,
    streamIdleTimeoutMs: 300_000,
  },
  testModels: {
    claude: "claude-haiku-4-5-20251001",
    codex: "gpt-5.6-sol@low",
    gemini: "gemini-3.5-flash",
  },
};

type FormState = {
  timeoutMs: string;
  streamFirstByteTimeoutMs: string;
  streamIdleTimeoutMs: string;
  claude: string;
  codex: string;
  gemini: string;
};

function formFromDefaults(defaults: ProviderRuntimeDefaults): FormState {
  return {
    timeoutMs: String(defaults.transport.timeoutMs),
    streamFirstByteTimeoutMs: String(
      defaults.transport.streamFirstByteTimeoutMs,
    ),
    streamIdleTimeoutMs: String(defaults.transport.streamIdleTimeoutMs),
    claude: defaults.testModels.claude,
    codex: defaults.testModels.codex,
    gemini: defaults.testModels.gemini,
  };
}

function parseInteger(value: string, min: number, max: number): number | null {
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed >= min && parsed <= max
    ? parsed
    : null;
}

export function ProviderRuntimeDefaultsPanel() {
  const { t } = useTranslation();
  const [form, setForm] = useState<FormState>(() =>
    formFromDefaults(FALLBACK_DEFAULTS),
  );
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [reloadGeneration, setReloadGeneration] = useState(0);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setLoadError(null);
    void providersApi
      .getRuntimeDefaults()
      .then((defaults) => {
        if (active) setForm(formFromDefaults(defaults));
      })
      .catch((error) => {
        if (active) {
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
    const timeoutMs = parseInteger(form.timeoutMs, 1_000, 3_600_000);
    const streamFirstByteTimeoutMs = parseInteger(
      form.streamFirstByteTimeoutMs,
      1_000,
      600_000,
    );
    const streamIdleTimeoutMs = parseInteger(
      form.streamIdleTimeoutMs,
      1_000,
      3_600_000,
    );
    const models = {
      claude: form.claude.trim(),
      codex: form.codex.trim(),
      gemini: form.gemini.trim(),
    };
    if (
      timeoutMs == null ||
      streamFirstByteTimeoutMs == null ||
      streamIdleTimeoutMs == null ||
      Object.values(models).some(
        (model) => !model || model.length > 256,
      )
    ) {
      toast.error(
        t("settings.advanced.providerDefaults.invalid", {
          defaultValue: "请检查超时范围和测试模型",
        }),
      );
      return;
    }

    setSaving(true);
    try {
      const defaults: ProviderRuntimeDefaults = {
        transport: {
          timeoutMs,
          streamFirstByteTimeoutMs,
          streamIdleTimeoutMs,
        },
        testModels: models,
      };
      await providersApi.saveRuntimeDefaults(defaults);
      setForm(formFromDefaults(defaults));
      toast.success(t("notifications.settingsSaved"), { closeButton: true });
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setSaving(false);
    }
  };

  if (loading) {
    return (
      <div className="flex justify-center p-4">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    );
  }

  if (loadError) {
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
          {t("settings.advanced.providerDefaults.transportTitle", {
            defaultValue: "请求超时默认值",
          })}
        </h4>
        <div className="grid gap-4 md:grid-cols-3">
          {[
            {
              key: "timeoutMs" as const,
              label: t("providerBundle.requestTimeout"),
              max: 3_600_000,
            },
            {
              key: "streamFirstByteTimeoutMs" as const,
              label: t("providerBundle.firstByteTimeout"),
              max: 600_000,
            },
            {
              key: "streamIdleTimeoutMs" as const,
              label: t("providerBundle.streamIdleTimeout"),
              max: 3_600_000,
            },
          ].map(({ key, label, max }) => (
            <div key={key} className="space-y-2">
              <Label htmlFor={`server-default-${key}`}>{label}</Label>
              <Input
                id={`server-default-${key}`}
                type="number"
                min={1_000}
                max={max}
                step={1_000}
                value={form[key]}
                onChange={(event) =>
                  setForm((current) => ({
                    ...current,
                    [key]: event.target.value,
                  }))
                }
              />
            </div>
          ))}
        </div>
      </div>

      <div className="space-y-4">
        <h4 className="text-sm font-medium text-muted-foreground">
          {t("settings.advanced.providerDefaults.modelsTitle", {
            defaultValue: "测试模型默认值",
          })}
        </h4>
        <div className="grid gap-4 md:grid-cols-3">
          {(["claude", "codex", "gemini"] as const).map((app) => (
            <div key={app} className="space-y-2">
              <Label htmlFor={`server-default-${app}-model`}>
                {t(`streamCheck.${app}Model`)}
              </Label>
              <Input
                id={`server-default-${app}-model`}
                value={form[app]}
                maxLength={256}
                onChange={(event) =>
                  setForm((current) => ({
                    ...current,
                    [app]: event.target.value,
                  }))
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
