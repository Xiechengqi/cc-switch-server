import { useId, useState } from "react";
import { useTranslation } from "react-i18next";

import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Button } from "@/components/ui/button";
import { providersApi, type ProviderInferenceTestResponse } from "@/lib/api/providers";

export interface GrokFeatureOptionValues {
  grokImageGenerationEnabled: boolean;
  grokImageEditEnabled: boolean;
  grokVideoGenerationEnabled: boolean;
}

export type GrokFeatureOptionKey = keyof GrokFeatureOptionValues;

const OPTIONS = [
  ["grokImageGenerationEnabled", "图片生成", "允许该 Grok OAuth Provider 处理图片生成请求。"],
  ["grokImageEditEnabled", "图片编辑", "允许该 Grok OAuth Provider 处理图片编辑请求。"],
  ["grokVideoGenerationEnabled", "视频生成", "允许该 Grok OAuth Provider 创建并查询视频任务。"],
] as const;

export function GrokFeatureOptions({ values, onChange, providerId }: {
  values: GrokFeatureOptionValues;
  onChange: (key: GrokFeatureOptionKey, enabled: boolean) => void;
  providerId?: string;
}) {
  const { t } = useTranslation();
  const prefix = useId();
  const [testing, setTesting] = useState<ProviderInferenceTestResponse["operation"] | null>(null);
  const [testResult, setTestResult] = useState("");
  const runTest = async (operation: ProviderInferenceTestResponse["operation"]) => {
    if (!providerId || testing) return;
    setTesting(operation);
    setTestResult("");
    try {
      const result = await providersApi.testGrokMedia(providerId, operation);
      setTestResult(`${result.statusCode} · ${result.latencyMs}ms`);
    } catch (error) {
      setTestResult(error instanceof Error ? error.message : String(error));
    } finally {
      setTesting(null);
    }
  };
  return <div className="divide-y rounded-md border border-border/60">
    {OPTIONS.map(([key, label, description]) => {
      const id = `${prefix}-${key}`;
      const operation = key === "grokImageGenerationEnabled"
        ? "image_generation"
        : key === "grokImageEditEnabled"
          ? "image_edit"
          : "video_generation";
      return <div key={key} className="flex items-start justify-between gap-4 px-3 py-3">
        <div className="min-w-0 space-y-1 pr-2">
          <Label htmlFor={id}>{t(`grokOauth.${key}`, { defaultValue: label })}</Label>
          <p id={`${id}-description`} className="text-xs leading-5 text-muted-foreground">
            {t(`grokOauth.${key}Description`, { defaultValue: description })}
          </p>
        </div>
        <div className="flex items-center gap-2">
          {providerId && values[key] ? (
            <Button type="button" variant="outline" size="sm" disabled={testing !== null} onClick={() => void runTest(operation)}>
              {testing === operation ? t("endpointTest.testing") : t("endpointTest.testSpeed")}
            </Button>
          ) : null}
          <Switch id={id} checked={values[key]} onCheckedChange={(enabled) => onChange(key, enabled)} aria-describedby={`${id}-description`} />
        </div>
      </div>;
    })}
    {testResult ? <div className="px-3 py-2 text-xs text-muted-foreground">{testResult}</div> : null}
  </div>;
}
