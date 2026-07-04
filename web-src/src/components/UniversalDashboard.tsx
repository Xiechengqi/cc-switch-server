import {
  Boxes,
  Copy,
  Download,
  Edit3,
  Globe,
  ListPlus,
  Loader2,
  Plus,
  RefreshCw,
  RotateCcw,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import { FormEvent, ReactNode, useCallback, useEffect, useMemo, useState } from "react";

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
  UniversalProviderSyncResult,
} from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { inferIconForText } from "@/config/iconInference";
import { ProviderIcon } from "@/components/ProviderIcon";

interface UniversalDraft {
  mode: "create" | "edit";
  original?: UniversalProvider;
  id: string;
  name: string;
  providerType: string;
  baseUrl: string;
  apiKey: string;
  websiteUrl: string;
  icon: string;
  iconColor: string;
  notes: string;
  claude: boolean;
  codex: boolean;
  gemini: boolean;
  claudeModel: string;
  claudeHaikuModel: string;
  claudeSonnetModel: string;
  claudeOpusModel: string;
  codexModel: string;
  codexReasoningEffort: string;
  geminiModel: string;
  claudeModelCatalog: string;
  claudeModelMapping: string;
  codexModelCatalog: string;
  codexModelMapping: string;
  geminiModelCatalog: string;
  geminiModelMapping: string;
}

const universalApps = ["claude", "codex", "gemini"] as const;

export function UniversalDashboard() {
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

  const list = useMemo(
    () =>
      Object.values(providers).sort((left, right) => {
        const sort = (left.sortIndex ?? 0) - (right.sortIndex ?? 0);
        return sort || left.name.localeCompare(right.name) || left.id.localeCompare(right.id);
      }),
    [providers],
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
    await runAction("save", async () => {
      const saved = await saveUniversalProvider(providerFromDraft(draft));
      setDraft(null);
      return tx("saved {{name}}", { name: saved.name });
    });
  }

  async function syncProvider(provider: UniversalProvider) {
    await runAction(`sync:${provider.id}`, async () => syncSummary(await syncUniversalProvider(provider.id), tx));
  }

  async function deleteProvider(provider: UniversalProvider) {
    if (!window.confirm(tx("Delete universal provider {{name}}? Derived app providers will be removed.", { name: provider.name }))) {
      return;
    }
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

  async function exportAction() {
    await runAction("export", async () => {
      const exported = await exportUniversalProviders();
      const text = JSON.stringify(exported, null, 2);
      setExportText(text);
      let copied = false;
      try {
        if (navigator.clipboard) {
          await navigator.clipboard.writeText(text);
          copied = true;
        }
      } catch {
        copied = false;
      }
      return copied
        ? tx("exported {{count}} providers to clipboard", { count: exported.length })
        : tx("exported {{count}} providers", { count: exported.length });
    });
  }

  return (
    <div className="universal-dashboard">
      <div className="provider-toolbar">
        <div className="section-title-row">
          <Boxes size={18} />
          <div>
            <h2>{t("server.universal.title")}</h2>
            <span>{t("server.universal.templates", { count: list.length })}</span>
          </div>
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

      {loading ? (
        <div className="provider-empty">
          <Loader2 size={22} />
          <span>{t("server.universal.loading")}</span>
        </div>
      ) : list.length ? (
        <div className="universal-card-grid">
          {list.map((provider) => (
            <UniversalCard
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
      ) : (
        <div className="provider-empty">
          <Boxes size={24} />
          <strong>{tx("No universal providers")}</strong>
          <span>{tx("Create one template, then sync it into Claude, Codex and Gemini providers.")}</span>
        </div>
      )}

      {draft && (
        <UniversalFormModal
          draft={draft}
          saving={busy === "save"}
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
              return `imported ${imported} universal providers`;
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
          <footer className="modal-inline-footer">
            <button className="secondary-button" type="button" onClick={() => setExportText(null)}>
              {tx("Close")}
            </button>
          </footer>
        </SimpleModal>
      )}
    </div>
  );
}

function UniversalCard({
  provider,
  busy,
  onEdit,
  onSync,
  onDuplicate,
  onDelete,
}: {
  provider: UniversalProvider;
  busy: string | null;
  onEdit: () => void;
  onSync: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
}) {
  const { tx } = useI18n();
  const icon = universalProviderIcon(provider);
  const enabledApps = enabledUniversalApps(provider);
  return (
    <article className="provider-card universal-card">
      <header className="universal-card-header">
        <div className="universal-card-title-row">
          <div className="provider-icon-frame universal-icon-frame">
            <ProviderIcon
              icon={icon.icon}
              name={provider.name}
              color={icon.color}
              size={24}
            />
          </div>
          <div className="universal-card-title">
            <h3>{provider.name}</h3>
            <p>{provider.providerType}</p>
          </div>
        </div>
        <div className="universal-card-actions">
          <IconAction title="Sync" busy={busy === `sync:${provider.id}`} onClick={onSync}>
            <RotateCcw size={15} />
          </IconAction>
          <IconAction title="Duplicate" busy={busy === `duplicate:${provider.id}`} onClick={onDuplicate}>
            <Copy size={15} />
          </IconAction>
          <IconAction title="Edit" onClick={onEdit}>
            <Edit3 size={15} />
          </IconAction>
          <IconAction title="Delete" busy={busy === `delete:${provider.id}`} onClick={onDelete} danger>
            <Trash2 size={15} />
          </IconAction>
        </div>
      </header>
      <div className="universal-url-row">
        <Globe size={14} />
        <span>{provider.baseUrl || provider.websiteUrl || "-"}</span>
        <StatusPill tone={provider.apiKey ? "success" : "warning"}>
          {tx(provider.apiKey ? "key" : "no key")}
        </StatusPill>
      </div>
      <div className="universal-app-row">
        {enabledApps.length ? (
          enabledApps.map((app) => <AppBadge key={app} label={app} enabled />)
        ) : (
          <span className="universal-no-apps">{tx("No apps enabled")}</span>
        )}
      </div>
      <div className="universal-model-strip">
        {provider.apps.claude && <KeyValue label="claude" value={provider.models?.claude?.model || "-"} />}
        {provider.apps.codex && <KeyValue label="codex" value={provider.models?.codex?.model || "-"} />}
        {provider.apps.gemini && <KeyValue label="gemini" value={provider.models?.gemini?.model || "-"} />}
      </div>
      {provider.notes && <div className="provider-card-result">{provider.notes}</div>}
      <details className="json-details">
        <summary>{tx("Config preview")}</summary>
        <div className="provider-card-meta">
          <KeyValue label="website" value={provider.websiteUrl || "-"} />
          <KeyValue label="catalog" value={configuredModelApps(provider, "modelCatalog")} />
          <KeyValue label="mapping" value={configuredModelApps(provider, "modelMapping")} />
          <KeyValue label="id" value={provider.id} />
        </div>
        <JsonPreview value={redactUniversalProvider(provider)} />
      </details>
    </article>
  );
}

function UniversalFormModal({
  draft,
  saving,
  onChange,
  onClose,
  onSubmit,
}: {
  draft: UniversalDraft;
  saving: boolean;
  onChange: (draft: UniversalDraft) => void;
  onClose: () => void;
  onSubmit: (event: FormEvent) => void;
}) {
  const { tx } = useI18n();
  function patch(next: Partial<UniversalDraft>) {
    onChange({ ...draft, ...next });
  }
  const inferredPreviewIcon = inferIconForText(draft.name, draft.providerType, draft.baseUrl, draft.websiteUrl);
  const previewIcon = draft.icon
    ? { icon: draft.icon, color: draft.iconColor }
    : { icon: inferredPreviewIcon.icon, color: inferredPreviewIcon.iconColor };
  return (
    <div className="modal-backdrop" role="presentation">
      <form className="provider-form-modal universal-form-modal" onSubmit={onSubmit}>
        <header>
          <div>
            <h2>{tx(draft.mode === "create" ? "Add Universal Provider" : "Edit Universal Provider")}</h2>
            <p>{tx("One provider template can derive Claude, Codex and Gemini providers.")}</p>
          </div>
          <button className="icon-button" type="button" onClick={onClose} aria-label={tx("Close")}>
            <X size={16} />
          </button>
        </header>
        <div className="provider-form-grid">
          <TextField label="ID" value={draft.id} disabled={draft.mode === "edit"} onChange={(value) => patch({ id: value })} />
          <TextField label="Name" value={draft.name} onChange={(value) => patch({ name: value })} />
          <TextField label="Provider type" value={draft.providerType} onChange={(value) => patch({ providerType: value })} />
          <TextField label="Base URL" value={draft.baseUrl} onChange={(value) => patch({ baseUrl: value })} />
          <TextField label="API key" value={draft.apiKey} onChange={(value) => patch({ apiKey: value })} />
          <TextField label="Website URL" value={draft.websiteUrl} onChange={(value) => patch({ websiteUrl: value })} />
          <div className="universal-icon-editor">
            <div className="provider-icon-frame universal-icon-frame">
              <ProviderIcon
                icon={previewIcon.icon}
                name={draft.name || draft.providerType || "Universal"}
                color={previewIcon.color}
                size={24}
              />
            </div>
            <label>
              <span>{tx("Icon")}</span>
              <input value={draft.icon} onChange={(event) => patch({ icon: event.target.value })} />
            </label>
            <label>
              <span>{tx("Color")}</span>
              <input
                type="color"
                value={colorInputValue(draft.iconColor || previewIcon.color)}
                onChange={(event) => patch({ iconColor: event.target.value })}
              />
            </label>
          </div>
          <label className="toggle-row">
            <input type="checkbox" checked={draft.claude} onChange={(event) => patch({ claude: event.target.checked })} />
            <span>Claude</span>
          </label>
          <label className="toggle-row">
            <input type="checkbox" checked={draft.codex} onChange={(event) => patch({ codex: event.target.checked })} />
            <span>Codex</span>
          </label>
          <label className="toggle-row">
            <input type="checkbox" checked={draft.gemini} onChange={(event) => patch({ gemini: event.target.checked })} />
            <span>Gemini</span>
          </label>
          <TextField label="Claude model" value={draft.claudeModel} onChange={(value) => patch({ claudeModel: value })} />
          <TextField label="Claude haiku" value={draft.claudeHaikuModel} onChange={(value) => patch({ claudeHaikuModel: value })} />
          <TextField label="Claude sonnet" value={draft.claudeSonnetModel} onChange={(value) => patch({ claudeSonnetModel: value })} />
          <TextField label="Claude opus" value={draft.claudeOpusModel} onChange={(value) => patch({ claudeOpusModel: value })} />
          <TextField label="Codex model" value={draft.codexModel} onChange={(value) => patch({ codexModel: value })} />
          <TextField label="Codex reasoning" value={draft.codexReasoningEffort} onChange={(value) => patch({ codexReasoningEffort: value })} />
          <TextField label="Gemini model" value={draft.geminiModel} onChange={(value) => patch({ geminiModel: value })} />
          <div className="wide-field universal-json-section">
            <div className="section-title-row compact-title">
              <Boxes size={16} />
              <div>
                <h3>{tx("Model catalog and mapping")}</h3>
                <span>{tx("Optional JSON stored on each derived provider.")}</span>
              </div>
            </div>
            <div className="universal-json-grid">
              {draft.claude && (
                <ModelJsonFields
                  app="Claude"
                  catalog={draft.claudeModelCatalog}
                  mapping={draft.claudeModelMapping}
                  onCatalog={(value) => patch({ claudeModelCatalog: value })}
                  onMapping={(value) => patch({ claudeModelMapping: value })}
                />
              )}
              {draft.codex && (
                <ModelJsonFields
                  app="Codex"
                  catalog={draft.codexModelCatalog}
                  mapping={draft.codexModelMapping}
                  onCatalog={(value) => patch({ codexModelCatalog: value })}
                  onMapping={(value) => patch({ codexModelMapping: value })}
                />
              )}
              {draft.gemini && (
                <ModelJsonFields
                  app="Gemini"
                  catalog={draft.geminiModelCatalog}
                  mapping={draft.geminiModelMapping}
                  onCatalog={(value) => patch({ geminiModelCatalog: value })}
                  onMapping={(value) => patch({ geminiModelMapping: value })}
                />
              )}
            </div>
          </div>
          <label className="wide-field">
            <span>{tx("Notes")}</span>
            <textarea value={draft.notes} onChange={(event) => patch({ notes: event.target.value })} />
          </label>
        </div>
        <footer>
          <button className="secondary-button" type="button" onClick={onClose}>
            {tx("Cancel")}
          </button>
          <button className="primary-button" type="submit" disabled={saving}>
            {saving && <Loader2 size={15} />}
            <span>{tx("Save Universal")}</span>
          </button>
        </footer>
      </form>
    </div>
  );
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
  return (
    <SimpleModal title="Import Universal Providers" subtitle="Paste an exported array or { providers } object." onClose={onClose}>
      <form
        className="modal-form-stack"
        onSubmit={(event) => {
          event.preventDefault();
          try {
            const parsed = JSON.parse(text) as { providers?: UniversalProvider[] } | UniversalProvider[];
            const providers = Array.isArray(parsed) ? parsed : parsed.providers;
            if (!providers?.length) throw new Error(tx("providers array is required"));
            onSubmit(providers);
          } catch (reason) {
            setError(errorMessage(reason));
          }
        }}
      >
        {error && <div className="form-error">{error}</div>}
        <textarea value={text} onChange={(event) => setText(event.target.value)} />
        <footer className="modal-inline-footer">
          <button className="secondary-button" type="button" onClick={onClose}>
            {tx("Cancel")}
          </button>
          <button className="primary-button" type="submit" disabled={saving}>
            {saving && <Loader2 size={15} />}
            <span>{tx("Import")}</span>
          </button>
        </footer>
      </form>
    </SimpleModal>
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
  return (
    <SimpleModal title="Create Universal From Preset" subtitle="Preset defaults are loaded into the editable form before saving." onClose={onClose}>
      <div className="provider-preset-grid">
        {presets.map((preset) => (
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
        ))}
      </div>
    </SimpleModal>
  );
}

function TextField({
  label,
  value,
  disabled,
  onChange,
}: {
  label: string;
  value: string;
  disabled?: boolean;
  onChange: (value: string) => void;
}) {
  const { tx } = useI18n();
  return (
    <label>
      <span>{tx(label)}</span>
      <input value={value} disabled={disabled} onChange={(event) => onChange(event.target.value)} />
    </label>
  );
}

function ModelJsonFields({
  app,
  catalog,
  mapping,
  onCatalog,
  onMapping,
}: {
  app: string;
  catalog: string;
  mapping: string;
  onCatalog: (value: string) => void;
  onMapping: (value: string) => void;
}) {
  const { tx } = useI18n();
  return (
    <section className="universal-json-card">
      <h4>{app}</h4>
      <label>
        <span>{tx("modelCatalog JSON")}</span>
        <textarea
          value={catalog}
          onChange={(event) => onCatalog(event.target.value)}
          placeholder={modelCatalogPlaceholder(app)}
          spellCheck={false}
        />
      </label>
      <label>
        <span>{tx("modelMapping JSON")}</span>
        <textarea
          value={mapping}
          onChange={(event) => onMapping(event.target.value)}
          placeholder={modelMappingPlaceholder(app)}
          spellCheck={false}
        />
      </label>
    </section>
  );
}

function AppBadge({ label, enabled }: { label: string; enabled: boolean }) {
  return <span className={enabled ? "universal-app-badge active" : "universal-app-badge"}>{label}</span>;
}

function enabledUniversalApps(provider: UniversalProvider): string[] {
  return [
    provider.apps.claude ? "Claude" : null,
    provider.apps.codex ? "Codex" : null,
    provider.apps.gemini ? "Gemini" : null,
  ].filter((app): app is string => Boolean(app));
}

function universalProviderIcon(provider: UniversalProvider): { icon?: string; color?: string } {
  if (provider.icon) return { icon: provider.icon, color: provider.iconColor || undefined };
  const inferred = inferIconForText(provider.name, provider.providerType, provider.baseUrl, provider.websiteUrl);
  return { icon: inferred.icon, color: inferred.iconColor };
}

function universalPresetIcon(preset: UniversalProviderPreset): { icon?: string; color?: string } {
  if (preset.icon) return { icon: preset.icon, color: preset.iconColor || undefined };
  const inferred = inferIconForText(preset.name, preset.providerType, preset.websiteUrl, preset.description);
  return { icon: inferred.icon, color: inferred.iconColor };
}

function colorInputValue(value?: string): string {
  return value && /^#[0-9a-f]{6}$/i.test(value) ? value : "#111827";
}

function KeyValue({ label, value }: { label: string; value: ReactNode }) {
  const { tx } = useI18n();
  return (
    <div className="compact-kv">
      <span>{tx(label)}</span>
      <strong>{value}</strong>
    </div>
  );
}

function StatusPill({
  children,
  tone,
}: {
  children: ReactNode;
  tone: "success" | "warning" | "danger";
}) {
  return <span className={`status-pill ${tone}`}>{children}</span>;
}

function IconAction({
  title,
  children,
  busy,
  danger,
  onClick,
}: {
  title: string;
  children: ReactNode;
  busy?: boolean;
  danger?: boolean;
  onClick: () => void;
}) {
  const { tx } = useI18n();
  const translatedTitle = tx(title);
  return (
    <button
      className={danger ? "icon-button danger" : "icon-button"}
      type="button"
      title={translatedTitle}
      aria-label={translatedTitle}
      onClick={onClick}
      disabled={busy}
    >
      {busy ? <Loader2 size={15} /> : children}
    </button>
  );
}

function SimpleModal({
  title,
  subtitle,
  children,
  onClose,
}: {
  title: string;
  subtitle?: string;
  children: ReactNode;
  onClose: () => void;
}) {
  const { tx } = useI18n();
  return (
    <div className="modal-backdrop" role="presentation">
      <section className="provider-form-modal simple-modal">
        <header>
          <div>
            <h2>{tx(title)}</h2>
            {subtitle && <p>{tx(subtitle)}</p>}
          </div>
          <button className="icon-button" type="button" onClick={onClose} aria-label={tx("Close")}>
            <X size={16} />
          </button>
        </header>
        <div className="simple-modal-body">{children}</div>
      </section>
    </div>
  );
}

function JsonPreview({ value }: { value: unknown }) {
  return <pre className="json-preview">{JSON.stringify(value, null, 2)}</pre>;
}

function emptyDraft(): UniversalDraft {
  return {
    mode: "create",
    id: "",
    name: "",
    providerType: "openai-compatible",
    baseUrl: "",
    apiKey: "",
    websiteUrl: "",
    icon: "",
    iconColor: "",
    notes: "",
    claude: true,
    codex: true,
    gemini: true,
    claudeModel: "claude-sonnet-4-20250514",
    claudeHaikuModel: "",
    claudeSonnetModel: "",
    claudeOpusModel: "",
    codexModel: "gpt-4o",
    codexReasoningEffort: "high",
    geminiModel: "gemini-2.5-pro",
    claudeModelCatalog: "",
    claudeModelMapping: "",
    codexModelCatalog: "",
    codexModelMapping: "",
    geminiModelCatalog: "",
    geminiModelMapping: "",
  };
}

function draftFromProvider(provider: UniversalProvider): UniversalDraft {
  return {
    ...emptyDraft(),
    mode: "edit",
    original: provider,
    id: provider.id,
    name: provider.name,
    providerType: provider.providerType,
    baseUrl: provider.baseUrl,
    apiKey: provider.apiKey,
    websiteUrl: provider.websiteUrl || "",
    icon: provider.icon || "",
    iconColor: provider.iconColor || "",
    notes: provider.notes || "",
    claude: provider.apps.claude,
    codex: provider.apps.codex,
    gemini: provider.apps.gemini,
    claudeModel: provider.models?.claude?.model || "",
    claudeHaikuModel: provider.models?.claude?.haikuModel || "",
    claudeSonnetModel: provider.models?.claude?.sonnetModel || "",
    claudeOpusModel: provider.models?.claude?.opusModel || "",
    codexModel: provider.models?.codex?.model || "",
    codexReasoningEffort: provider.models?.codex?.reasoningEffort || "",
    geminiModel: provider.models?.gemini?.model || "",
    claudeModelCatalog: jsonText(provider.models?.claude?.modelCatalog),
    claudeModelMapping: jsonText(provider.models?.claude?.modelMapping),
    codexModelCatalog: jsonText(provider.models?.codex?.modelCatalog),
    codexModelMapping: jsonText(provider.models?.codex?.modelMapping),
    geminiModelCatalog: jsonText(provider.models?.gemini?.modelCatalog),
    geminiModelMapping: jsonText(provider.models?.gemini?.modelMapping),
  };
}

function draftFromPreset(preset: UniversalProviderPreset): UniversalDraft {
  const base = emptyDraft();
  const models = preset.defaultModels || {};
  return {
    ...base,
    id: `universal-${slugify(preset.providerType || preset.name)}-${Date.now().toString(36)}`,
    name: preset.name,
    providerType: preset.providerType,
    websiteUrl: preset.websiteUrl || "",
    icon: preset.icon || "",
    iconColor: preset.iconColor || "",
    notes: preset.description || "",
    claude: preset.defaultApps.claude,
    codex: preset.defaultApps.codex,
    gemini: preset.defaultApps.gemini,
    claudeModel: models.claude?.model || "",
    claudeHaikuModel: models.claude?.haikuModel || "",
    claudeSonnetModel: models.claude?.sonnetModel || "",
    claudeOpusModel: models.claude?.opusModel || "",
    codexModel: models.codex?.model || "",
    codexReasoningEffort: models.codex?.reasoningEffort || "",
    geminiModel: models.gemini?.model || "",
    claudeModelCatalog: jsonText(models.claude?.modelCatalog),
    claudeModelMapping: jsonText(models.claude?.modelMapping),
    codexModelCatalog: jsonText(models.codex?.modelCatalog),
    codexModelMapping: jsonText(models.codex?.modelMapping),
    geminiModelCatalog: jsonText(models.gemini?.modelCatalog),
    geminiModelMapping: jsonText(models.gemini?.modelMapping),
  };
}

function providerFromDraft(draft: UniversalDraft): UniversalProvider {
  const original = draft.original || {};
  return {
    ...original,
    id: draft.id.trim(),
    name: draft.name.trim(),
    providerType: draft.providerType.trim() || "openai-compatible",
    baseUrl: draft.baseUrl.trim(),
    apiKey: draft.apiKey,
    websiteUrl: draft.websiteUrl.trim() || null,
    notes: draft.notes.trim() || null,
    icon: draft.icon.trim() || null,
    iconColor: draft.iconColor.trim() || null,
    apps: {
      claude: draft.claude,
      codex: draft.codex,
      gemini: draft.gemini,
    },
    models: {
      ...(draft.original?.models || {}),
      claude: {
        ...(draft.original?.models?.claude || {}),
        model: optionalValue(draft.claudeModel),
        haikuModel: optionalValue(draft.claudeHaikuModel),
        sonnetModel: optionalValue(draft.claudeSonnetModel),
        opusModel: optionalValue(draft.claudeOpusModel),
        modelCatalog: optionalJson(draft.claudeModelCatalog, "Claude modelCatalog"),
        modelMapping: optionalJson(draft.claudeModelMapping, "Claude modelMapping"),
      },
      codex: {
        ...(draft.original?.models?.codex || {}),
        model: optionalValue(draft.codexModel),
        reasoningEffort: optionalValue(draft.codexReasoningEffort),
        modelCatalog: optionalJson(draft.codexModelCatalog, "Codex modelCatalog"),
        modelMapping: optionalJson(draft.codexModelMapping, "Codex modelMapping"),
      },
      gemini: {
        ...(draft.original?.models?.gemini || {}),
        model: optionalValue(draft.geminiModel),
        modelCatalog: optionalJson(draft.geminiModelCatalog, "Gemini modelCatalog"),
        modelMapping: optionalJson(draft.geminiModelMapping, "Gemini modelMapping"),
      },
    },
    createdAt: draft.original?.createdAt ?? Date.now(),
    sortIndex: draft.original?.sortIndex ?? Date.now(),
  } as UniversalProvider;
}

function slugify(value: string): string {
  const slug = value
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug || "provider";
}

function optionalValue(value: string): string | undefined {
  const trimmed = value.trim();
  return trimmed || undefined;
}

function optionalJson(value: string, label: string): unknown {
  const trimmed = value.trim();
  if (!trimmed) return undefined;
  try {
    return JSON.parse(trimmed) as unknown;
  } catch (reason) {
    const suffix = reason instanceof Error ? reason.message : String(reason);
    throw new Error(`${label} JSON is invalid: ${suffix}`);
  }
}

function jsonText(value: unknown): string {
  return value === undefined || value === null ? "" : JSON.stringify(value, null, 2);
}

function configuredModelApps(
  provider: UniversalProvider,
  key: "modelCatalog" | "modelMapping",
): string {
  const configured = universalApps.filter((app) => Boolean(provider.models?.[app]?.[key]));
  return configured.length ? configured.join(", ") : "-";
}

function modelCatalogPlaceholder(app: string): string {
  return JSON.stringify(
    {
      models: [
        {
          id: app === "Gemini" ? "gemini-2.5-pro" : app === "Codex" ? "gpt-4o" : "claude-sonnet-4-20250514",
          upstreamModel: app === "Gemini" ? "gemini-2.5-pro" : app === "Codex" ? "gpt-4o" : "claude-sonnet-4-20250514",
          displayName: `${app} primary`,
        },
      ],
    },
    null,
    2,
  );
}

function modelMappingPlaceholder(app: string): string {
  return JSON.stringify(
    {
      rules: [
        {
          match: app === "Gemini" ? "gemini-pro" : app === "Codex" ? "gpt-4o" : "claude-sonnet",
          upstreamModel: app === "Gemini" ? "gemini-2.5-pro" : app === "Codex" ? "gpt-4o" : "claude-sonnet-4-20250514",
        },
      ],
    },
    null,
    2,
  );
}

function redactUniversalProvider(provider: UniversalProvider): UniversalProvider {
  return {
    ...provider,
    apiKey: provider.apiKey ? "<configured>" : "",
  };
}

function syncSummary(result: UniversalProviderSyncResult, tx: (text: string, variables?: Record<string, string | number>) => string): string {
  return tx("synced {{synced}}; skipped {{skipped}}; removed {{removed}}", {
    synced: result.synced.join(", ") || "-",
    skipped: result.skipped.join(", ") || "-",
    removed: result.removed.join(", ") || "-",
  });
}

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}
