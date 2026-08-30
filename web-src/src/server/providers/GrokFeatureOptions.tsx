import { useId, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import {
  providersApi,
  type ProviderInferenceTestResponse,
} from "@/lib/api/providers";
import { FeatureToggleList, FeatureToggleRow } from "./FeatureToggleRow";

export interface GrokFeatureOptionValues {
  grokImageGenerationEnabled: boolean;
  grokImageEditEnabled: boolean;
  grokVideoGenerationEnabled: boolean;
}

export type GrokFeatureOptionKey = keyof GrokFeatureOptionValues;

const OPTIONS = [
  [
    "grokImageGenerationEnabled",
    "image_generation",
    "图片生成",
    "允许该 Grok OAuth Provider 处理图片生成请求。",
  ],
  [
    "grokImageEditEnabled",
    "image_edit",
    "图片编辑",
    "允许该 Grok OAuth Provider 处理图片编辑请求。",
  ],
  [
    "grokVideoGenerationEnabled",
    "video_generation",
    "视频生成",
    "允许该 Grok OAuth Provider 创建并查询视频任务。",
  ],
] as const satisfies ReadonlyArray<
  readonly [
    GrokFeatureOptionKey,
    ProviderInferenceTestResponse["operation"],
    string,
    string,
  ]
>;

export function GrokFeatureOptions({
  values,
  onChange,
  providerId,
}: {
  values: GrokFeatureOptionValues;
  onChange: (key: GrokFeatureOptionKey, enabled: boolean) => void;
  providerId?: string;
}) {
  const { t } = useTranslation();
  const prefix = useId();
  const [testing, setTesting] = useState<
    ProviderInferenceTestResponse["operation"] | null
  >(null);
  const [testResult, setTestResult] = useState("");
  const runTest = async (
    operation: ProviderInferenceTestResponse["operation"],
  ) => {
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
  return (
    <FeatureToggleList>
      {OPTIONS.map(([key, operation, label, description]) => (
        <FeatureToggleRow
          key={key}
          id={`${prefix}-${key}`}
          label={t(`grokOauth.${key}`, { defaultValue: label })}
          description={t(`grokOauth.${key}Description`, {
            defaultValue: description,
          })}
          checked={values[key]}
          onCheckedChange={(enabled) => onChange(key, enabled)}
          action={
            providerId && values[key] ? (
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-7 shrink-0 px-2 text-xs"
                disabled={testing !== null}
                onClick={() => void runTest(operation)}
              >
                {testing === operation
                  ? t("endpointTest.testing")
                  : t("endpointTest.testSpeed")}
              </Button>
            ) : null
          }
        />
      ))}
      {testResult ? (
        <div className="px-3 py-2 text-xs text-muted-foreground">
          {testResult}
        </div>
      ) : null}
    </FeatureToggleList>
  );
}
