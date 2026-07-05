import type {
  AccountManagerCapability,
  AccountRecord,
  AppKind,
  Provider,
  ProviderMatrixEntry,
  StoredProvider,
} from "@/lib/api";
import {
  apiFormatFromProvider,
  apiKeyFromProvider,
  asRecord,
  baseUrlFromProvider,
  getString,
  modelFromProvider,
} from "@/components/providers/providerDisplay";

export interface ProviderDraft {
  mode: "create" | "edit";
  app: AppKind;
  id: string;
  name: string;
  providerTypeId: string;
  baseUrl: string;
  apiKey: string;
  model: string;
  apiFormat: string;
  accountId: string;
  category: string;
  icon: string;
  iconColor: string;
  isFullUrl: boolean;
  endpointAutoSelect: boolean;
  customUserAgent: string;
  localProxyHeadersJson: string;
  localProxyBodyJson: string;
  codexSupportsThinking: boolean;
  codexSupportsEffort: boolean;
  codexThinkingParam: string;
  codexEffortParam: string;
  codexEffortValueMode: string;
  codexOutputFormat: string;
  modelCatalogJson: string;
  modelMappingJson: string;
  pricingJson: string;
  advancedJson: string;
}

export function filterProviderList(
  providers: StoredProvider[],
  query: string,
  accountsById: Map<string, AccountRecord>,
): StoredProvider[] {
  const normalizedQuery = query.trim().toLowerCase();
  if (!normalizedQuery) return providers;
  return providers.filter((provider) => {
    const accountId = provider.provider.meta?.authBinding?.accountId || "";
    const account = accountId ? accountsById.get(accountId) : undefined;
    return [
      provider.provider.id,
      provider.provider.name,
      provider.providerTypeId,
      modelFromProvider(provider.provider),
      baseUrlFromProvider(provider.provider, provider.app),
      apiFormatFromProvider(provider.provider),
      accountId,
      account?.email,
      account?.subscriptionLevel,
      provider.provider.category,
    ]
      .filter(Boolean)
      .join(" ")
      .toLowerCase()
      .includes(normalizedQuery);
  });
}

export function createDraft(app: AppKind, entry: ProviderMatrixEntry): ProviderDraft {
  const provider: Provider = {
    id: "",
    name: entry.label,
    settingsConfig: {},
    category: "",
    meta: {
      providerType: entry.providerTypeId,
      apiFormat: entry.defaults.apiFormat,
    },
  };
  return {
    mode: "create",
    app,
    id: "",
    name: entry.label,
    providerTypeId: entry.providerTypeId,
    baseUrl: entry.defaults.baseUrl,
    apiKey: "",
    model: entry.defaults.model,
    apiFormat: entry.defaults.apiFormat,
    accountId: "",
    category: "",
    icon: "",
    iconColor: "",
    isFullUrl: false,
    endpointAutoSelect: false,
    customUserAgent: "",
    localProxyHeadersJson: "",
    localProxyBodyJson: "",
    codexSupportsThinking: false,
    codexSupportsEffort: false,
    codexThinkingParam: "thinking",
    codexEffortParam: "reasoning_effort",
    codexEffortValueMode: "passthrough",
    codexOutputFormat: "auto",
    modelCatalogJson: "",
    modelMappingJson: "",
    pricingJson: "",
    advancedJson: JSON.stringify(provider, null, 2),
  };
}

export function editDraft(stored: StoredProvider, entry: ProviderMatrixEntry): ProviderDraft {
  const provider = stored.provider;
  const meta = asRecord(provider.meta);
  const requestOverrides = asRecord(meta.localProxyRequestOverrides);
  const codexReasoning = asRecord(meta.codexChatReasoning);
  return {
    mode: "edit",
    app: stored.app,
    id: provider.id,
    name: provider.name,
    providerTypeId: stored.providerTypeId,
    baseUrl: baseUrlFromProvider(provider, stored.app) || entry.defaults.baseUrl,
    apiKey: apiKeyFromProvider(provider, entry) || "",
    model: modelFromProvider(provider) || entry.defaults.model,
    apiFormat: apiFormatFromProvider(provider) || entry.defaults.apiFormat,
    accountId: provider.meta?.authBinding?.accountId || "",
    category: provider.category || "",
    icon: getString(provider.icon) || "",
    iconColor: getString(provider.iconColor) || "",
    isFullUrl: boolValue(meta.isFullUrl),
    endpointAutoSelect: boolValue(meta.endpointAutoSelect),
    customUserAgent: getString(meta.customUserAgent) || "",
    localProxyHeadersJson: jsonText(requestOverrides.headers),
    localProxyBodyJson: jsonText(requestOverrides.body),
    codexSupportsThinking: boolValue(codexReasoning.supportsThinking),
    codexSupportsEffort: boolValue(codexReasoning.supportsEffort),
    codexThinkingParam: getString(codexReasoning.thinkingParam) || "thinking",
    codexEffortParam: getString(codexReasoning.effortParam) || "reasoning_effort",
    codexEffortValueMode: getString(codexReasoning.effortValueMode) || "passthrough",
    codexOutputFormat: getString(codexReasoning.outputFormat) || "auto",
    modelCatalogJson: providerSettingJson(provider, ["modelCatalog"]),
    modelMappingJson: providerSettingJson(provider, ["modelMapping"]),
    pricingJson: providerSettingJson(provider, ["pricing", "modelPricing"]),
    advancedJson: JSON.stringify(provider, null, 2),
  };
}

export function providerFromDraft(draft: ProviderDraft, entry: ProviderMatrixEntry): Provider {
  const parsed = parseProviderJson(draft.advancedJson);
  const settings = asRecord(parsed.settingsConfig);
  const env = { ...asRecord(settings.env) };
  const baseKey = baseUrlKeyFor(draft.app, entry);
  if (draft.baseUrl.trim()) env[baseKey] = draft.baseUrl.trim();
  const key = entry.defaults.key || "API_KEY";
  if (draft.apiKey.trim()) env[key] = draft.apiKey.trim();
  if (draft.model.trim()) settings.model = draft.model.trim();
  if (draft.apiFormat.trim()) settings.apiFormat = draft.apiFormat.trim();
  if (draft.providerTypeId === "claude_auth") {
    settings.auth_mode = "bearer_only";
    env.AUTH_MODE = "bearer_only";
  }
  settings.env = env;
  setOptionalJsonSetting(settings, "modelCatalog", draft.modelCatalogJson, "modelCatalog");
  setOptionalJsonSetting(settings, "modelMapping", draft.modelMappingJson, "modelMapping");
  setPricingJsonSetting(settings, draft.pricingJson);

  const meta = { ...asRecord(parsed.meta) };
  meta.providerType = draft.providerTypeId;
  if (draft.apiFormat.trim()) meta.apiFormat = draft.apiFormat.trim();
  setOptionalBooleanMeta(meta, "isFullUrl", draft.isFullUrl);
  setOptionalBooleanMeta(meta, "endpointAutoSelect", draft.endpointAutoSelect);
  setOptionalStringMeta(meta, "customUserAgent", draft.customUserAgent);
  setOptionalRequestOverrides(meta, draft.localProxyHeadersJson, draft.localProxyBodyJson);
  setOptionalCodexReasoning(meta, draft);
  if (draft.accountId.trim()) {
    meta.authBinding = {
      source: "managed_account",
      authProvider: authProviderForType(draft.providerTypeId),
      accountId: draft.accountId.trim(),
    };
  } else {
    delete meta.authBinding;
  }

  return {
    ...parsed,
    id: draft.id || parsed.id || "",
    name: draft.name.trim(),
    category: draft.category.trim() || null,
    icon: draft.icon.trim() || undefined,
    iconColor: draft.iconColor.trim() || undefined,
    settingsConfig: settings,
    meta,
  };
}

export function duplicateStoredProvider(provider: StoredProvider, existing: StoredProvider[]): Provider {
  const source = provider.provider;
  const nextId = uniqueProviderId(source.id || source.name || provider.providerTypeId, existing);
  const maxSortIndex = existing.reduce(
    (max, item, index) => Math.max(max, numberValue(item.provider.sortIndex) ?? index),
    -1,
  );
  return {
    ...source,
    id: nextId,
    name: `${source.name || provider.providerTypeId} copy`,
    sortIndex: maxSortIndex + 1,
  };
}

export function colorInputValue(value?: string): string {
  return value && /^#[0-9a-f]{6}$/i.test(value) ? value : "#111827";
}

export function providerSettingJson(provider: Provider, keys: string[]): string {
  const settings = asRecord(provider.settingsConfig);
  for (const key of keys) {
    if (settings[key] !== undefined && settings[key] !== null) {
      return jsonText(settings[key]);
    }
  }
  return "";
}

export function accountMatchesProviderType(account: AccountRecord, providerTypeId: string): boolean {
  return account.providerType === providerTypeId || account.providerType === accountProviderTypeFor(providerTypeId);
}

export function providerKey(app: AppKind, providerId: string): string {
  return `${app}:${providerId}`;
}

export function capabilityForProvider(
  provider: StoredProvider,
  capabilitiesByType: Map<string, AccountManagerCapability>,
): AccountManagerCapability | undefined {
  return (
    capabilitiesByType.get(provider.providerTypeId) ||
    capabilitiesByType.get(accountProviderTypeFor(provider.providerTypeId))
  );
}

export function accountForProvider(
  provider: StoredProvider,
  accountsById: Map<string, AccountRecord>,
): AccountRecord | undefined {
  const accountId = provider.provider.meta?.authBinding?.accountId;
  if (accountId) return accountsById.get(accountId);
  return undefined;
}

export function providerModelCatalogPlaceholder(app: AppKind): string {
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

export function providerModelMappingPlaceholder(app: AppKind): string {
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

export function providerPricingPlaceholder(): string {
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

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function uniqueProviderId(seed: string, existing: StoredProvider[]): string {
  const existingIds = new Set(existing.map((item) => item.provider.id));
  const base = `${seed}-copy`
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 48) || "provider-copy";
  if (!existingIds.has(base)) return base;
  for (let index = 2; index < 1000; index += 1) {
    const candidate = `${base}-${index}`;
    if (!existingIds.has(candidate)) return candidate;
  }
  return `${base}-${Date.now()}`;
}

function numberValue(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function parseProviderJson(value: string): Provider {
  const parsed = JSON.parse(value || "{}") as Provider;
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error("advanced provider JSON must be an object");
  }
  return parsed;
}

function setOptionalJsonSetting(
  settings: Record<string, unknown>,
  key: string,
  raw: string,
  label: string,
) {
  const parsed = optionalJson(raw, label);
  if (parsed === undefined) {
    delete settings[key];
  } else {
    settings[key] = parsed;
  }
}

function setPricingJsonSetting(settings: Record<string, unknown>, raw: string) {
  const parsed = optionalJson(raw, "pricing");
  if (parsed === undefined) {
    delete settings.pricing;
    delete settings.modelPricing;
  } else {
    settings.pricing = parsed;
    delete settings.modelPricing;
  }
}

function setOptionalBooleanMeta(meta: Record<string, unknown>, key: string, value: boolean) {
  if (value) {
    meta[key] = true;
  } else {
    delete meta[key];
  }
}

function setOptionalStringMeta(meta: Record<string, unknown>, key: string, value: string) {
  const trimmed = value.trim();
  if (trimmed) {
    if (/[\u0000-\u001f\u007f]/.test(trimmed)) {
      throw new Error(`${key} must not contain control characters`);
    }
    meta[key] = trimmed;
  } else {
    delete meta[key];
  }
}

function setOptionalRequestOverrides(
  meta: Record<string, unknown>,
  headersRaw: string,
  bodyRaw: string,
) {
  const headers = optionalJsonObject(headersRaw, "headers overrides");
  const body = optionalJsonObject(bodyRaw, "body overrides");
  const overrides = asRecord(meta.localProxyRequestOverrides);
  if (headers !== undefined) {
    overrides.headers = headers;
  } else {
    delete overrides.headers;
  }
  if (body !== undefined) {
    overrides.body = body;
  } else {
    delete overrides.body;
  }
  if (Object.keys(overrides).length) {
    meta.localProxyRequestOverrides = overrides;
  } else {
    delete meta.localProxyRequestOverrides;
  }
}

function optionalJsonObject(raw: string, label: string): Record<string, unknown> | undefined {
  const parsed = optionalJson(raw, label);
  if (parsed === undefined) return undefined;
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error(`${label} JSON must be an object`);
  }
  return parsed as Record<string, unknown>;
}

function setOptionalCodexReasoning(meta: Record<string, unknown>, draft: ProviderDraft) {
  if (draft.app !== "codex" || draft.apiFormat !== "openai_chat") {
    delete meta.codexChatReasoning;
    return;
  }
  const reasoning: Record<string, unknown> = {};
  if (draft.codexSupportsThinking || draft.codexSupportsEffort) {
    reasoning.supportsThinking = draft.codexSupportsThinking || draft.codexSupportsEffort;
    reasoning.supportsEffort = draft.codexSupportsEffort;
    reasoning.thinkingParam = draft.codexThinkingParam || "thinking";
    reasoning.effortParam = draft.codexSupportsEffort
      ? draft.codexEffortParam || "reasoning_effort"
      : "none";
    reasoning.effortValueMode = draft.codexEffortValueMode || "passthrough";
    reasoning.outputFormat = draft.codexOutputFormat || "auto";
  }
  if (Object.keys(reasoning).length) {
    meta.codexChatReasoning = reasoning;
  } else {
    delete meta.codexChatReasoning;
  }
}

function optionalJson(raw: string, label: string): unknown {
  const trimmed = raw.trim();
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

function boolValue(value: unknown): boolean {
  return value === true;
}

function baseUrlKeyFor(app: AppKind, entry: ProviderMatrixEntry): string {
  const fromTemplate = entry.templateEnv.find((key) => key.includes("BASE_URL"));
  if (fromTemplate) return fromTemplate;
  if (app === "claude") return "ANTHROPIC_BASE_URL";
  if (app === "codex") return "OPENAI_BASE_URL";
  return "GOOGLE_GEMINI_BASE_URL";
}

function authProviderForType(providerTypeId: string): string {
  if (providerTypeId === "claude_auth") return "claude_oauth";
  if (providerTypeId === "gemini_cli") return "google_gemini_oauth";
  return providerTypeId;
}

function accountProviderTypeFor(providerTypeId: string): string {
  if (providerTypeId === "claude_auth") return "claude_oauth";
  if (providerTypeId === "gemini_cli") return "gemini_cli";
  return providerTypeId;
}
