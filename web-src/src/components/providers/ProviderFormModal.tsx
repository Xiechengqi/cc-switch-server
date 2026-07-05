import {
  Boxes,
  ChevronDown,
  Download,
  Eye,
  EyeOff,
  Loader2,
  ServerCog,
  X,
} from "lucide-react";
import { FormEvent, useState } from "react";

import { AccountRecord, AppKind, fetchProviderModels, Provider, ProviderMatrixEntry } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { inferIconForText } from "@/config/iconInference";
import { ColorPicker } from "@/components/ColorPicker";
import { IconPicker } from "@/components/IconPicker";
import JsonEditor from "@/components/JsonEditor";
import { ProviderIcon } from "@/components/ProviderIcon";
import { StatusPill } from "@/components/StatusPill";
import { asRecord, getString, setting } from "@/components/providers/providerDisplay";
import type { ProviderDraft } from "@/components/providers/ProviderList";


const apps: Array<{ id: AppKind; label: string }> = [
  { id: "claude", label: "Claude" },
  { id: "codex", label: "Codex" },
  { id: "gemini", label: "Gemini" },
];

function accountProviderTypeFor(providerTypeId: string): string {
  if (providerTypeId === "claude_auth") return "claude_oauth";
  if (providerTypeId === "gemini_cli") return "gemini_cli";
  return providerTypeId;
}

function accountMatchesProviderType(account: AccountRecord, providerTypeId: string): boolean {
  return account.providerType === providerTypeId || account.providerType === accountProviderTypeFor(providerTypeId);
}

function colorInputValue(value?: string): string {
  return value && /^#[0-9a-f]{6}$/i.test(value) ? value : "#111827";
}

function providerSettingJson(provider: Provider, keys: string[]): string {
  const settings = asRecord(provider.settingsConfig);
  for (const key of keys) {
    if (settings[key] !== undefined && settings[key] !== null) {
      return JSON.stringify(settings[key], null, 2);
    }
  }
  return "";
}

function providerModelCatalogPlaceholder(app: AppKind): string {
  const model =
    app === "gemini"
      ? "gemini-2.5-pro"
      : app === "codex"
        ? "gpt-5.5"
        : "claude-sonnet-5";
  return JSON.stringify(
    {
      models: [
        {
          id: model,
          upstreamModel: model,
          displayName: model,
        },
      ],
    },
    null,
    2,
  );
}

function providerModelMappingPlaceholder(app: AppKind): string {
  const target =
    app === "gemini"
      ? "gemini-2.5-pro"
      : app === "codex"
        ? "gpt-5.5"
        : "claude-sonnet-5";
  return JSON.stringify(
    {
      rules: [
        {
          match: "*",
          upstreamModel: target,
        },
      ],
    },
    null,
    2,
  );
}

function providerPricingPlaceholder(): string {
  return JSON.stringify(
    {
      default: {
        inputUsdPerMillion: 3,
        outputUsdPerMillion: 15,
        cacheReadUsdPerMillion: 0.3,
        cacheCreationUsdPerMillion: 3.75,
      },
    },
    null,
    2,
  );
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

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

function ProviderAuthSection({
  draft,
  entry,
  accountOptions,
  onPatch,
}: {
  draft: ProviderDraft;
  entry: ProviderMatrixEntry;
  accountOptions: AccountRecord[];
  onPatch: (next: Partial<ProviderDraft>) => void;
}) {
  const { tx } = useI18n();
  const [showApiKey, setShowApiKey] = useState(false);
  const apiKeyUrl = apiKeyUrlForProvider(entry, draft);
  const apiKeyOptional = entry.credentialMode === "oauth_or_manual_token" || Boolean(draft.accountId);
  return (
    <section className="wide-field provider-auth-section">
      <div className="section-title-row compact-title">
        <ServerCog size={16} />
        <div>
          <h3>{tx("Authentication and endpoint")}</h3>
          <span>{tx("Manual token, managed account, and upstream routing settings")}</span>
        </div>
      </div>
      <div className="provider-auth-grid">
        <article className="provider-auth-card">
          <header>
            <StatusPill tone={entry.directConfigSupported ? "success" : "warning"}>
              {tx(entry.directConfigSupported ? "direct" : "limited")}
            </StatusPill>
            <span>{tx(entry.credentialMode)}</span>
          </header>
          <div className="provider-field">
            <span>{tx(entry.defaults.key || "API key")}</span>
            <div className="provider-api-key-row">
              <input
                type={showApiKey ? "text" : "password"}
                value={draft.apiKey}
                onChange={(event) => onPatch({ apiKey: event.target.value })}
                placeholder={apiKeyOptional ? tx("optional account token") : ""}
                autoComplete="off"
              />
              <button
                className="secondary-button compact icon-only"
                type="button"
                onClick={() => setShowApiKey((value) => !value)}
                title={tx(showApiKey ? "Hide API key" : "Show API key")}
                aria-label={tx(showApiKey ? "Hide API key" : "Show API key")}
              >
                {showApiKey ? <EyeOff size={14} /> : <Eye size={14} />}
              </button>
            </div>
            <div className="provider-api-key-hint">
              <span>
                {apiKeyOptional
                  ? tx("API key is optional when a managed account is selected.")
                  : tx("Enter the upstream API key for direct config.")}
              </span>
              {apiKeyUrl ? (
                <a href={apiKeyUrl} target="_blank" rel="noopener noreferrer">
                  {tx("Get API Key")}
                </a>
              ) : null}
            </div>
          </div>
        </article>
        <article className="provider-auth-card">
          <header>
            <StatusPill tone={entry.accountSupported ? "success" : "warning"}>
              {tx(entry.managedAccountRecommended ? "recommended" : "account")}
            </StatusPill>
            <span>{tx("{{count}} accounts", { count: accountOptions.length })}</span>
          </header>
          <label>
            <span>{tx("Managed account")}</span>
            <select value={draft.accountId} onChange={(event) => onPatch({ accountId: event.target.value })}>
              <option value="">{tx("Direct config")}</option>
              {accountOptions.map((account) => (
                <option key={account.id} value={account.id}>
                  {account.email || account.id}
                  {account.subscriptionLevel ? ` (${account.subscriptionLevel})` : ""}
                </option>
              ))}
            </select>
          </label>
        </article>
        <article className="provider-auth-card">
          <header>
            <StatusPill tone="success">{tx("endpoint")}</StatusPill>
            <span>{entry.defaults.apiFormat || tx("api format")}</span>
          </header>
          <label>
            <span>{tx("Base URL")}</span>
            <input
              value={draft.baseUrl}
              onChange={(event) => onPatch({ baseUrl: event.target.value })}
            />
          </label>
          <label>
            <span>{tx("API format")}</span>
            <select
              value={draft.apiFormat}
              onChange={(event) => onPatch({ apiFormat: event.target.value })}
            >
              {apiFormatOptions(draft.app, draft.apiFormat, entry.defaults.apiFormat).map((option) => (
                <option key={option} value={option}>
                  {apiFormatLabel(option)}
                </option>
              ))}
            </select>
          </label>
          <div className="provider-endpoint-toggle-row">
            <label className="checkbox-row">
              <input
                type="checkbox"
                checked={draft.isFullUrl}
                onChange={(event) => onPatch({ isFullUrl: event.target.checked })}
              />
              <span>{tx("Treat Base URL as full upstream URL")}</span>
            </label>
            <label className="checkbox-row">
              <input
                type="checkbox"
                checked={draft.endpointAutoSelect}
                onChange={(event) => onPatch({ endpointAutoSelect: event.target.checked })}
              />
              <span>{tx("Auto-select fastest endpoint")}</span>
            </label>
          </div>
        </article>
      </div>
    </section>
  );
}

function ProviderModelField({
  draft,
  options,
  busy,
  result,
  onChange,
  onFetch,
}: {
  draft: ProviderDraft;
  options: string[];
  busy: boolean;
  result: string | null;
  onChange: (model: string) => void;
  onFetch?: () => void;
}) {
  const { tx } = useI18n();
  const [open, setOpen] = useState(false);
  return (
    <div className="provider-model-field provider-field">
      <span>{tx("Model")}</span>
      <div className="provider-model-input-row">
        <input
          value={draft.model}
          onChange={(event) => onChange(event.target.value)}
          autoComplete="off"
        />
        {options.length ? (
          <div className="provider-model-dropdown">
            <button
              className={open ? "secondary-button compact icon-only active" : "secondary-button compact icon-only"}
              type="button"
              onClick={() => setOpen((value) => !value)}
              title={tx("Select catalog model")}
              aria-label={tx("Select catalog model")}
              aria-expanded={open}
            >
              <ChevronDown size={14} />
            </button>
            {open ? (
              <div className="provider-model-menu" role="listbox" aria-label={tx("Catalog models")}>
                <div className="provider-model-menu-label">{tx("Catalog models")}</div>
                {options.map((model) => (
                  <button
                    key={model}
                    className={model === draft.model ? "provider-model-option active" : "provider-model-option"}
                    type="button"
                    role="option"
                    aria-selected={model === draft.model}
                    onClick={() => {
                      onChange(model);
                      setOpen(false);
                    }}
                  >
                    <span>{model}</span>
                  </button>
                ))}
              </div>
            ) : null}
          </div>
        ) : null}
        {onFetch ? (
          <button
            className="secondary-button compact icon-only"
            type="button"
            onClick={onFetch}
            disabled={busy}
            title={tx("Fetch models")}
            aria-label={tx("Fetch models")}
          >
            {busy ? <Loader2 size={14} /> : <Download size={14} />}
          </button>
        ) : null}
      </div>
      <small>
        {result ||
          (options.length
            ? tx("{{count}} catalog models available", { count: options.length })
            : tx(onFetch ? "Fetch upstream models into model catalog" : "Saved model or provider default"))}
      </small>
    </div>
  );
}

function apiKeyUrlForProvider(entry: ProviderMatrixEntry, draft: ProviderDraft): string | null {
  if (entry.apiKeyUrl) return entry.apiKeyUrl;
  if (!["api_key", "aws_credentials"].includes(entry.credentialMode)) return null;
  const text = `${entry.providerTypeId} ${entry.label} ${draft.baseUrl}`.toLowerCase();
  if (text.includes("openrouter")) return "https://openrouter.ai/keys";
  if (text.includes("anthropic") || text.includes("claude")) return "https://console.anthropic.com/settings/keys";
  if (text.includes("openai") || text.includes("codex")) return "https://platform.openai.com/api-keys";
  if (text.includes("gemini") || text.includes("google")) return "https://aistudio.google.com/app/apikey";
  if (text.includes("deepseek")) return "https://platform.deepseek.com/api_keys";
  if (text.includes("nvidia")) return "https://build.nvidia.com/settings/api-keys";
  if (text.includes("ollama")) return "https://ollama.com/settings/keys";
  if (text.includes("cursor")) return "https://cursor.com/settings";
  if (text.includes("github") || text.includes("copilot")) return "https://github.com/settings/tokens";
  try {
    const url = new URL(draft.baseUrl);
    return `${url.origin}/`;
  } catch {
    return null;
  }
}

function apiFormatOptions(app: AppKind, current: string, fallback: string): string[] {
  const base: Record<AppKind, string[]> = {
    claude: ["anthropic", "openai_chat", "openai_responses", "gemini_native"],
    codex: ["openai_responses", "openai_chat"],
    gemini: ["gemini_native", "openai_chat"],
  };
  return uniqueStrings([...base[app], fallback, current].filter(Boolean));
}

function apiFormatLabel(value: string): string {
  const labels: Record<string, string> = {
    anthropic: "Anthropic Messages",
    openai_chat: "OpenAI Chat Completions",
    openai_responses: "OpenAI Responses",
    gemini_native: "Gemini Native",
  };
  return labels[value] || value;
}

function ProviderDesktopAdvancedSection({
  draft,
  onPatch,
}: {
  draft: ProviderDraft;
  onPatch: (next: Partial<ProviderDraft>) => void;
}) {
  const { tx } = useI18n();
  const showCodexReasoning = draft.app === "codex";
  const hasValues = Boolean(
    draft.customUserAgent.trim() ||
      draft.localProxyHeadersJson.trim() ||
      draft.localProxyBodyJson.trim() ||
      draft.codexSupportsThinking ||
      draft.codexSupportsEffort ||
      draft.codexThinkingParam !== "thinking" ||
      draft.codexEffortParam !== "reasoning_effort" ||
      draft.codexEffortValueMode !== "passthrough" ||
      draft.codexOutputFormat !== "auto",
  );
  return (
    <details className="wide-field provider-advanced-section provider-desktop-advanced" open={hasValues || undefined}>
      <summary>
        <ServerCog size={16} />
        <span>{tx("Desktop advanced options")}</span>
        <small>{tx("Full URL mode, endpoint selection, request overrides, and Codex reasoning")}</small>
      </summary>
      <div className="provider-desktop-advanced-grid">
        <article className="provider-auth-card provider-request-overrides-card">
          <header>
            <StatusPill tone={draft.customUserAgent.trim() ? "success" : "warning"}>
              {tx("request")}
            </StatusPill>
            <span>{tx("Local proxy request overrides")}</span>
          </header>
          <label>
            <span>{tx("Custom User-Agent")}</span>
            <input
              value={draft.customUserAgent}
              onChange={(event) => onPatch({ customUserAgent: event.target.value })}
              placeholder="cc-switch-server"
            />
          </label>
          <div className="provider-request-json-grid">
            <div className="json-editor-field">
              <span>{tx("Headers JSON")}</span>
              <JsonEditor
                value={draft.localProxyHeadersJson}
                onChange={(value) => onPatch({ localProxyHeadersJson: value })}
                placeholder={JSON.stringify({ "x-provider-feature": "enabled" }, null, 2)}
                rows={5}
              />
            </div>
            <div className="json-editor-field">
              <span>{tx("Body JSON")}</span>
              <JsonEditor
                value={draft.localProxyBodyJson}
                onChange={(value) => onPatch({ localProxyBodyJson: value })}
                placeholder={JSON.stringify({ extra_body: { value: true } }, null, 2)}
                rows={5}
              />
            </div>
          </div>
        </article>

        {showCodexReasoning && (
          <article className="provider-auth-card provider-codex-reasoning-card">
            <header>
              <StatusPill tone={draft.codexSupportsThinking || draft.codexSupportsEffort ? "success" : "warning"}>
                {tx("reasoning")}
              </StatusPill>
              <span>{tx("Codex Chat Completions capability")}</span>
            </header>
            <label className="checkbox-row">
              <input
                type="checkbox"
                checked={draft.codexSupportsThinking}
                onChange={(event) =>
                  onPatch({
                    codexSupportsThinking: event.target.checked,
                    codexSupportsEffort: event.target.checked ? draft.codexSupportsEffort : false,
                  })
                }
              />
              <span>{tx("Supports thinking mode")}</span>
            </label>
            <label className="checkbox-row">
              <input
                type="checkbox"
                checked={draft.codexSupportsEffort}
                onChange={(event) =>
                  onPatch({
                    codexSupportsThinking: event.target.checked ? true : draft.codexSupportsThinking,
                    codexSupportsEffort: event.target.checked,
                  })
                }
              />
              <span>{tx("Supports reasoning effort")}</span>
            </label>
            <div className="provider-reasoning-grid">
              <label>
                <span>{tx("Thinking param")}</span>
                <select
                  value={draft.codexThinkingParam}
                  onChange={(event) => onPatch({ codexThinkingParam: event.target.value })}
                >
                  {["thinking", "enable_thinking", "reasoning_split"].map((value) => (
                    <option key={value} value={value}>
                      {value}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                <span>{tx("Effort param")}</span>
                <select
                  value={draft.codexEffortParam}
                  onChange={(event) => onPatch({ codexEffortParam: event.target.value })}
                >
                  {["none", "reasoning_effort", "reasoning.effort"].map((value) => (
                    <option key={value} value={value}>
                      {value}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                <span>{tx("Effort values")}</span>
                <select
                  value={draft.codexEffortValueMode}
                  onChange={(event) => onPatch({ codexEffortValueMode: event.target.value })}
                >
                  {["passthrough", "low_high", "deepseek", "openrouter"].map((value) => (
                    <option key={value} value={value}>
                      {value}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                <span>{tx("Output format")}</span>
                <select
                  value={draft.codexOutputFormat}
                  onChange={(event) => onPatch({ codexOutputFormat: event.target.value })}
                >
                  {["auto", "reasoning_content", "reasoning", "reasoning_details", "think_tags"].map((value) => (
                    <option key={value} value={value}>
                      {value}
                    </option>
                  ))}
                </select>
              </label>
            </div>
          </article>
        )}
      </div>
    </details>
  );
}

function uniqueStrings(values: string[]): string[] {
  return Array.from(new Set(values.map((value) => value.trim()).filter(Boolean)));
}

function modelOptionsFromCatalogJson(raw: string): string[] {
  const trimmed = raw.trim();
  if (!trimmed) return [];
  try {
    const parsed = JSON.parse(trimmed) as unknown;
    const models = Array.isArray(parsed)
      ? parsed
      : asRecord(parsed).models;
    return uniqueStrings(extractModelIds(models));
  } catch {
    return [];
  }
}

function extractModelIds(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((item) => {
    if (typeof item === "string") return [item];
    const record = asRecord(item);
    return [
      getString(record.id),
      getString(record.model),
      getString(record.upstreamModel),
      getString(record.name),
    ].filter((model): model is string => Boolean(model));
  });
}

function modelCatalogJsonFromFetchedModels(
  models: Array<{ id: string; upstreamModel: string; displayName?: string | null }>,
): string {
  if (!models.length) return "";
  return JSON.stringify(
    {
      models: models.map((model) => ({
        id: model.id,
        upstreamModel: model.upstreamModel,
        ...(model.displayName ? { displayName: model.displayName } : {}),
      })),
    },
    null,
    2,
  );
}

function ProviderJsonField({
  title,
  label,
  value,
  placeholder,
  onChange,
}: {
  title: string;
  label: string;
  value: string;
  placeholder: string;
  onChange: (value: string) => void;
}) {
  const { tx } = useI18n();
  return (
    <section className="universal-json-card">
      <h4>{tx(title)}</h4>
      <div className="json-editor-field">
        <span>{tx(label)}</span>
        <JsonEditor
          value={value}
          onChange={onChange}
          placeholder={placeholder}
          rows={7}
        />
      </div>
    </section>
  );
}
