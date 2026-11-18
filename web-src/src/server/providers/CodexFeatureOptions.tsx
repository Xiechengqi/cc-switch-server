import { useId } from "react";
import { useTranslation } from "react-i18next";

import { FeatureToggleList, FeatureToggleRow } from "./FeatureToggleRow";

export interface CodexFeatureOptionValues {
  codexServiceTier: "standard" | "priority" | "ultrafast";
  codexImageGenerationEnabled: boolean;
  codexWebsocketEnabled: boolean;
}

export type CodexFeatureOptionKey = keyof CodexFeatureOptionValues;

interface CodexFeatureOptionsProps {
  values: CodexFeatureOptionValues;
  onChange: <K extends CodexFeatureOptionKey>(key: K, value: CodexFeatureOptionValues[K]) => void;
}

const OPTIONS = [
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
      <div className="flex items-center gap-2 px-3 py-2">
        <label htmlFor={`${idPrefix}-tier`} className="min-w-0 flex-1 text-sm">
          {t("codexOauth.serviceTier")}
        </label>
        <select
          id={`${idPrefix}-tier`}
          className="rounded-md border border-input bg-background px-2 py-1 text-sm"
          aria-describedby={`${idPrefix}-tier-description`}
          value={values.codexServiceTier}
          onChange={(event) => onChange("codexServiceTier", event.target.value as CodexFeatureOptionValues["codexServiceTier"])}
        >
          <option value="standard">{t("codexOauth.serviceTierStandard")}</option>
          <option value="priority">{t("codexOauth.serviceTierPriority")}</option>
          <option value="ultrafast">{t("codexOauth.serviceTierUltrafast")}</option>
        </select>
      </div>
      <p id={`${idPrefix}-tier-description`} className="px-3 py-2 text-xs text-muted-foreground">
        {t("codexOauth.serviceTierDescription")}
      </p>
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
