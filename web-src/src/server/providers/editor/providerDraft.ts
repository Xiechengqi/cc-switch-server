import {
  providerPresets,
  type ProviderPreset,
} from "@/config/claudeProviderPresets";
import {
  codexProviderPresets,
  type CodexProviderPreset,
} from "@/config/codexProviderPresets";
import {
  geminiProviderPresets,
  type GeminiProviderPreset,
} from "@/config/geminiProviderPresets";
import type { ProviderCategory, ProviderMeta } from "@/types";
import {
  anthropicApiKeyPreset,
  googleGeminiApiKeyPreset,
  openAiApiKeyPreset,
} from "@/server/directProviderPresets";
import {
  driverForProfile,
  legacyPresetNameForProfile,
  type CoreProviderApp,
  type ProviderRegistryProfile,
  type ProviderUpstreamProtocol,
} from "@/server/providerRegistry";

export interface CoreProviderDraft {
  name: string;
  websiteUrl: string;
  notes: string;
  settingsConfig: Record<string, unknown>;
  category?: ProviderCategory;
  meta: ProviderMeta;
  icon?: string;
  iconColor?: string;
}

const DEFAULT_SINGLE_MODELS: Record<string, string> = {
  "claude.openai_oauth": "gpt-5.6-sol",
  "claude.grok_oauth": "grok-4.5",
  "codex.grok_oauth": "grok-4.5",
  "gemini.grok_oauth": "grok-4.5",
  "claude.kiro_oauth": "claude-sonnet-4-8",
  "claude.ollama_cloud": "kimi-k2.7-code",
  "codex.ollama_cloud": "kimi-k2.7-code",
  "claude.cursor_oauth": "composer-2.5",
  "claude.cursor_api_key": "composer-2.5",
  "claude.antigravity_oauth": "claude-sonnet-4-6",
  "claude.antigravity_cli": "claude-sonnet-4-6",
  "claude.github_copilot": "claude-sonnet-5",
  "claude.deepseek_account": "deepseek-v4-flash",
  "claude.deepseek_api": "deepseek-v4-flash",
  "codex.deepseek_api": "deepseek-v4-flash",
  "claude.aws_bedrock_aksk": "global.anthropic.claude-opus-4-8",
  "claude.aws_bedrock_api_key": "global.anthropic.claude-opus-4-8",
  "claude.openrouter": "anthropic/claude-sonnet-4.6",
  "claude.nvidia": "moonshotai/kimi-k2.5",
  "codex.nvidia": "moonshotai/kimi-k2.5",
  "codex.cursor_api_key": "gpt-5.5",
  "codex.cursor_oauth": "gpt-5.5",
  "codex.openrouter": "gpt-5.4",
  "gemini.antigravity_oauth": "gemini-3.5-flash-medium",
  "gemini.antigravity_cli": "gemini-3.5-flash-medium",
  "gemini.openrouter": "gemini-3.5-flash",
  "claude.custom_http": "claude-sonnet-4-6",
  "codex.custom_http": "gpt-5.4",
  "gemini.custom_http": "gemini-3.5-flash",
};

const ENDPOINT_ENV_KEYS: Record<CoreProviderApp, string> = {
  claude: "ANTHROPIC_BASE_URL",
  codex: "OPENAI_BASE_URL",
  gemini: "GOOGLE_GEMINI_BASE_URL",
};

const MODEL_ENV_KEYS: Record<CoreProviderApp, string> = {
  claude: "ANTHROPIC_MODEL",
  codex: "OPENAI_MODEL",
  gemini: "GEMINI_MODEL",
};

const DEFAULT_AWS_REGION = "us-east-1";

function isCredentialField(key: string): boolean {
  const normalized = key.replace(/[-_]/g, "").toLowerCase();
  return (
    normalized.endsWith("apikey") ||
    normalized.endsWith("authtoken") ||
    normalized.endsWith("accesstoken") ||
    normalized.endsWith("refreshtoken") ||
    normalized.endsWith("accesskey") ||
    normalized.endsWith("accesskeyid") ||
    normalized.endsWith("secretaccesskey") ||
    normalized.endsWith("sessiontoken") ||
    normalized === "authorization" ||
    normalized === "password"
  );
}

function sanitizePresetSettings(settings: Record<string, unknown>): void {
  const visit = (value: Record<string, unknown>) => {
    for (const [key, child] of Object.entries(value)) {
      if (isCredentialField(key) && typeof child === "string") {
        delete value[key];
        continue;
      }
      if (child && typeof child === "object" && !Array.isArray(child)) {
        visit(child as Record<string, unknown>);
      }
    }
  };
  visit(settings);

  const env = ensureObject(settings, "env");
  if (
    Object.values(env).some(
      (value) => typeof value === "string" && value.includes("${AWS_REGION}"),
    )
  ) {
    env.AWS_REGION = DEFAULT_AWS_REGION;
    for (const [key, value] of Object.entries(env)) {
      if (typeof value === "string") {
        env[key] = value.split("${AWS_REGION}").join(DEFAULT_AWS_REGION);
      }
    }
  }
}

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function apiFormatForProtocol(
  protocol: ProviderUpstreamProtocol | undefined,
): ProviderMeta["apiFormat"] | undefined {
  switch (protocol) {
    case "anthropic_messages":
      return "anthropic";
    case "open_ai_chat":
      return "openai_chat";
    case "open_ai_responses":
      return "openai_responses";
    case "gemini_native":
      return "gemini_native";
    default:
      return undefined;
  }
}

function presetForProfile(
  profile: ProviderRegistryProfile,
): ProviderPreset | CodexProviderPreset | GeminiProviderPreset | undefined {
  if (profile.profileId === "claude.anthropic_api_key") {
    return anthropicApiKeyPreset;
  }
  if (profile.profileId === "codex.openai_api_key") {
    return openAiApiKeyPreset;
  }
  if (profile.profileId === "gemini.google_api_key") {
    return googleGeminiApiKeyPreset;
  }
  const legacyName = legacyPresetNameForProfile(profile.app, profile.profileId);
  if (!legacyName) return undefined;
  if (profile.app === "claude") {
    return providerPresets.find((preset) => preset.name === legacyName);
  }
  if (profile.app === "codex") {
    return codexProviderPresets.find((preset) => preset.name === legacyName);
  }
  return geminiProviderPresets.find((preset) => preset.name === legacyName);
}

function settingsFromPreset(
  profile: ProviderRegistryProfile,
  preset:
    ProviderPreset | CodexProviderPreset | GeminiProviderPreset | undefined,
): Record<string, unknown> {
  if (!preset) return { env: {} };
  if (profile.app === "codex") {
    const codex = preset as CodexProviderPreset;
    return {
      auth: clone(codex.auth ?? {}),
      config: codex.config ?? "",
      ...(codex.modelCatalog?.length
        ? { modelCatalog: { models: clone(codex.modelCatalog) } }
        : {}),
      ...(codex.modelMapping
        ? { modelMapping: clone(codex.modelMapping) }
        : {}),
    };
  }
  if (profile.app === "gemini") {
    return clone(
      ((preset as GeminiProviderPreset).settingsConfig ?? {
        env: {},
      }) as Record<string, unknown>,
    );
  }
  return clone(
    ((preset as ProviderPreset).settingsConfig ?? {
      env: {},
    }) as Record<string, unknown>,
  );
}

export function createDraftForProfile(
  profile: ProviderRegistryProfile,
): CoreProviderDraft {
  const preset = presetForProfile(profile);
  const settingsConfig = settingsFromPreset(profile, preset);
  sanitizePresetSettings(settingsConfig);
  const driver = driverForProfile(profile);
  const presetRecord = (preset ?? {}) as Record<string, unknown>;
  const meta: ProviderMeta = {
    ...(profile.compatibilityProviderType
      ? { providerType: profile.compatibilityProviderType }
      : {}),
    ...(apiFormatForProtocol(driver?.upstreamProtocol)
      ? { apiFormat: apiFormatForProtocol(driver?.upstreamProtocol) }
      : {}),
    ...(typeof presetRecord.apiKeyField === "string"
      ? { apiKeyField: presetRecord.apiKeyField as ProviderMeta["apiKeyField"] }
      : {}),
  };
  if (profile.modelPolicy === "passthrough") {
    settingsConfig.modelMapping = { mode: "passthrough" };
  } else {
    const upstreamModel =
      readUpstreamModel(settingsConfig) ??
      DEFAULT_SINGLE_MODELS[profile.profileId] ??
      "";
    settingsConfig.modelMapping = { mode: "single", upstreamModel };
    setSingleModel(settingsConfig, profile.app, upstreamModel);
  }
  ensureObject(settingsConfig, "env");

  return {
    name: preset?.name ?? profile.label,
    websiteUrl: preset?.websiteUrl ?? "",
    notes:
      "description" in (preset ?? {}) &&
      typeof (preset as { description?: unknown }).description === "string"
        ? ((preset as { description: string }).description ?? "")
        : "",
    settingsConfig,
    category:
      (preset?.category as ProviderCategory | undefined) ??
      (profile.formComposition === "custom" ? "custom" : undefined),
    meta,
    icon: typeof presetRecord.icon === "string" ? presetRecord.icon : undefined,
    iconColor:
      typeof presetRecord.iconColor === "string"
        ? presetRecord.iconColor
        : undefined,
  };
}

export function ensureObject(
  parent: Record<string, unknown>,
  key: string,
): Record<string, unknown> {
  const current = parent[key];
  if (current && typeof current === "object" && !Array.isArray(current)) {
    return current as Record<string, unknown>;
  }
  const created: Record<string, unknown> = {};
  parent[key] = created;
  return created;
}

export function readEndpoint(
  settings: Record<string, unknown>,
  app: CoreProviderApp,
): string {
  const env = ensureObject(settings, "env");
  const value = env[ENDPOINT_ENV_KEYS[app]];
  if (typeof value === "string" && value.trim()) return value.trim();
  const direct = settings[ENDPOINT_ENV_KEYS[app]];
  return typeof direct === "string" ? direct.trim() : "";
}

export function setEndpoint(
  settings: Record<string, unknown>,
  app: CoreProviderApp,
  endpoint: string,
): void {
  const env = ensureObject(settings, "env");
  const key = ENDPOINT_ENV_KEYS[app];
  const value = endpoint.trim().replace(/\/+$/, "");
  if (value) env[key] = value;
  else delete env[key];
}

export function readUpstreamModel(
  settings: Record<string, unknown>,
): string | undefined {
  const mapping = settings.modelMapping;
  if (mapping && typeof mapping === "object" && !Array.isArray(mapping)) {
    const model = (mapping as Record<string, unknown>).upstreamModel;
    if (typeof model === "string" && model.trim()) return model.trim();
  }
  for (const key of ["model", "upstreamModel"]) {
    const value = settings[key];
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  const env = ensureObject(settings, "env");
  for (const key of Object.values(MODEL_ENV_KEYS)) {
    const value = env[key];
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  return undefined;
}

export function setSingleModel(
  settings: Record<string, unknown>,
  app: CoreProviderApp,
  model: string,
): void {
  const upstreamModel = model.trim();
  settings.modelMapping = { mode: "single", upstreamModel };
  const env = ensureObject(settings, "env");
  if (upstreamModel) env[MODEL_ENV_KEYS[app]] = upstreamModel;
  else delete env[MODEL_ENV_KEYS[app]];
}

export function setPassthroughModel(settings: Record<string, unknown>): void {
  settings.modelMapping = { mode: "passthrough" };
}

export function defaultSingleModel(profileId: string): string {
  return DEFAULT_SINGLE_MODELS[profileId] ?? "";
}

export function endpointEnvironmentKey(app: CoreProviderApp): string {
  return ENDPOINT_ENV_KEYS[app];
}
