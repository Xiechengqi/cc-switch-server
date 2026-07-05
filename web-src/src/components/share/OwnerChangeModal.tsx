import { useState } from "react";
import { Loader2, RefreshCw, Users } from "lucide-react";

import { ModalFooter } from "@/components/ModalFooter";
import { SimpleModal } from "@/components/SimpleModal";
import { useI18n } from "@/lib/i18n";

export interface OwnerChangeDraft {
  originalOwnerEmail: string;
  ownerEmail: string;
}

export function OwnerChangeModal({
  draft,
  saving,
  onClose,
  onRequestCode,
  onVerify,
}: {
  draft: OwnerChangeDraft;
  saving: boolean;
  onClose: () => void;
  onRequestCode: () => Promise<{ maskedDestination: string; cooldownSecs: number }>;
  onVerify: (code: string) => Promise<void>;
}) {
  const { tx } = useI18n();
  const [code, setCode] = useState("");
  const [result, setResult] = useState<string | null>(null);
  const [localError, setLocalError] = useState<string | null>(null);
  const hasCode = code.trim().length > 0;
  return (
    <SimpleModal title="Verify Share Owner" subtitle={draft.ownerEmail} onClose={onClose}>
      <form
        className="modal-form-stack owner-change-form"
        onSubmit={(event) => {
          event.preventDefault();
          setLocalError(null);
          void onVerify(code).catch((reason) => setLocalError(errorMessage(reason)));
        }}
      >
        <section className="owner-change-panel">
          <header>
            <span className="provider-icon-frame">
              <Users size={20} />
            </span>
            <div>
              <h3>{tx("Owner handoff")}</h3>
              <p>{tx("Email verification is required before saving this share owner change.")}</p>
            </div>
          </header>
          <div className="owner-change-flow">
            <OwnerNode label="current owner" value={draft.originalOwnerEmail || "-"} muted />
            <span className="owner-change-arrow">-&gt;</span>
            <OwnerNode label="new owner" value={draft.ownerEmail || "-"} />
          </div>
          <div className="owner-change-steps">
            <OwnerStep label="request code" active />
            <OwnerStep label="verify email" active={Boolean(result) || hasCode} />
            <OwnerStep label="save share" active={hasCode} />
          </div>
        </section>
        <button
          className="secondary-button owner-request-button"
          type="button"
          disabled={saving}
          onClick={() => {
            setLocalError(null);
            void onRequestCode()
              .then((response) =>
                setResult(
                  response.cooldownSecs
                    ? tx("code sent to {{destination}}; cooldown {{seconds}}s", {
                        destination: response.maskedDestination,
                        seconds: response.cooldownSecs,
                      })
                    : tx("code sent to {{destination}}", { destination: response.maskedDestination }),
                ),
              )
              .catch((reason) => setLocalError(errorMessage(reason)));
          }}
        >
          {saving ? <Loader2 size={15} /> : <RefreshCw size={15} />}
          <span>{tx("Request Code")}</span>
        </button>
        <label>
          <span>{tx("Email code")}</span>
          <input value={code} onChange={(event) => setCode(event.target.value)} required />
        </label>
        {result && <div className="provider-card-result">{result}</div>}
        {localError && <div className="form-error">{localError}</div>}
        <ModalFooter saving={saving} disabled={!hasCode} onClose={onClose} label="Verify Owner" />
      </form>
    </SimpleModal>
  );
}

function OwnerNode({ label, value, muted = false }: { label: string; value: string; muted?: boolean }) {
  const { tx } = useI18n();
  return (
    <div className={muted ? "owner-node muted" : "owner-node"}>
      <span>{tx(label)}</span>
      <strong title={value}>{value}</strong>
    </div>
  );
}

function OwnerStep({ label, active }: { label: string; active: boolean }) {
  const { tx } = useI18n();
  return (
    <div className={active ? "owner-step active" : "owner-step"}>
      <span />
      <strong>{tx(label)}</strong>
    </div>
  );
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
