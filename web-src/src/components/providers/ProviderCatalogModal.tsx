import { ArrowUpAZ, ListPlus, Loader2, Search, ServerCog, X } from "lucide-react";
import { useMemo, useState } from "react";

import { AppKind, ProviderMatrixEntry, ProviderPresetSummary } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { inferIconForText } from "@/config/iconInference";
import { ProviderIcon } from "@/components/ProviderIcon";
import { appLabel } from "@/components/providers/providerDisplay";
import { presetIcon } from "@/lib/provider-icons";

export function ProviderCatalogModal({
  app,
  entries,
  presets,
  busyId,
  onSelectEntry,
  onSelect,
  onClose,
}: {
  app: AppKind;
  entries: ProviderMatrixEntry[];
  presets: ProviderPresetSummary[];
  busyId: string | null;
  onSelectEntry: (entry: ProviderMatrixEntry) => void;
  onSelect: (preset: ProviderPresetSummary) => void;
  onClose: () => void;
}) {
  const { tx } = useI18n();
  const [query, setQuery] = useState("");
  const [sortMode, setSortMode] = useState<"recommended" | "name">("recommended");
  const visiblePresets = useMemo(
    () => filterCatalogPresets(presets, query, sortMode),
    [presets, query, sortMode],
  );
  const visibleEntries = useMemo(
    () => filterCatalogEntries(entries, query, sortMode),
    [entries, query, sortMode],
  );
  return (
    <div className="modal-backdrop" role="presentation">
      <section className="provider-form-modal simple-modal provider-catalog-modal">
        <header>
          <div>
            <h2>{tx("Add Provider")}</h2>
            <p>{tx("Choose a desktop preset or provider type for {{app}}", { app: appLabel(app) })}</p>
          </div>
          <button className="icon-button" type="button" onClick={onClose} aria-label={tx("Close")}>
            <X size={16} />
          </button>
        </header>
        <div className="provider-catalog-body">
          <div className="provider-catalog-toolbar">
            <label className="provider-catalog-search">
              <Search size={15} />
              <input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder={tx("Search presets and provider types")}
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
              {tx("{{presets}} presets / {{types}} types", {
                presets: visiblePresets.length,
                types: visibleEntries.length,
              })}
            </span>
          </div>
          <section className="provider-catalog-section">
            <div className="section-title-row compact-title">
              <ListPlus size={16} />
              <div>
                <h3>{tx("Presets")}</h3>
                <span>{tx("Create with curated desktop defaults")}</span>
              </div>
            </div>
            <div className="provider-preset-grid">
              {visiblePresets.length ? (
                visiblePresets.map((preset) => {
                  const busy = busyId === `preset:${app}:${preset.name}`;
                  const icon = presetIcon(preset);
                  return (
                    <button
                      className="provider-preset-card"
                      type="button"
                      key={preset.name}
                      onClick={() => onSelect(preset)}
                      disabled={busy}
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
                      <span>{preset.providerType || "provider"}</span>
                      <small>{preset.apiFormat || "api format -"} · {preset.baseUrl || "base URL -"}</small>
                      {busy && <Loader2 size={15} />}
                    </button>
                  );
                })
              ) : (
                <div className="provider-empty inline-empty">
                  {query.trim() ? tx("No presets match this search") : tx("No presets for {{app}}", { app: appLabel(app) })}
                </div>
              )}
            </div>
          </section>

          <section className="provider-catalog-section">
            <div className="section-title-row compact-title">
              <ServerCog size={16} />
              <div>
                <h3>{tx("Provider Types")}</h3>
                <span>{tx("Start from a server-supported adapter type")}</span>
              </div>
            </div>
            <div className="provider-type-grid catalog-type-grid">
              {visibleEntries.length ? (
                visibleEntries.map((entry) => {
                  const icon = entryIcon(entry);
                  return (
                    <button
                      className="provider-type-option catalog-type-option"
                      type="button"
                      key={entry.providerTypeId}
                      onClick={() => onSelectEntry(entry)}
                    >
                      <span className="provider-preset-title">
                        <span className="provider-icon-frame small">
                          <ProviderIcon
                            icon={icon.icon}
                            name={entry.label}
                            color={icon.color}
                            size={18}
                          />
                        </span>
                        <strong>{entry.label}</strong>
                      </span>
                      <span>{entry.defaults.apiFormat || entry.providerType}</span>
                      <small>{entry.defaults.baseUrl || entry.note || tx("Manual configuration")}</small>
                    </button>
                  );
                })
              ) : (
                <div className="provider-empty inline-empty">
                  {query.trim() ? tx("No provider types match this search") : tx("No provider types for {{app}}", { app: appLabel(app) })}
                </div>
              )}
            </div>
          </section>
        </div>
      </section>
    </div>
  );
}

function filterCatalogPresets(
  presets: ProviderPresetSummary[],
  query: string,
  sortMode: "recommended" | "name",
): ProviderPresetSummary[] {
  const normalizedQuery = query.trim().toLowerCase();
  const filtered = normalizedQuery
    ? presets.filter((preset) =>
        [
          preset.name,
          preset.providerType,
          preset.apiFormat,
          preset.baseUrl,
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

function filterCatalogEntries(
  entries: ProviderMatrixEntry[],
  query: string,
  sortMode: "recommended" | "name",
): ProviderMatrixEntry[] {
  const normalizedQuery = query.trim().toLowerCase();
  const filtered = normalizedQuery
    ? entries.filter((entry) =>
        [
          entry.label,
          entry.providerType,
          entry.providerTypeId,
          entry.defaults.apiFormat,
          entry.defaults.baseUrl,
          entry.note,
        ]
          .filter(Boolean)
          .join(" ")
          .toLowerCase()
          .includes(normalizedQuery),
      )
    : entries;
  if (sortMode === "recommended") return filtered;
  return [...filtered].sort((left, right) => left.label.localeCompare(right.label));
}

function entryIcon(entry: ProviderMatrixEntry): { icon?: string; color?: string } {
  const inferred = inferIconForText(
    entry.label,
    entry.providerType,
    entry.providerTypeId,
    entry.defaults.baseUrl,
    entry.defaults.apiFormat,
  );
  return { icon: inferred.icon, color: inferred.iconColor };
}
