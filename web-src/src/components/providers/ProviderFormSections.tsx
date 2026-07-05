import { ChevronDown, Download, Eye, EyeOff, Loader2, ServerCog } from "lucide-react";
import { useState } from "react";

import JsonEditor from "@/components/JsonEditor";
import { StatusPill } from "@/components/StatusPill";
import type { AccountRecord, AppKind, ProviderMatrixEntry } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { asRecord, getString } from "@/components/providers/providerDisplay";
import type { ProviderDraft } from "@/components/providers/providerDraft";

export function ProviderAuthSection({
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

export function ProviderModelField({
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

export function ProviderDesktopAdvancedSection({
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

export function ProviderJsonField({
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

export function modelOptionsFromCatalogJson(raw: string): string[] {
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

export function modelCatalogJsonFromFetchedModels(
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

function uniqueStrings(values: string[]): string[] {
  return Array.from(new Set(values.map((value) => value.trim()).filter(Boolean)));
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
