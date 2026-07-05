import { Loader2 } from "lucide-react";

import { useI18n } from "@/lib/i18n";

interface ModalFooterProps {
  saving: boolean;
  disabled?: boolean;
  onClose: () => void;
  label: string;
}

export function ModalFooter({ saving, disabled = false, onClose, label }: ModalFooterProps) {
  const { tx } = useI18n();
  return (
    <footer className="modal-inline-footer">
      <button className="secondary-button" type="button" onClick={onClose}>
        {tx("Cancel")}
      </button>
      <button className="primary-button" type="submit" disabled={saving || disabled}>
        {saving && <Loader2 size={15} />}
        <span>{tx(label)}</span>
      </button>
    </footer>
  );
}
