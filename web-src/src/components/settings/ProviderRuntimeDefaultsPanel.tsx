import { useEffect, useState } from "react";
import { Loader2, Save } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  providersApi,
  type ProviderRequestDefaults,
} from "@/lib/api/providers";

type FormState = {
  requestTimeoutSeconds: string;
  streamFirstByteTimeoutSeconds: string;
  streamIdleTimeoutSeconds: string;
};

function formFromDefaults(defaults: ProviderRequestDefaults): FormState {
  return {
    requestTimeoutSeconds: String(defaults.requestTimeoutSeconds),
    streamFirstByteTimeoutSeconds: String(
      defaults.streamFirstByteTimeoutSeconds,
    ),
    streamIdleTimeoutSeconds: String(defaults.streamIdleTimeoutSeconds),
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
  const [form, setForm] = useState<FormState | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [reloadGeneration, setReloadGeneration] = useState(0);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setLoadError(null);
    void providersApi
      .getRequestDefaults()
      .then((defaults) => {
        if (active) setForm(formFromDefaults(defaults));
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
    const defaults: ProviderRequestDefaults = {
      requestTimeoutSeconds:
        parseInteger(form.requestTimeoutSeconds, 1, 3_600) ?? 0,
      streamFirstByteTimeoutSeconds:
        parseInteger(form.streamFirstByteTimeoutSeconds, 1, 600) ?? 0,
      streamIdleTimeoutSeconds:
        parseInteger(form.streamIdleTimeoutSeconds, 1, 3_600) ?? 0,
    };
    if (Object.values(defaults).some((value) => value === 0)) {
      toast.error(
        t("settings.advanced.providerDefaults.invalid", {
          defaultValue: "请检查超时范围",
        }),
      );
      return;
    }

    setSaving(true);
    try {
      await providersApi.saveRequestDefaults(defaults);
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
      <div className="grid gap-4 md:grid-cols-3">
        {[
          {
            key: "requestTimeoutSeconds" as const,
            label: t("providerBundle.requestTimeout"),
            max: 3_600,
          },
          {
            key: "streamFirstByteTimeoutSeconds" as const,
            label: t("providerBundle.firstByteTimeout"),
            max: 600,
          },
          {
            key: "streamIdleTimeoutSeconds" as const,
            label: t("providerBundle.streamIdleTimeout"),
            max: 3_600,
          },
        ].map(({ key, label, max }) => (
          <div key={key} className="space-y-2">
            <Label htmlFor={`server-default-${key}`}>{label}</Label>
            <Input
              id={`server-default-${key}`}
              type="number"
              min={1}
              max={max}
              step={1}
              value={form[key]}
              onChange={(event) =>
                setForm((current) =>
                  current ? { ...current, [key]: event.target.value } : current,
                )
              }
            />
          </div>
        ))}
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
