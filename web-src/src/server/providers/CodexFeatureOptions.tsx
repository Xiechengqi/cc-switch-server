import { useId } from "react";
import { useTranslation } from "react-i18next";

import { FeatureToggleList, FeatureToggleRow } from "./FeatureToggleRow";

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
    <FeatureToggleList>
      {OPTIONS.map((option) => (
        <FeatureToggleRow
          key={option.key}
          id={`${idPrefix}-${option.key}`}
          label={t(option.labelKey)}
          description={t(option.descriptionKey)}
          checked={values[option.key]}
          onCheckedChange={(enabled) => onChange(option.key, enabled)}
        />
      ))}
    </FeatureToggleList>
  );
}
