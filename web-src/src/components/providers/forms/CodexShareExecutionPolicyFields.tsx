import React from "react";
import { useTranslation } from "react-i18next";

import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";

interface CodexShareExecutionPolicyFieldsProps {
  allowPersonalCredits: boolean;
  autoConsumeBankedReset: boolean;
  bankedResetExpiryLeadMinutes: string;
  previousResponseCacheEnabled: boolean;
  disabled?: boolean;
  onAllowPersonalCreditsChange: (checked: boolean) => void;
  onAutoConsumeBankedResetChange: (checked: boolean) => void;
  onBankedResetExpiryLeadMinutesChange: (value: string) => void;
  onPreviousResponseCacheEnabledChange: (checked: boolean) => void;
}

export const CodexShareExecutionPolicyFields: React.FC<
  CodexShareExecutionPolicyFieldsProps
> = ({
  allowPersonalCredits,
  autoConsumeBankedReset,
  bankedResetExpiryLeadMinutes,
  previousResponseCacheEnabled,
  disabled,
  onAllowPersonalCreditsChange,
  onAutoConsumeBankedResetChange,
  onBankedResetExpiryLeadMinutesChange,
  onPreviousResponseCacheEnabledChange,
}) => {
  const { t } = useTranslation();
  const leadMinutes = Number(bankedResetExpiryLeadMinutes);
  const leadInvalid =
    !Number.isSafeInteger(leadMinutes) || leadMinutes < 10 || leadMinutes > 10080;

  return (
    <div className="space-y-3 border-y border-border/50 py-4 md:col-span-2">
      <div>
        <Label>{t("codexSharePolicy.title")}</Label>
        <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
          {t("codexSharePolicy.description")}
        </p>
      </div>
      <div className="grid gap-3 sm:grid-cols-2">
        <label className="flex items-start justify-between gap-3 rounded-md border border-border-default p-3">
          <span className="min-w-0">
            <span className="block text-sm font-medium">
              {t("codexSharePolicy.personalCredits")}
            </span>
            <span className="mt-1 block text-xs leading-relaxed text-muted-foreground">
              {t("codexSharePolicy.personalCreditsHint")}
            </span>
          </span>
          <Switch
            checked={allowPersonalCredits}
            disabled={disabled}
            onCheckedChange={onAllowPersonalCreditsChange}
          />
        </label>
        <label className="flex items-start justify-between gap-3 rounded-md border border-border-default p-3">
          <span className="min-w-0">
            <span className="block text-sm font-medium">
              {t("codexSharePolicy.previousResponseCache")}
            </span>
            <span className="mt-1 block text-xs leading-relaxed text-muted-foreground">
              {t("codexSharePolicy.previousResponseCacheHint")}
            </span>
          </span>
          <Switch
            checked={previousResponseCacheEnabled}
            disabled={disabled}
            onCheckedChange={onPreviousResponseCacheEnabledChange}
          />
        </label>
        <label className="flex items-start justify-between gap-3 rounded-md border border-border-default p-3 sm:col-span-2">
          <span className="min-w-0">
            <span className="block text-sm font-medium">
              {t("codexSharePolicy.autoReset")}
            </span>
            <span className="mt-1 block text-xs leading-relaxed text-muted-foreground">
              {t("codexSharePolicy.autoResetHint")}
            </span>
          </span>
          <Switch
            checked={autoConsumeBankedReset}
            disabled={disabled}
            onCheckedChange={onAutoConsumeBankedResetChange}
          />
        </label>
      </div>
      {autoConsumeBankedReset ? (
        <div className="max-w-xs space-y-2">
          <Label htmlFor="codex-banked-reset-lead-minutes">
            {t("codexSharePolicy.resetLeadMinutes")}
          </Label>
          <Input
            id="codex-banked-reset-lead-minutes"
            type="number"
            min={10}
            max={10080}
            step={10}
            disabled={disabled}
            aria-invalid={leadInvalid}
            value={bankedResetExpiryLeadMinutes}
            onChange={(event) =>
              onBankedResetExpiryLeadMinutesChange(event.target.value)
            }
          />
          <p
            className={
              leadInvalid
                ? "text-xs text-destructive"
                : "text-xs text-muted-foreground"
            }
          >
            {leadInvalid
              ? t("codexSharePolicy.resetLeadInvalid")
              : t("codexSharePolicy.resetLeadHint")}
          </p>
        </div>
      ) : null}
    </div>
  );
};
