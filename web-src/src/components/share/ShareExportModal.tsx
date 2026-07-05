import { Copy } from "lucide-react";

import { SimpleModal } from "@/components/SimpleModal";
import { useI18n } from "@/lib/i18n";

export function ShareExportModal({
  exportText,
  copyStatus,
  onCopy,
  onClose,
}: {
  exportText: string;
  copyStatus: { tone: "success" | "warning"; message: string } | null;
  onCopy: () => void;
  onClose: () => void;
}) {
  const { tx } = useI18n();
  return (
    <SimpleModal
      title="Export Shares"
      subtitle="Copy this JSON when clipboard access is unavailable."
      onClose={onClose}
    >
      <textarea readOnly value={exportText} />
      {copyStatus && <div className={`connect-copy-status ${copyStatus.tone}`}>{copyStatus.message}</div>}
      <footer className="modal-inline-footer">
        <button className="secondary-button" type="button" onClick={onCopy}>
          <Copy size={15} />
          <span>{tx("Copy JSON")}</span>
        </button>
        <button className="secondary-button" type="button" onClick={onClose}>
          {tx("Close")}
        </button>
      </footer>
    </SimpleModal>
  );
}
