import { useState } from "react";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ModalFooter } from "@/components/ModalFooter";
import { SimpleModal } from "@/components/SimpleModal";
import type { ShareRecord } from "@/lib/api";
import { useI18n } from "@/lib/i18n";

export function ImportSharesModal({
  saving,
  onClose,
  onSubmit,
}: {
  saving: boolean;
  onClose: () => void;
  onSubmit: (shares: ShareRecord[]) => void;
}) {
  const { tx } = useI18n();
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pendingShares, setPendingShares] = useState<ShareRecord[] | null>(null);
  return (
    <>
      <SimpleModal title="Import Shares" subtitle="Paste an exported array or { shares } object." onClose={onClose}>
        <form
          className="modal-form-stack"
          onSubmit={(event) => {
            event.preventDefault();
            try {
              const parsed = JSON.parse(text) as { shares?: ShareRecord[] } | ShareRecord[];
              const shares = Array.isArray(parsed) ? parsed : parsed.shares;
              if (!shares?.length) throw new Error(tx("shares array is required"));
              setError(null);
              setPendingShares(shares);
            } catch (reason) {
              setError(errorMessage(reason));
            }
          }}
        >
          {error && <div className="form-error">{error}</div>}
          <textarea value={text} onChange={(event) => setText(event.target.value)} />
          <ModalFooter saving={saving} onClose={onClose} label="Import Shares" />
        </form>
      </SimpleModal>
      <ConfirmDialog
        isOpen={pendingShares !== null}
        title={tx("Import shares")}
        message={tx("Import {{count}} shares? Existing shares with the same IDs may be updated.", {
          count: pendingShares?.length || 0,
        })}
        confirmText={tx("Import")}
        onConfirm={() => {
          const shares = pendingShares;
          setPendingShares(null);
          if (shares) onSubmit(shares);
        }}
        onCancel={() => setPendingShares(null)}
      />
    </>
  );
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
