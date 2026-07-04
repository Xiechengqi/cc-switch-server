import { useMemo } from "react";
import { Wand2 } from "lucide-react";

import { useI18n } from "@/lib/i18n";

interface JsonEditorProps {
  id?: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  rows?: number;
  showValidation?: boolean;
  height?: string | number;
  readOnly?: boolean;
}

export default function JsonEditor({
  id,
  value,
  onChange,
  placeholder = "",
  rows = 8,
  showValidation = true,
  height,
  readOnly = false,
}: JsonEditorProps) {
  const { tx } = useI18n();
  const validationError = useMemo(() => {
    if (!showValidation || !value.trim()) return null;
    try {
      JSON.parse(value);
      return null;
    } catch (error) {
      return error instanceof Error ? error.message : tx("Invalid JSON");
    }
  }, [showValidation, tx, value]);

  function formatJson() {
    if (!value.trim()) return;
    const formatted = JSON.stringify(JSON.parse(value), null, 2);
    onChange(formatted);
  }

  const heightStyle = height ? { height: typeof height === "number" ? `${height}px` : height } : undefined;

  return (
    <div className="json-editor-shell" id={id}>
      <textarea
        className="json-editor-textarea"
        value={value}
        onChange={(event) => onChange(event.target.value)}
        placeholder={placeholder}
        rows={rows}
        readOnly={readOnly}
        spellCheck={false}
        style={heightStyle}
      />
      <div className="json-editor-toolbar">
        {!readOnly && (
          <button
            className="secondary-button compact"
            type="button"
            onClick={() => {
              try {
                formatJson();
              } catch {
                // The inline validation message already shows parse details.
              }
            }}
          >
            <Wand2 size={14} />
            <span>{tx("Format")}</span>
          </button>
        )}
        {validationError && <span className="error-text">{validationError}</span>}
      </div>
    </div>
  );
}
