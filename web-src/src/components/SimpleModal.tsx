import { X } from "lucide-react";
import { ReactNode } from "react";

import { useI18n } from "@/lib/i18n";

interface SimpleModalProps {
  title: string;
  titleVariables?: Record<string, string | number | boolean | null | undefined>;
  subtitle?: string;
  subtitleVariables?: Record<string, string | number | boolean | null | undefined>;
  children: ReactNode;
  onClose: () => void;
}

export function SimpleModal({
  title,
  titleVariables,
  subtitle,
  subtitleVariables,
  children,
  onClose,
}: SimpleModalProps) {
  const { tx } = useI18n();
  return (
    <div className="modal-backdrop" role="presentation">
      <section className="provider-form-modal simple-modal">
        <header>
          <div>
            <h2>{tx(title, titleVariables)}</h2>
            {subtitle && <p>{tx(subtitle, subtitleVariables)}</p>}
          </div>
          <button className="icon-button" type="button" onClick={onClose} aria-label={tx("Close")}>
            <X size={16} />
          </button>
        </header>
        <div className="simple-modal-body">{children}</div>
      </section>
    </div>
  );
}
