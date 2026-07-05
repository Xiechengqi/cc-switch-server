import {
  Boxes,
  Loader2,
  X,
} from "lucide-react";
import { FormEvent, useState } from "react";

import { AccountRecord, AppKind, fetchProviderModels, ProviderMatrixEntry } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { inferIconForText } from "@/config/iconInference";
import { ColorPicker } from "@/components/ColorPicker";
import { IconPicker } from "@/components/IconPicker";
import JsonEditor from "@/components/JsonEditor";
import { ProviderIcon } from "@/components/ProviderIcon";
import {
  modelCatalogJsonFromFetchedModels,
  modelOptionsFromCatalogJson,
  ProviderAuthSection,
  ProviderDesktopAdvancedSection,
  ProviderJsonField,
  ProviderModelField,
} from "@/components/providers/ProviderFormSections";
import {
  accountMatchesProviderType,
  colorInputValue,
  errorMessage,
  providerModelCatalogPlaceholder,
  providerModelMappingPlaceholder,
  providerPricingPlaceholder,
  providerSettingJson,
  type ProviderDraft,
} from "@/components/providers/providerDraft";


const apps: Array<{ id: AppKind; label: string }> = [
  { id: "claude", label: "Claude" },
  { id: "codex", label: "Codex" },
  { id: "gemini", label: "Gemini" },
];

export function ProviderFormModal({
  draft,
  entries,
  accounts,
  saving,
  onChange,
  onSubmit,
  onClose,
}: {
  draft: ProviderDraft;
  entries: ProviderMatrixEntry[];
  accounts: AccountRecord[];
  saving: boolean;
  onChange: (draft: ProviderDraft) => void;
  onSubmit: (event: FormEvent) => void;
  onClose: () => void;
}) {
  const { tx } = useI18n();
  const [modelFetchBusy, setModelFetchBusy] = useState(false);
  const [modelFetchResult, setModelFetchResult] = useState<string | null>(null);
  const entry = entries.find((item) => item.providerTypeId === draft.providerTypeId) || entries[0];
  const accountOptions = accounts.filter((account) =>
    accountMatchesProviderType(account, draft.providerTypeId),
  );
  const hasAdvancedConfig = Boolean(
    draft.modelCatalogJson.trim() ||
      draft.modelMappingJson.trim() ||
      draft.pricingJson.trim() ||
      draft.advancedJson.trim(),
  );
  const inferredPreviewIcon = inferIconForText(
    draft.name,
    draft.providerTypeId,
    draft.baseUrl,
    draft.apiFormat,
  );
  const previewIcon = draft.icon
    ? { icon: draft.icon, color: draft.iconColor }
    : { icon: inferredPreviewIcon.icon, color: inferredPreviewIcon.iconColor };
  function patch(next: Partial<ProviderDraft>) {
    onChange({ ...draft, ...next });
  }
  async function fetchModelsForDraft() {
    if (draft.mode !== "edit" || !draft.id) return;
    setModelFetchBusy(true);
    setModelFetchResult(null);
    try {
      const result = await fetchProviderModels(draft.app, draft.id, true);
      const nextProvider = result.provider;
      const nextModelCatalogJson = nextProvider
        ? providerSettingJson(nextProvider.provider, ["modelCatalog"])
        : modelCatalogJsonFromFetchedModels(result.models);
      onChange({
        ...draft,
        model: draft.model || result.models[0]?.id || result.models[0]?.upstreamModel || "",
        modelCatalogJson: nextModelCatalogJson || draft.modelCatalogJson,
        advancedJson: nextProvider ? JSON.stringify(nextProvider.provider, null, 2) : draft.advancedJson,
      });
      setModelFetchResult(tx("Fetched {{models}} models; merged {{merged}}", {
        models: result.models.length,
        merged: result.mergedCount,
      }));
    } catch (reason) {
      setModelFetchResult(errorMessage(reason));
    } finally {
      setModelFetchBusy(false);
    }
  }
  return (
    <div className="modal-backdrop" role="presentation">
      <form className="provider-form-modal" onSubmit={onSubmit}>
        <header>
          <div>
            <h2>{tx(draft.mode === "create" ? "Add Provider" : "Edit Provider")}</h2>
            <p>{entry?.note || tx("Server provider configuration")}</p>
          </div>
          <button className="icon-button" type="button" onClick={onClose} aria-label={tx("Close")}>
            <X size={16} />
          </button>
        </header>
        <div className="provider-form-grid">
          <label>
            <span>{tx("App")}</span>
            <select value={draft.app} disabled>
              {apps.map((app) => (
                <option key={app.id} value={app.id}>
                  {app.label}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>{tx("Provider type")}</span>
            <select
              value={draft.providerTypeId}
              onChange={(event) => {
                const nextEntry = entries.find((item) => item.providerTypeId === event.target.value);
                patch({
                  providerTypeId: event.target.value,
                  baseUrl: nextEntry?.defaults.baseUrl || draft.baseUrl,
                  apiFormat: nextEntry?.defaults.apiFormat || draft.apiFormat,
                  model: nextEntry?.defaults.model || draft.model,
                });
              }}
            >
              {entries
                .filter((item) => item.uiVisible)
                .map((item) => (
                  <option key={item.providerTypeId} value={item.providerTypeId}>
                    {item.label}
                  </option>
                ))}
            </select>
          </label>
          <label>
            <span>{tx("Name")}</span>
            <input value={draft.name} onChange={(event) => patch({ name: event.target.value })} />
          </label>
          <div className="universal-icon-editor provider-icon-editor">
            <div className="provider-icon-frame universal-icon-frame">
              <ProviderIcon
                icon={previewIcon.icon}
                name={draft.name || draft.providerTypeId || "Provider"}
                color={previewIcon.color}
                size={24}
              />
            </div>
            <IconPicker
              label={tx("Icon")}
              value={draft.icon}
              fallbackIcon={inferredPreviewIcon.icon}
              fallbackColor={previewIcon.color}
              providerName={draft.name || draft.providerTypeId || "Provider"}
              onChange={(value) => patch({ icon: value })}
            />
            <ColorPicker
              label={tx("Color")}
              value={draft.iconColor}
              fallback={colorInputValue(previewIcon.color)}
              onChange={(value) => patch({ iconColor: value })}
            />
          </div>
          <ProviderModelField
            draft={draft}
            options={modelOptionsFromCatalogJson(draft.modelCatalogJson)}
            busy={modelFetchBusy}
            result={modelFetchResult}
            onChange={(model) => patch({ model })}
            onFetch={draft.mode === "edit" && draft.id ? fetchModelsForDraft : undefined}
          />
          <label className="wide-field">
            <span>{tx("Category")}</span>
            <input
              value={draft.category}
              onChange={(event) => patch({ category: event.target.value })}
            />
          </label>
          {entry && (
            <ProviderAuthSection
              draft={draft}
              entry={entry}
              accountOptions={accountOptions}
              onPatch={patch}
            />
          )}
          <ProviderDesktopAdvancedSection
            draft={draft}
            onPatch={patch}
          />
          <details className="wide-field provider-advanced-section" open={hasAdvancedConfig || undefined}>
            <summary>
              <Boxes size={16} />
              <span>{tx("Advanced configuration")}</span>
              <small>{tx("Model catalog, mapping, pricing, and provider JSON overrides")}</small>
            </summary>
            <div className="universal-json-section">
              <div className="universal-json-grid">
                <ProviderJsonField
                  title={tx("Model Catalog")}
                  label="modelCatalog JSON"
                  value={draft.modelCatalogJson}
                  placeholder={providerModelCatalogPlaceholder(draft.app)}
                  onChange={(value) => patch({ modelCatalogJson: value })}
                />
                <ProviderJsonField
                  title={tx("Model Mapping")}
                  label="modelMapping JSON"
                  value={draft.modelMappingJson}
                  placeholder={providerModelMappingPlaceholder(draft.app)}
                  onChange={(value) => patch({ modelMappingJson: value })}
                />
                <ProviderJsonField
                  title={tx("Pricing")}
                  label="pricing JSON"
                  value={draft.pricingJson}
                  placeholder={providerPricingPlaceholder()}
                  onChange={(value) => patch({ pricingJson: value })}
                />
              </div>
              <div className="json-editor-field">
                <span>{tx("Advanced provider JSON")}</span>
                <JsonEditor
                  value={draft.advancedJson}
                  onChange={(value) => patch({ advancedJson: value })}
                  rows={10}
                />
              </div>
            </div>
          </details>
        </div>
        <footer>
          <button className="secondary-button" type="button" onClick={onClose}>
            {tx("Cancel")}
          </button>
          <button className="primary-button" type="submit" disabled={saving}>
            {saving && <Loader2 size={15} />}
            <span>{tx("Save Provider")}</span>
          </button>
        </footer>
      </form>
    </div>
  );
}
