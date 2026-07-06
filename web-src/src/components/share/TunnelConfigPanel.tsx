import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { RefreshCw, Save } from "lucide-react";
import type { TunnelConfig } from "@/lib/api";
import { tunnelConfigSchema } from "@/lib/schemas/share";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  CardDescription,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

interface TunnelConfigPanelProps {
  initialConfig: TunnelConfig;
  tunnelConfigured: boolean;
  isSaving: boolean;
  onSave: (config: TunnelConfig) => Promise<void> | void;
}

export function TunnelConfigPanel({
  initialConfig,
  tunnelConfigured,
  isSaving,
  onSave,
}: TunnelConfigPanelProps) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<TunnelConfig>(initialConfig);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setDraft(initialConfig);
  }, [initialConfig]);

  const isDirty = JSON.stringify(draft) !== JSON.stringify(initialConfig);

  const handleSubmit = async () => {
    const result = tunnelConfigSchema.safeParse(draft);
    if (!result.success) {
      setError(
        t(result.error.issues[0]?.message || "share.validation.required"),
      );
      return;
    }

    setError(null);
    await onSave(result.data);
  };

  return (
    <Card className="border-border-default/70">
      <CardHeader className="pb-4">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <CardTitle className="text-lg">{t("share.tunnel.title")}</CardTitle>
            <CardDescription className="mt-1">
              {t("share.tunnel.description")}
            </CardDescription>
          </div>
          <div className="text-sm text-muted-foreground">
            {tunnelConfigured
              ? t("share.tunnel.configured")
              : t("share.tunnel.notConfigured")}
          </div>
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="space-y-2">
          <Label htmlFor="share-domain">{t("share.tunnel.domain")}</Label>
          <Input
            id="share-domain"
            placeholder="example.com"
            value={draft.domain}
            onChange={(e) =>
              setDraft((prev) => ({ ...prev, domain: e.target.value }))
            }
          />
        </div>
        {error ? <div className="text-sm text-red-500">{error}</div> : null}
        <div className="flex flex-wrap items-center justify-end gap-2">
          <Button
            type="button"
            variant="outline"
            onClick={() => {
              setDraft(initialConfig);
              setError(null);
            }}
            disabled={!isDirty || isSaving}
          >
            <RefreshCw className="h-4 w-4" />
            {t("common.reset")}
          </Button>
          <Button
            type="button"
            onClick={() => void handleSubmit()}
            disabled={isSaving}
          >
            <Save className="h-4 w-4" />
            {t("common.save")}
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}
