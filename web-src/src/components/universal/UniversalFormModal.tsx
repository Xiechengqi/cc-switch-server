import { Boxes, Loader2, X } from "lucide-react";
import { FormEvent, ReactNode } from "react";

import { ColorPicker } from "@/components/ColorPicker";
import { IconPicker } from "@/components/IconPicker";
import JsonEditor from "@/components/JsonEditor";
import { ProviderIcon } from "@/components/ProviderIcon";
import { TextField } from "@/components/TextField";
import { inferIconForText } from "@/config/iconInference";
import type { AppKind, UniversalProvider, UniversalProviderPreset, UniversalProviderSyncResult } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { appIcon } from "@/lib/provider-icons";

export interface UniversalDraft {
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

export function UniversalFormModal({
  draft,
  saving,
  savingMode,
  onSubmitMode,
  onChange,
  onClose,
  onSubmit,
}: {
  draft: UniversalDraft;
  saving: boolean;
  savingMode: "save" | "save-sync" | null;
  onSubmitMode: (mode: "save" | "save-sync") => void;
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
            <IconPicker
              label={tx("Icon")}
              value={draft.icon}
              fallbackIcon={inferredPreviewIcon.icon}
              fallbackColor={previewIcon.color}
              providerName={draft.name || draft.providerType || "Universal"}
              onChange={(value) => patch({ icon: value })}
            />
            <ColorPicker
              label={tx("Color")}
              value={draft.iconColor}
              fallback={colorInputValue(previewIcon.color)}
              onChange={(value) => patch({ iconColor: value })}
            />
          </div>
          <div className="wide-field universal-app-config-grid">
            <UniversalAppConfigCard
              app="claude"
              label="Claude"
              enabled={draft.claude}
              onEnabled={(enabled) => patch({ claude: enabled })}
            >
              <TextField label="Claude model" value={draft.claudeModel} onChange={(value) => patch({ claudeModel: value })} />
              <TextField label="Claude haiku" value={draft.claudeHaikuModel} onChange={(value) => patch({ claudeHaikuModel: value })} />
              <TextField label="Claude sonnet" value={draft.claudeSonnetModel} onChange={(value) => patch({ claudeSonnetModel: value })} />
              <TextField label="Claude opus" value={draft.claudeOpusModel} onChange={(value) => patch({ claudeOpusModel: value })} />
            </UniversalAppConfigCard>
            <UniversalAppConfigCard
              app="codex"
              label="Codex"
              enabled={draft.codex}
              onEnabled={(enabled) => patch({ codex: enabled })}
            >
              <TextField label="Codex model" value={draft.codexModel} onChange={(value) => patch({ codexModel: value })} />
              <TextField label="Codex reasoning" value={draft.codexReasoningEffort} onChange={(value) => patch({ codexReasoningEffort: value })} />
            </UniversalAppConfigCard>
            <UniversalAppConfigCard
              app="gemini"
              label="Gemini"
              enabled={draft.gemini}
              onEnabled={(enabled) => patch({ gemini: enabled })}
            >
              <TextField label="Gemini model" value={draft.geminiModel} onChange={(value) => patch({ geminiModel: value })} />
            </UniversalAppConfigCard>
          </div>
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
          <button
            className="secondary-button"
            type="submit"
            disabled={saving}
            onClick={() => onSubmitMode("save-sync")}
          >
            {savingMode === "save-sync" && <Loader2 size={15} />}
            <span>{tx("Save and Sync")}</span>
          </button>
          <button
            className="primary-button"
            type="submit"
            disabled={saving}
            onClick={() => onSubmitMode("save")}
          >
            {savingMode === "save" && <Loader2 size={15} />}
            <span>{tx("Save Universal")}</span>
          </button>
        </footer>
      </form>
    </div>
  );
}

function UniversalAppConfigCard({
  app,
  label,
  enabled,
  onEnabled,
  children,
}: {
  app: AppKind;
  label: string;
  enabled: boolean;
  onEnabled: (enabled: boolean) => void;
  children: ReactNode;
}) {
  const { tx } = useI18n();
  const icon = appIcon(app);
  return (
    <section className={enabled ? "universal-app-config active" : "universal-app-config"}>
      <header>
        <div className="universal-app-config-title">
          <span className="provider-icon-frame small">
            <ProviderIcon icon={icon.icon} color={icon.color} name={label} size={18} />
          </span>
          <div>
            <h3>{tx(label)}</h3>
            <p>{tx("Derived provider settings")}</p>
          </div>
        </div>
        <label className="toggle-row compact-toggle">
          <input type="checkbox" checked={enabled} onChange={(event) => onEnabled(event.target.checked)} />
          <span>{tx(enabled ? "enabled" : "disabled")}</span>
        </label>
      </header>
      {enabled ? (
        <div className="universal-app-config-fields">{children}</div>
      ) : (
        <div className="compact-empty">
          <span>{tx("This app will not receive a derived provider.")}</span>
        </div>
      )}
    </section>
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
      <div className="json-editor-field">
        <span>{tx("modelCatalog JSON")}</span>
        <JsonEditor
          value={catalog}
          onChange={onCatalog}
          placeholder={modelCatalogPlaceholder(app)}
          rows={6}
        />
      </div>
      <div className="json-editor-field">
        <span>{tx("modelMapping JSON")}</span>
        <JsonEditor
          value={mapping}
          onChange={onMapping}
          placeholder={modelMappingPlaceholder(app)}
          rows={6}
        />
      </div>
    </section>
  );
}

export function enabledUniversalApps(provider: UniversalProvider): string[] {
  return [
    provider.apps.claude ? "Claude" : null,
    provider.apps.codex ? "Codex" : null,
    provider.apps.gemini ? "Gemini" : null,
  ].filter((app): app is string => Boolean(app));
}

export function emptyDraft(): UniversalDraft {
  return {
    mode: "create",
    id: "",
    providerType: "openai-compatible",
    name: "",
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

export function draftFromProvider(provider: UniversalProvider): UniversalDraft {
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

export function draftFromPreset(preset: UniversalProviderPreset): UniversalDraft {
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

export function providerFromDraft(draft: UniversalDraft): UniversalProvider {
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

export function syncSummary(
  result: UniversalProviderSyncResult,
  tx: (text: string, variables?: Record<string, string | number>) => string,
): string {
  return tx("synced {{synced}}; skipped {{skipped}}; removed {{removed}}", {
    synced: result.synced.join(", ") || "-",
    skipped: result.skipped.join(", ") || "-",
    removed: result.removed.join(", ") || "-",
  });
}

export function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}

function colorInputValue(value?: string): string {
  return value && /^#[0-9a-f]{6}$/i.test(value) ? value : "#111827";
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
