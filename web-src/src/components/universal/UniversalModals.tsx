import { ArrowUpAZ, Copy, Search } from "lucide-react";
import { useMemo, useState } from "react";

import { inferIconForText } from "@/config/iconInference";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ModalFooter } from "@/components/ModalFooter";
import { ProviderIcon } from "@/components/ProviderIcon";
import { SimpleModal } from "@/components/SimpleModal";
import { UniversalProvider, UniversalProviderPreset } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { errorMessage } from "@/components/universal/UniversalFormModal";

export function ImportUniversalModal({
  saving,
  onClose,
  onSubmit,
}: {
  saving: boolean;
  onClose: () => void;
  onSubmit: (providers: UniversalProvider[]) => void;
}) {
  const { tx } = useI18n();
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pendingProviders, setPendingProviders] = useState<UniversalProvider[] | null>(null);
  return (
    <>
      <SimpleModal title="Import Universal Providers" subtitle="Paste an exported array or { providers } object." onClose={onClose}>
        <form
          className="modal-form-stack"
          onSubmit={(event) => {
            event.preventDefault();
            try {
              const parsed = JSON.parse(text) as { providers?: UniversalProvider[] } | UniversalProvider[];
              const providers = Array.isArray(parsed) ? parsed : parsed.providers;
              if (!providers?.length) throw new Error(tx("providers array is required"));
              setError(null);
              setPendingProviders(providers);
            } catch (reason) {
              setError(errorMessage(reason));
            }
          }}
        >
          {error && <div className="form-error">{error}</div>}
          <textarea value={text} onChange={(event) => setText(event.target.value)} />
          <ModalFooter saving={saving} onClose={onClose} label="Import" />
        </form>
      </SimpleModal>
      <ConfirmDialog
        isOpen={pendingProviders !== null}
        title={tx("Import universal providers")}
        message={tx("Import {{count}} universal providers? Existing providers with the same IDs may be updated.", {
          count: pendingProviders?.length || 0,
        })}
        confirmText={tx("Import")}
        onConfirm={() => {
          const providers = pendingProviders;
          setPendingProviders(null);
          if (providers) onSubmit(providers);
        }}
        onCancel={() => setPendingProviders(null)}
      />
    </>
  );
}

export function UniversalPresetModal({
  presets,
  onSelect,
  onClose,
}: {
  presets: UniversalProviderPreset[];
  onSelect: (preset: UniversalProviderPreset) => void;
  onClose: () => void;
}) {
  const { tx } = useI18n();
  const [query, setQuery] = useState("");
  const [sortMode, setSortMode] = useState<"recommended" | "name">("recommended");
  const visiblePresets = useMemo(
    () => filterUniversalPresets(presets, query, sortMode),
    [presets, query, sortMode],
  );
  return (
    <SimpleModal title="Create Universal From Preset" subtitle="Preset defaults are loaded into the editable form before saving." onClose={onClose}>
      <div className="provider-catalog-toolbar universal-preset-toolbar">
        <label className="provider-catalog-search">
          <Search size={15} />
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={tx("Search universal presets")}
          />
        </label>
        <button
          className={sortMode === "name" ? "secondary-button compact active" : "secondary-button compact"}
          type="button"
          onClick={() => setSortMode((current) => (current === "name" ? "recommended" : "name"))}
          aria-label={tx("Sort presets")}
          title={tx("Sort presets")}
        >
          <ArrowUpAZ size={14} />
          <span>{tx(sortMode === "name" ? "A-Z" : "recommended")}</span>
        </button>
        <span className="provider-catalog-count">
          {tx("{{count}} presets", { count: visiblePresets.length })}
        </span>
      </div>
      <div className="provider-preset-grid">
        {visiblePresets.length ? (
          visiblePresets.map((preset) => {
            const icon = universalPresetIcon(preset);
            return (
              <button
                className="provider-preset-card"
                type="button"
                key={preset.providerType}
                onClick={() => onSelect(preset)}
              >
                <span className="provider-preset-title">
                  <span className="provider-icon-frame small">
                    <ProviderIcon
                      icon={icon.icon}
                      name={preset.name}
                      color={icon.color}
                      size={18}
                    />
                  </span>
                  <strong>{preset.name}</strong>
                </span>
                <span>{preset.providerType}</span>
                <small>{preset.description || tx("Universal provider template")}</small>
              </button>
            );
          })
        ) : (
          <div className="provider-empty inline-empty">{tx("No universal presets match this search")}</div>
        )}
      </div>
    </SimpleModal>
  );
}

export function UniversalExportModal({
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
      title="Export Universal Providers"
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

function filterUniversalPresets(
  presets: UniversalProviderPreset[],
  query: string,
  sortMode: "recommended" | "name",
): UniversalProviderPreset[] {
  const normalizedQuery = query.trim().toLowerCase();
  const filtered = normalizedQuery
    ? presets.filter((preset) =>
        [
          preset.name,
          preset.providerType,
          preset.description,
          preset.websiteUrl,
          preset.icon,
        ]
          .filter(Boolean)
          .join(" ")
          .toLowerCase()
          .includes(normalizedQuery),
      )
    : presets;
  if (sortMode === "recommended") return filtered;
  return [...filtered].sort((left, right) => left.name.localeCompare(right.name));
}

function universalPresetIcon(preset: UniversalProviderPreset): { icon?: string; color?: string } {
  if (preset.icon) return { icon: preset.icon, color: preset.iconColor || undefined };
  const inferred = inferIconForText(preset.name, preset.providerType, preset.websiteUrl, preset.description);
  return { icon: inferred.icon, color: inferred.iconColor };
}
