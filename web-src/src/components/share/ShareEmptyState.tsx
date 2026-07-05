import { Share2, Upload } from "lucide-react";

import { useI18n } from "@/lib/i18n";

interface ShareEmptyStateProps {
  canCreate: boolean;
  onCreate: () => void;
  onImport: () => void;
}

export function ShareEmptyState({ canCreate, onCreate, onImport }: ShareEmptyStateProps) {
  const { t, tx } = useI18n();
  return (
    <div className="provider-empty provider-empty-state">
      <div className="provider-empty-icon">
        <Share2 size={28} />
      </div>
      <strong>{t("server.shares.noShares")}</strong>
      <p>{t("server.shares.noSharesHint")}</p>
      {!canCreate && <p>{tx("Add a provider before creating a share.")}</p>}
      <div className="provider-empty-actions">
        <button className="primary-button" type="button" onClick={onImport}>
          <Upload size={15} />
          <span>{t("common.import")}</span>
        </button>
        <button
          className="secondary-button"
          type="button"
          onClick={onCreate}
          disabled={!canCreate}
        >
          <Share2 size={15} />
          <span>{t("server.shares.createShare")}</span>
        </button>
      </div>
    </div>
  );
}
