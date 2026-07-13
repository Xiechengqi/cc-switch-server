import { useState } from "react";
import { Eye, EyeOff } from "lucide-react";

import { useI18n } from "@/lib/i18n";

export function AuthPasswordInput({
  label,
  value,
  onChange,
  autoComplete,
  className,
  id,
  placeholder,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  autoComplete?: string;
  className?: string;
  id?: string;
  placeholder?: string;
}) {
  const { t } = useI18n();
  const [visible, setVisible] = useState(false);

  return (
    <label className={className}>
      <span>{label}</span>
      <div className="password-field">
        <input
          id={id}
          type={visible ? "text" : "password"}
          autoComplete={autoComplete}
          value={value}
          placeholder={placeholder}
          className="password-field-input"
          onChange={(event) => onChange(event.target.value)}
        />
        <button
          type="button"
          className="password-field-toggle"
          tabIndex={-1}
          aria-label={
            visible
              ? t("apiKeyInput.hide", { defaultValue: "隐藏" })
              : t("apiKeyInput.show", { defaultValue: "显示" })
          }
          onClick={() => setVisible((current) => !current)}
        >
          {visible ? <EyeOff size={16} /> : <Eye size={16} />}
        </button>
      </div>
    </label>
  );
}
