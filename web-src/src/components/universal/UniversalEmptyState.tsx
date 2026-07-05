import { Boxes, ListPlus, Plus, Upload } from "lucide-react";

import { useI18n } from "@/lib/i18n";

interface UniversalEmptyStateProps {
  canUsePresets: boolean;
  onImport: () => void;
  onPreset: () => void;
  onCreate: () => void;
}

export function UniversalEmptyState({
  canUsePresets,
  onImport,
  onPreset,
  onCreate,
}: UniversalEmptyStateProps) {
  const { t, tx } = useI18n();
  return (
    <div className="provider-empty provider-empty-state">
      <div className="provider-empty-icon">
        <Boxes size={28} />
      </div>
      <strong>{tx("No universal providers")}</strong>
      <p>{tx("Create one template, then sync it into Claude, Codex and Gemini providers.")}</p>
      <div className="provider-empty-actions">
        <button className="primary-button" type="button" onClick={onImport}>
          <Upload size={15} />
          <span>{t("common.import")}</span>
        </button>
        <button
          className="secondary-button"
          type="button"
          onClick={onPreset}
          disabled={!canUsePresets}
        >
          <ListPlus size={15} />
          <span>{t("server.common.fromPreset")}</span>
        </button>
        <button className="secondary-button" type="button" onClick={onCreate}>
          <Plus size={15} />
          <span>{t("server.universal.add")}</span>
        </button>
      </div>
    </div>
  );
}
