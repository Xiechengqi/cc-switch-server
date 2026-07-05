import { useI18n } from "@/lib/i18n";

interface TextFieldProps {
  label: string;
  value: string;
  disabled?: boolean;
  placeholder?: string;
  onChange: (value: string) => void;
}

export function TextField({ label, value, disabled, placeholder, onChange }: TextFieldProps) {
  const { tx } = useI18n();
  return (
    <label>
      <span>{tx(label)}</span>
      <input
        value={value}
        disabled={disabled}
        placeholder={placeholder}
        onChange={(event) => onChange(event.target.value)}
      />
    </label>
  );
}
