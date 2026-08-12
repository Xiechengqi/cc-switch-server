import { useId } from "react";
import { useTranslation } from "react-i18next";

import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";

export interface CodexFeatureOptionValues {
  codexFastMode: boolean;
  codexImageGenerationEnabled: boolean;
  codexWebsocketEnabled: boolean;
}

export type CodexFeatureOptionKey = keyof CodexFeatureOptionValues;

interface CodexFeatureOptionsProps {
  values: CodexFeatureOptionValues;
  onChange: (key: CodexFeatureOptionKey, enabled: boolean) => void;
}

const OPTIONS = [
  {
    key: "codexFastMode",
    labelKey: "codexOauth.fastMode",
    descriptionKey: "codexOauth.fastModeDescription",
  },
  {
    key: "codexImageGenerationEnabled",
    labelKey: "codexOauth.imageGeneration",
    descriptionKey: "codexOauth.imageGenerationDescription",
  },
  {
    key: "codexWebsocketEnabled",
    labelKey: "codexOauth.websocket",
    descriptionKey: "codexOauth.websocketDescription",
  },
] as const satisfies ReadonlyArray<{
  key: CodexFeatureOptionKey;
  labelKey: string;
  descriptionKey: string;
}>;

export function CodexFeatureOptions({
  values,
  onChange,
}: CodexFeatureOptionsProps) {
  const { t } = useTranslation();
  const idPrefix = useId();

  return (
    <div className="divide-y rounded-md border border-border/60">
      {OPTIONS.map((option) => {
        const controlId = `${idPrefix}-${option.key}`;
        const descriptionId = `${controlId}-description`;
        return (
          <div
            key={option.key}
            className="flex items-start justify-between gap-4 px-3 py-3"
          >
            <div className="min-w-0 space-y-1 pr-2">
              <Label htmlFor={controlId} className="text-sm font-medium">
                {t(option.labelKey)}
              </Label>
              <p
                id={descriptionId}
                className="text-xs leading-5 text-muted-foreground"
              >
                {t(option.descriptionKey)}
              </p>
            </div>
            <Switch
              id={controlId}
              className="mt-0.5 shrink-0"
              checked={values[option.key]}
              onCheckedChange={(enabled) => onChange(option.key, enabled)}
              aria-describedby={descriptionId}
            />
          </div>
        );
      })}
    </div>
  );
}
