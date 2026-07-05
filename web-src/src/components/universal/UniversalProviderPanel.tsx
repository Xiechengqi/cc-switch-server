import {
  closestCenter,
  DndContext,
  DragEndEvent,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import {
  arrayMove,
  rectSortingStrategy,
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import {
  ArrowUpAZ,
  Copy,
  Download,
  Edit3,
  Globe,
  GripVertical,
  ListPlus,
  Loader2,
  Plus,
  RefreshCw,
  RotateCcw,
  Search,
  Trash2,
  Upload,
} from "lucide-react";
import {
  CSSProperties,
  FormEvent,
  HTMLAttributes,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from "react";

import {
  deleteUniversalProvider,
  exportUniversalProviders,
  importUniversalProviders,
  loadUniversalProviderPresets,
  loadUniversalProviders,
  saveUniversalProvider,
  syncUniversalProvider,
  UniversalProvider,
  UniversalProviderPreset,
} from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { inferIconForText } from "@/config/iconInference";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { IconAction } from "@/components/IconAction";
import { KeyValue } from "@/components/KeyValue";
import { LoadingBlock } from "@/components/LoadingBlock";
import { ModalFooter } from "@/components/ModalFooter";
import { SimpleModal } from "@/components/SimpleModal";
import { JsonPreview } from "@/components/JsonPreview";
import { ProviderIcon } from "@/components/ProviderIcon";
import { StatusPill } from "@/components/StatusPill";
import { SortableUniversalCard } from "@/components/universal/UniversalCard";
import { UniversalListToolbar } from "@/components/universal/UniversalListToolbar";
import { UniversalEmptyState } from "@/components/universal/UniversalEmptyState";
import {
  draftFromPreset,
  draftFromProvider,
  emptyDraft,
  enabledUniversalApps,
  errorMessage,
  providerFromDraft,
  syncSummary,
  UniversalFormModal,
  type UniversalDraft,
} from "@/components/universal/UniversalFormModal";

const universalApps = ["claude", "codex", "gemini"] as const;

export function UniversalProviderPanel() {
  const { t, tx } = useI18n();
  const [providers, setProviders] = useState<Record<string, UniversalProvider>>({});
  const [presets, setPresets] = useState<UniversalProviderPreset[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [draft, setDraft] = useState<UniversalDraft | null>(null);
  const [presetOpen, setPresetOpen] = useState(false);
  const [importOpen, setImportOpen] = useState(false);
  const [exportText, setExportText] = useState<string | null>(null);
  const [exportCopyStatus, setExportCopyStatus] = useState<{ tone: "success" | "warning"; message: string } | null>(null);
  const [providerQuery, setProviderQuery] = useState("");
  const [submitMode, setSubmitMode] = useState<"save" | "save-sync">("save");
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  const list = useMemo(
    () =>
      Object.values(providers).sort((left, right) => {
        const sort = (left.sortIndex ?? 0) - (right.sortIndex ?? 0);
        return sort || left.name.localeCompare(right.name) || left.id.localeCompare(right.id);
      }),
    [providers],
  );
  const visibleProviders = useMemo(
    () => filterUniversalProviders(list, providerQuery),
    [list, providerQuery],
  );

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [providerResult, presetResult] = await Promise.all([
        loadUniversalProviders(),
        loadUniversalProviderPresets(),
      ]);
      setProviders(providerResult);
      setPresets(presetResult);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  async function runAction(action: string, task: () => Promise<string>) {
    setBusy(action);
    setError(null);
    try {
      setResult(await task());
      await refresh();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function submitDraft(event: FormEvent) {
    event.preventDefault();
    if (!draft) return;
    const mode = submitMode;
    setSubmitMode("save");
    await runAction(mode, async () => {
      const saved = await saveUniversalProvider(providerFromDraft(draft));
      if (mode === "save-sync") {
        const sync = await syncUniversalProvider(saved.id);
        setDraft(null);
        return tx("saved {{name}}; {{summary}}", { name: saved.name, summary: syncSummary(sync, tx) });
      }
      setDraft(null);
      return tx("saved {{name}}", { name: saved.name });
    });
  }

  async function syncProvider(provider: UniversalProvider) {
    await runAction(`sync:${provider.id}`, async () => syncSummary(await syncUniversalProvider(provider.id), tx));
  }

  async function deleteProvider(provider: UniversalProvider) {
    await runAction(`delete:${provider.id}`, async () => {
      const deleted = await deleteUniversalProvider(provider.id);
      return deleted ? tx("deleted {{name}}", { name: provider.name }) : tx("{{name}} was not found", { name: provider.name });
    });
  }

  async function duplicateProvider(provider: UniversalProvider) {
    const copy: UniversalProvider = {
      ...provider,
      id: `${provider.id}-copy-${Date.now().toString(36)}`,
      name: `${provider.name} Copy`,
      sortIndex: Date.now(),
    };
    await runAction(`duplicate:${provider.id}`, async () => {
      const saved = await saveUniversalProvider(copy);
      const sync = await syncUniversalProvider(saved.id);
      return tx("duplicated {{name}}; {{summary}}", { name: saved.name, summary: syncSummary(sync, tx) });
    });
  }

  async function handleUniversalDragEnd(event: DragEndEvent) {
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const oldIndex = list.findIndex((provider) => provider.id === active.id);
    const newIndex = list.findIndex((provider) => provider.id === over.id);
    if (oldIndex < 0 || newIndex < 0) return;

    const reordered = arrayMove(list, oldIndex, newIndex);
    const sortedProviders = reordered.map((provider, index) => ({
      ...provider,
      sortIndex: index,
    }));
    setProviders((current) => {
      const next = { ...current };
      for (const provider of sortedProviders) {
        next[provider.id] = provider;
      }
      return next;
    });
    setError(null);
    try {
      await Promise.all(sortedProviders.map((provider) => saveUniversalProvider(provider)));
    } catch (reason) {
      setError(errorMessage(reason));
      await refresh();
    }
  }

  async function exportAction() {
    await runAction("export", async () => {
      const exported = await exportUniversalProviders();
      const text = JSON.stringify(exported, null, 2);
      setExportText(text);
      setExportCopyStatus(null);
      let copied = false;
      try {
        if (navigator.clipboard) {
          await navigator.clipboard.writeText(text);
          copied = true;
        }
      } catch {
        copied = false;
      }
      setExportCopyStatus({
        tone: copied ? "success" : "warning",
        message: copied ? tx("Copied JSON") : tx("Clipboard unavailable; copy the visible value manually."),
      });
      return copied
        ? tx("exported {{count}} providers to clipboard", { count: exported.length })
        : tx("exported {{count}} providers", { count: exported.length });
    });
  }

  async function copyExportText() {
    if (!exportText) return;
    if (!navigator.clipboard?.writeText) {
      setExportCopyStatus({ tone: "warning", message: tx("Clipboard unavailable; copy the visible value manually.") });
      return;
    }
    try {
      await navigator.clipboard.writeText(exportText);
      setExportCopyStatus({ tone: "success", message: tx("Copied JSON") });
    } catch {
      setExportCopyStatus({ tone: "warning", message: tx("Copy failed; copy the visible value manually.") });
    }
  }

  return (
    <div className="universal-provider-panel">
      <div className="provider-toolbar">
        <div className="provider-toolbar-status">
          <span>{t("server.universal.templates", { count: list.length })}</span>
        </div>
        <div className="provider-toolbar-actions">
          {error && <span className="error-text">{error}</span>}
          {result && <span className="usage-result">{result}</span>}
          <button className="secondary-button" type="button" onClick={() => void refresh()}>
            <RefreshCw size={15} />
            <span>{t("common.refresh")}</span>
          </button>
          <button className="secondary-button" type="button" onClick={() => void exportAction()} disabled={busy === "export"}>
            {busy === "export" ? <Loader2 size={15} /> : <Download size={15} />}
            <span>{t("server.common.export")}</span>
          </button>
          <button className="secondary-button" type="button" onClick={() => setImportOpen(true)}>
            <Upload size={15} />
            <span>{t("common.import")}</span>
          </button>
          <button className="secondary-button" type="button" onClick={() => setPresetOpen(true)} disabled={!presets.length}>
            <ListPlus size={15} />
            <span>{t("server.common.fromPreset")}</span>
          </button>
          <button className="primary-button" type="button" onClick={() => setDraft(emptyDraft())}>
            <Plus size={15} />
            <span>{t("server.universal.add")}</span>
          </button>
        </div>
      </div>

      <UniversalListToolbar
        query={providerQuery}
        visible={visibleProviders.length}
        total={list.length}
        onQueryChange={setProviderQuery}
      />

      {loading ? (
        <LoadingBlock label="server.universal.loading" />
      ) : list.length ? (
        visibleProviders.length ? (
          <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={(event) => void handleUniversalDragEnd(event)}>
            <SortableContext items={list.map((provider) => provider.id)} strategy={rectSortingStrategy}>
              <div className="universal-card-grid">
                {visibleProviders.map((provider) => (
                  <SortableUniversalCard
                    key={provider.id}
                    provider={provider}
                    busy={busy}
                    onEdit={() => setDraft(draftFromProvider(provider))}
                    onSync={() => void syncProvider(provider)}
                    onDuplicate={() => void duplicateProvider(provider)}
                    onDelete={() => void deleteProvider(provider)}
                  />
                ))}
              </div>
            </SortableContext>
          </DndContext>
        ) : (
          <div className="provider-empty compact-empty">
            <Search size={20} />
            <span>{tx("No universal providers match the current search")}</span>
          </div>
        )
      ) : (
        <UniversalEmptyState
          canUsePresets={presets.length > 0}
          onImport={() => setImportOpen(true)}
          onPreset={() => setPresetOpen(true)}
          onCreate={() => setDraft(emptyDraft())}
        />
      )}

      {draft && (
        <UniversalFormModal
          draft={draft}
          saving={busy === "save" || busy === "save-sync"}
          savingMode={busy === "save-sync" ? "save-sync" : busy === "save" ? "save" : null}
          onSubmitMode={setSubmitMode}
          onChange={setDraft}
          onClose={() => setDraft(null)}
          onSubmit={submitDraft}
        />
      )}

      {presetOpen && (
        <UniversalPresetModal
          presets={presets}
          onSelect={(preset) => {
            setDraft(draftFromPreset(preset));
            setPresetOpen(false);
          }}
          onClose={() => setPresetOpen(false)}
        />
      )}

      {importOpen && (
        <ImportUniversalModal
          saving={busy === "import"}
          onClose={() => setImportOpen(false)}
          onSubmit={(providersToImport) =>
            void runAction("import", async () => {
              const imported = await importUniversalProviders(providersToImport);
              setImportOpen(false);
              return tx("imported {{count}} universal providers", { count: imported });
            })
          }
        />
      )}

      {exportText && (
        <SimpleModal
          title="Export Universal Providers"
          subtitle="Copy this JSON when clipboard access is unavailable."
          onClose={() => setExportText(null)}
        >
          <textarea readOnly value={exportText} />
          {exportCopyStatus && <div className={`connect-copy-status ${exportCopyStatus.tone}`}>{exportCopyStatus.message}</div>}
          <footer className="modal-inline-footer">
            <button className="secondary-button" type="button" onClick={() => void copyExportText()}>
              <Copy size={15} />
              <span>{tx("Copy JSON")}</span>
            </button>
            <button className="secondary-button" type="button" onClick={() => setExportText(null)}>
              {tx("Close")}
            </button>
          </footer>
        </SimpleModal>
      )}
    </div>
  );
}

function filterUniversalProviders(providers: UniversalProvider[], query: string): UniversalProvider[] {
  const normalizedQuery = query.trim().toLowerCase();
  if (!normalizedQuery) return providers;
  return providers.filter((provider) => {
    const modelValues = universalApps.flatMap((app) => {
      const model = provider.models?.[app];
      if (!model) return [];
      const record = model as Record<string, unknown>;
      return [
        record.model,
        record.haikuModel,
        record.sonnetModel,
        record.opusModel,
        record.reasoningEffort,
      ];
    });
    return [
      provider.id,
      provider.name,
      provider.providerType,
      provider.baseUrl,
      provider.websiteUrl,
      provider.notes,
      provider.icon,
      ...enabledUniversalApps(provider),
      ...modelValues,
    ]
      .filter(Boolean)
      .join(" ")
      .toLowerCase()
      .includes(normalizedQuery);
  });
}

function ImportUniversalModal({
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

function UniversalPresetModal({
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
          visiblePresets.map((preset) => (
            <button
              className="provider-preset-card"
              type="button"
              key={preset.providerType}
              onClick={() => onSelect(preset)}
            >
              <span className="provider-preset-title">
                <span className="provider-icon-frame small">
                  <ProviderIcon
                    icon={universalPresetIcon(preset).icon}
                    name={preset.name}
                    color={universalPresetIcon(preset).color}
                    size={18}
                  />
                </span>
                <strong>{preset.name}</strong>
              </span>
              <span>{preset.providerType}</span>
              <small>{preset.description || tx("Universal provider template")}</small>
            </button>
          ))
        ) : (
          <div className="provider-empty inline-empty">{tx("No universal presets match this search")}</div>
        )}
      </div>
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
