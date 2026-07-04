import { Search, X } from "lucide-react";
import { useMemo, useState } from "react";

import { ProviderIcon } from "@/components/ProviderIcon";
import { getIconMetadata, iconMetadata } from "@/icons/extracted";
import { useI18n } from "@/lib/i18n";

interface IconPickerProps {
  label: string;
  value: string;
  fallbackIcon?: string;
  fallbackColor?: string;
  providerName: string;
  onChange: (value: string) => void;
}

const iconOptions = Object.values(iconMetadata).sort((left, right) =>
  left.displayName.localeCompare(right.displayName),
);

export function IconPicker({
  label,
  value,
  fallbackIcon,
  fallbackColor,
  providerName,
  onChange,
}: IconPickerProps) {
  const { tx } = useI18n();
  const [query, setQuery] = useState("");
  const selectedIcon = value.trim() || fallbackIcon || "";
  const selectedMetadata = selectedIcon ? getIconMetadata(selectedIcon) : undefined;
  const visibleOptions = useMemo(() => {
    const normalizedQuery = query.trim().toLowerCase();
    if (!normalizedQuery) return iconOptions;
    return iconOptions.filter((option) =>
      [option.name, option.displayName]
        .join(" ")
        .toLowerCase()
        .includes(normalizedQuery),
    );
  }, [query]);

  return (
    <div className="icon-picker-field">
      <span>{label}</span>
      <div className="icon-picker-current">
        <span className="provider-icon-frame small">
          <ProviderIcon
            icon={selectedIcon}
            name={providerName || selectedMetadata?.displayName || tx("Provider")}
            color={fallbackColor}
            size={18}
          />
        </span>
        <input
          value={value}
          onChange={(event) => onChange(event.target.value)}
          placeholder={fallbackIcon || tx("auto")}
        />
        {value && (
          <button
            className="icon-picker-clear"
            type="button"
            onClick={() => onChange("")}
            aria-label={tx("Clear icon")}
            title={tx("Clear icon")}
          >
            <X size={13} />
          </button>
        )}
      </div>
      <label className="icon-picker-search">
        <Search size={13} />
        <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={tx("Search icons")} />
      </label>
      <div className="icon-picker-grid">
        {visibleOptions.slice(0, 36).map((option) => (
          <button
            className={option.name === value.trim() ? "active" : ""}
            type="button"
            key={option.name}
            onClick={() => onChange(option.name)}
            title={option.displayName}
            aria-label={option.displayName}
          >
            <ProviderIcon
              icon={option.name}
              name={option.displayName}
              color={option.defaultColor}
              size={18}
            />
          </button>
        ))}
      </div>
    </div>
  );
}
