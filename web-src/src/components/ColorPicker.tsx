import { RotateCcw } from "lucide-react";

import { useI18n } from "@/lib/i18n";

interface ColorPickerProps {
  label: string;
  value: string;
  fallback?: string;
  onChange: (value: string) => void;
}

const swatches = [
  "#2563eb",
  "#16a34a",
  "#dc2626",
  "#9333ea",
  "#ea580c",
  "#0891b2",
  "#4f46e5",
  "#64748b",
];

export function ColorPicker({ label, value, fallback, onChange }: ColorPickerProps) {
  const { tx } = useI18n();
  const activeColor = normalizeColor(value || fallback) || "#2563eb";
  return (
    <div className="color-picker-field">
      <span>{tx(label)}</span>
      <div className="color-picker-row">
        <label className="color-picker-swatch" title={tx("Choose color")}>
          <input
            type="color"
            value={activeColor}
            onChange={(event) => onChange(event.target.value)}
            aria-label={tx(label)}
          />
          <span style={{ backgroundColor: activeColor }} />
        </label>
        <input
          className="color-picker-hex"
          value={value}
          onChange={(event) => onChange(event.target.value)}
          placeholder={fallback || "#2563eb"}
          spellCheck={false}
        />
        <button
          className="icon-button"
          type="button"
          onClick={() => onChange("")}
          aria-label={tx("Reset color")}
          title={tx("Reset color")}
        >
          <RotateCcw size={14} />
        </button>
      </div>
      <div className="color-picker-swatches" aria-label={tx("Color swatches")}>
        {swatches.map((color) => (
          <button
            key={color}
            className={normalizeColor(value) === color ? "active" : ""}
            type="button"
            onClick={() => onChange(color)}
            aria-label={color}
            title={color}
            style={{ backgroundColor: color }}
          />
        ))}
      </div>
    </div>
  );
}

function normalizeColor(value?: string): string | null {
  const trimmed = value?.trim();
  if (!trimmed) return null;
  const withHash = trimmed.startsWith("#") ? trimmed : `#${trimmed}`;
  return /^#[0-9a-fA-F]{6}$/.test(withHash) ? withHash.toLowerCase() : null;
}
