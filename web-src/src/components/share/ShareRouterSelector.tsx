import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { SHARE_REGIONS } from "@/config/shareRegions";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  CUSTOM_SHARE_ROUTER_VALUE,
  findShareRouterRegion,
  normalizeShareRouterDomainForCompare,
} from "@/utils/shareRouter";

interface ShareRouterSelectorProps {
  value: string;
  onChange: (value: string) => void;
  selectId: string;
  customInputId: string;
  disabled?: boolean;
  error?: string | null;
}

export function ShareRouterSelector({
  value,
  onChange,
  selectId,
  customInputId,
  disabled = false,
  error,
}: ShareRouterSelectorProps) {
  const { t } = useTranslation();
  const normalized = normalizeShareRouterDomainForCompare(value);
  const selectedRegion = useMemo(() => findShareRouterRegion(value), [value]);
  const [customMode, setCustomMode] = useState(
    Boolean(normalized) && !selectedRegion,
  );
  const isCustom = customMode || (Boolean(normalized) && !selectedRegion);
  const [lastCustomDomain, setLastCustomDomain] = useState(
    isCustom ? value.trim() : "",
  );

  const selectValue = isCustom
    ? CUSTOM_SHARE_ROUTER_VALUE
    : selectedRegion?.baseUrl || "";

  useEffect(() => {
    if (normalized && !selectedRegion) {
      setCustomMode(true);
      setLastCustomDomain(value.trim());
    }
  }, [normalized, selectedRegion, value]);

  const handleSelect = (next: string) => {
    if (next === CUSTOM_SHARE_ROUTER_VALUE) {
      setCustomMode(true);
      const fallback = lastCustomDomain || (selectedRegion ? "" : value.trim());
      onChange(fallback);
      return;
    }
    setCustomMode(false);
    onChange(next);
  };

  const handleCustomChange = (next: string) => {
    setLastCustomDomain(next);
    onChange(next);
  };

  return (
    <div className="space-y-2">
      <Select
        value={selectValue}
        onValueChange={handleSelect}
        disabled={disabled}
      >
        <SelectTrigger id={selectId}>
          <SelectValue placeholder={t("share.tunnel.selectRegion")} />
        </SelectTrigger>
        <SelectContent>
          {SHARE_REGIONS.map((region) => (
            <SelectItem key={region.baseUrl} value={region.baseUrl}>
              {region.region} - {region.baseUrl}
            </SelectItem>
          ))}
          <SelectItem value={CUSTOM_SHARE_ROUTER_VALUE}>
            {t("share.tunnel.customRouter", {
              defaultValue: "Custom router",
            })}
          </SelectItem>
        </SelectContent>
      </Select>

      {isCustom ? (
        <div className="space-y-1.5">
          <Label
            htmlFor={customInputId}
            className="text-xs text-muted-foreground"
          >
            {t("share.tunnel.customRouterDomain", {
              defaultValue: "Router domain",
            })}
          </Label>
          <Input
            id={customInputId}
            value={value}
            onChange={(event) => handleCustomChange(event.target.value)}
            disabled={disabled}
            placeholder="router.example.com"
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
          />
          <div className="text-xs text-muted-foreground">
            {t("share.tunnel.customRouterHint", {
              defaultValue:
                "Use a custom cc-switch-router domain. Do not include paths or query strings.",
            })}
          </div>
        </div>
      ) : null}

      {error ? <div className="text-xs text-destructive">{error}</div> : null}
    </div>
  );
}
