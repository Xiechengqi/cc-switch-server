import type {
  OpenCodeModel,
  OpenCodeProviderConfig,
  ProviderMeta,
  ProviderTestConfig,
} from "@/types";
import type { PricingModelSourceOption } from "../ProviderAdvancedConfig";

// ── Default configs ──────────────────────────────────────────────────

export const CLAUDE_DEFAULT_CONFIG = JSON.stringify({ env: {} }, null, 2);
export const CLAUDE_DESKTOP_DEFAULT_CONFIG = JSON.stringify(
  {
    env: {
      ANTHROPIC_BASE_URL: "",
      ANTHROPIC_AUTH_TOKEN: "",
    },
  },
  null,
  2,
);
export const CODEX_DEFAULT_CONFIG = JSON.stringify(
  { auth: {}, config: "" },
  null,
  2,
);
export const GEMINI_DEFAULT_CONFIG = JSON.stringify(
  {
    env: {
      GOOGLE_GEMINI_BASE_URL: "",
      GEMINI_API_KEY: "",
      GEMINI_MODEL: "gemini-3.5-flash",
    },
  },
  null,
  2,
);

export const OPENCODE_DEFAULT_NPM = "@ai-sdk/openai-compatible";
export const OPENCODE_DEFAULT_CONFIG = JSON.stringify(
  {
    npm: OPENCODE_DEFAULT_NPM,
    options: {
      baseURL: "",
      apiKey: "",
      setCacheKey: true,
    },
    models: {},
  },
  null,
  2,
);
export const OPENCODE_KNOWN_OPTION_KEYS = [
  "baseURL",
  "apiKey",
  "headers",
] as const;

export const OPENCLAW_DEFAULT_CONFIG = JSON.stringify(
  {
    baseUrl: "",
    apiKey: "",
    api: "openai-completions",
    models: [],
  },
  null,
  2,
);

// ── Pure functions ───────────────────────────────────────────────────

export function isKnownOpencodeOptionKey(key: string): boolean {
  return OPENCODE_KNOWN_OPTION_KEYS.includes(
    key as (typeof OPENCODE_KNOWN_OPTION_KEYS)[number],
  );
}

export function parseOpencodeConfig(
  settingsConfig?: Record<string, unknown>,
): OpenCodeProviderConfig {
  const normalize = (
    parsed: Partial<OpenCodeProviderConfig>,
  ): OpenCodeProviderConfig => ({
    npm: parsed.npm || OPENCODE_DEFAULT_NPM,
    options:
      parsed.options && typeof parsed.options === "object"
        ? (parsed.options as OpenCodeProviderConfig["options"])
        : {},
    models:
      parsed.models && typeof parsed.models === "object"
        ? (parsed.models as Record<string, OpenCodeModel>)
        : {},
  });

  try {
    const parsed = JSON.parse(
      settingsConfig ? JSON.stringify(settingsConfig) : OPENCODE_DEFAULT_CONFIG,
    ) as Partial<OpenCodeProviderConfig>;
    return normalize(parsed);
  } catch {
    return {
      npm: OPENCODE_DEFAULT_NPM,
      options: {},
      models: {},
    };
  }
}

export function parseOpencodeConfigStrict(
  settingsConfig?: Record<string, unknown>,
): OpenCodeProviderConfig {
  const parsed = JSON.parse(
    settingsConfig ? JSON.stringify(settingsConfig) : OPENCODE_DEFAULT_CONFIG,
  ) as Partial<OpenCodeProviderConfig>;
  return {
    npm: parsed.npm || OPENCODE_DEFAULT_NPM,
    options:
      parsed.options && typeof parsed.options === "object"
        ? (parsed.options as OpenCodeProviderConfig["options"])
        : {},
    models:
      parsed.models && typeof parsed.models === "object"
        ? (parsed.models as Record<string, OpenCodeModel>)
        : {},
  };
}

export const OPENCODE_KNOWN_MODEL_KEYS = ["name", "limit", "options"] as const;

export function isKnownModelKey(key: string): boolean {
  return OPENCODE_KNOWN_MODEL_KEYS.includes(
    key as (typeof OPENCODE_KNOWN_MODEL_KEYS)[number],
  );
}

export function getModelExtraFields(
  model: OpenCodeModel,
): Record<string, string> {
  const extra: Record<string, string> = {};
  for (const [k, v] of Object.entries(model)) {
    if (!isKnownModelKey(k)) {
      extra[k] = typeof v === "string" ? v : JSON.stringify(v);
    }
  }
  return extra;
}

export function toOpencodeExtraOptions(
  options: OpenCodeProviderConfig["options"],
): Record<string, string> {
  const extra: Record<string, string> = {};
  for (const [k, v] of Object.entries(options || {})) {
    if (!isKnownOpencodeOptionKey(k)) {
      extra[k] = typeof v === "string" ? v : JSON.stringify(v);
    }
  }
  return extra;
}

export { buildOmoProfilePreview } from "@/types/omo";

export const normalizePricingSource = (
  value?: string | null,
): PricingModelSourceOption =>
  value === "request" || value === "response" ? value : "inherit";

/** Normalize persisted test config: only explicit `enabled: true` turns the switch on. */
export function normalizeProviderTestConfig(
  config?: ProviderTestConfig | null,
): ProviderTestConfig {
  if (!config) return { enabled: false };
  return { ...config, enabled: config.enabled === true };
}

/** Preset test config fields without auto-enabling the separate-config switch. */
export function presetProviderTestConfig(
  config?: ProviderTestConfig | null,
): ProviderTestConfig {
  if (!config) return { enabled: false };
  const { enabled: _ignored, ...fields } = config;
  return { ...fields, enabled: false };
}

/** True when meta carries an explicit pricing override (not absent/null/inherit). */
export function hasPricingConfigOverride(meta?: ProviderMeta | null): boolean {
  if (!meta) return false;
  const costMultiplier = meta.costMultiplier;
  if (costMultiplier != null && costMultiplier !== "") {
    return true;
  }
  const pricingModelSource = meta.pricingModelSource;
  if (
    pricingModelSource != null &&
    pricingModelSource !== "" &&
    pricingModelSource !== "inherit"
  ) {
    return true;
  }
  const quotaDispatchLimitPercent = meta.quotaDispatchLimitPercent;
  return quotaDispatchLimitPercent != null && quotaDispatchLimitPercent >= 1;
}
