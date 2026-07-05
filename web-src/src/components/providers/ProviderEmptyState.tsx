import { Download, ListPlus, Users } from "lucide-react";

import { AppKind } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { appLabel } from "@/components/providers/providerDisplay";

interface ProviderEmptyStateProps {
  app: AppKind;
  canCreate: boolean;
  onCreate: () => void;
  onImport?: () => void;
}

export function ProviderEmptyState({ app, canCreate, onCreate, onImport }: ProviderEmptyStateProps) {
  const { t, tx } = useI18n();
  const appName = appLabel(app);
  return (
    <div className="provider-empty provider-empty-state">
      <div className="provider-empty-icon">
        <Users size={28} />
      </div>
      <strong>{t("server.providers.noProvidersForApp", { app: appName })}</strong>
      <p>{t("server.providers.noProvidersHint")}</p>
      <p>{tx("Import existing configuration or create a provider from desktop presets.")}</p>
      <div className="provider-empty-actions">
        {onImport && (
          <button className="primary-button" type="button" onClick={onImport}>
            <Download size={15} />
            <span>{t("common.import")}</span>
          </button>
        )}
        <button
          className={onImport ? "secondary-button" : "primary-button"}
          type="button"
          onClick={onCreate}
          disabled={!canCreate}
        >
          <ListPlus size={15} />
          <span>{t("server.providers.addProvider")}</span>
        </button>
      </div>
    </div>
  );
}
