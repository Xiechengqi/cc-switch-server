import type { ProviderPreset } from "@/config/claudeProviderPresets";
import type { CodexProviderPreset } from "@/config/codexProviderPresets";
import type { GeminiProviderPreset } from "@/config/geminiProviderPresets";

type DirectPresetApp = "claude" | "codex" | "gemini";

interface DirectProfileVisual {
  name: string;
  websiteUrl: string;
  icon: string;
  iconColor?: string;
}

const KIMI_CODE_BASE_URL = "https://api.kimi.com/coding/v1";
const KIMI_CODE_MODEL = "kimi-for-coding";
const KIRO_BASE_URL = "https://q.us-east-1.amazonaws.com";
const KIRO_MODEL = "claude-sonnet-4-8";
const QODER_MODEL = "auto";

export const anthropicApiKeyPreset: ProviderPreset = {
  name: "Anthropic API Key",
  websiteUrl: "https://console.anthropic.com/",
  apiKeyUrl: "https://console.anthropic.com/settings/keys",
  settingsConfig: {
    env: {
      ANTHROPIC_BASE_URL: "https://api.anthropic.com",
    },
    modelMapping: { mode: "passthrough" },
  },
  isOfficial: true,
  category: "official",
  apiKeyField: "ANTHROPIC_API_KEY",
  apiFormat: "anthropic",
  icon: "anthropic",
  iconColor: "#D4915D",
};

export const claudeGoogleOAuthPreset: ProviderPreset = {
  name: "Google Gemini OAuth",
  websiteUrl: "https://codeassist.google/",
  settingsConfig: {
    env: {
      ANTHROPIC_BASE_URL: "https://cloudcode-pa.googleapis.com",
      ANTHROPIC_MODEL: "gemini-3.1-pro-preview",
    },
    modelMapping: {
      mode: "single",
      upstreamModel: "gemini-3.1-pro-preview",
    },
  },
  isOfficial: true,
  category: "official",
  apiFormat: "gemini_native",
  providerType: "google_gemini_oauth",
  requiresOAuth: true,
  icon: "gemini",
  iconColor: "#4285F4",
};

export const openAiApiKeyPreset: CodexProviderPreset = {
  name: "OpenAI API Key",
  websiteUrl: "https://platform.openai.com/",
  apiKeyUrl: "https://platform.openai.com/api-keys",
  auth: { OPENAI_API_KEY: "" },
  config: `model = "gpt-5.4"
model_reasoning_effort = "high"
disable_response_storage = true`,
  modelMapping: { mode: "passthrough" },
  isOfficial: true,
  category: "official",
  apiFormat: "openai_responses",
  icon: "openai",
  iconColor: "#00A67E",
};

export const githubCopilotCodexPreset: CodexProviderPreset = {
  name: "GitHub Copilot",
  websiteUrl: "https://github.com/features/copilot",
  auth: {},
  config: `model_provider = "github_copilot"
model = "gpt-5.5"
disable_response_storage = true

[model_providers.github_copilot]
name = "GitHub Copilot"
base_url = "https://api.githubcopilot.com"
wire_api = "responses"
requires_openai_auth = true`,
  modelMapping: { mode: "single", upstreamModel: "gpt-5.5" },
  category: "third_party",
  apiFormat: "openai_chat",
  providerType: "github_copilot",
  requiresOAuth: true,
  icon: "github",
  iconColor: "#000000",
};

export const kiroCodexPreset: CodexProviderPreset = {
  name: "Kiro OAuth",
  websiteUrl: "https://kiro.dev",
  auth: {},
  config: `model_provider = "kiro"
model = "${KIRO_MODEL}"
disable_response_storage = true

[model_providers.kiro]
name = "Kiro"
base_url = "${KIRO_BASE_URL}"
wire_api = "responses"
requires_openai_auth = true`,
  modelMapping: { mode: "single", upstreamModel: KIRO_MODEL },
  isOfficial: true,
  category: "official",
  apiFormat: "openai_chat",
  providerType: "kiro_oauth",
  requiresOAuth: true,
  icon: "kiro",
};

export const googleGeminiApiKeyPreset: GeminiProviderPreset = {
  name: "Google Gemini API Key",
  websiteUrl: "https://ai.google.dev/",
  apiKeyUrl: "https://aistudio.google.com/apikey",
  settingsConfig: {
    env: {},
  },
  description: "Google Gemini API Key",
  category: "official",
  icon: "gemini",
  iconColor: "#4285F4",
};

export const kimiCodeClaudePreset: ProviderPreset = {
  name: "Kimi Code OAuth",
  websiteUrl: "https://kimi.com",
  settingsConfig: {
    env: {
      ANTHROPIC_BASE_URL: KIMI_CODE_BASE_URL,
      ANTHROPIC_MODEL: KIMI_CODE_MODEL,
    },
    modelMapping: { mode: "single", upstreamModel: KIMI_CODE_MODEL },
  },
  isOfficial: true,
  category: "official",
  apiFormat: "openai_chat",
  icon: "kimi",
  iconColor: "#111827",
};

export const kimiCodeCodexPreset: CodexProviderPreset = {
  name: "Kimi Code OAuth",
  websiteUrl: "https://kimi.com",
  auth: {},
  config: `model_provider = "kimi_code"
model = "${KIMI_CODE_MODEL}"
disable_response_storage = true

[model_providers.kimi_code]
name = "Kimi Code"
base_url = "${KIMI_CODE_BASE_URL}"
wire_api = "responses"
requires_openai_auth = true`,
  modelMapping: { mode: "single", upstreamModel: KIMI_CODE_MODEL },
  isOfficial: true,
  category: "official",
  apiFormat: "openai_chat",
  icon: "kimi",
  iconColor: "#111827",
};

export const kimiCodeGeminiPreset: GeminiProviderPreset = {
  name: "Kimi Code OAuth",
  websiteUrl: "https://kimi.com",
  settingsConfig: {
    env: {
      GOOGLE_GEMINI_BASE_URL: KIMI_CODE_BASE_URL,
      GEMINI_MODEL: KIMI_CODE_MODEL,
    },
  },
  baseURL: KIMI_CODE_BASE_URL,
  model: KIMI_CODE_MODEL,
  description: "Kimi Code OAuth",
  category: "official",
  icon: "kimi",
  iconColor: "#111827",
};

export const qoderClaudePreset: ProviderPreset = {
  name: "Qoder COSY",
  websiteUrl: "https://qoder.com",
  settingsConfig: {
    env: {},
    modelMapping: { mode: "single", upstreamModel: QODER_MODEL },
  },
  isOfficial: true,
  category: "official",
  icon: "qoder",
};

export const qoderCodexPreset: CodexProviderPreset = {
  name: "Qoder COSY",
  websiteUrl: "https://qoder.com",
  auth: {},
  config: "",
  modelMapping: { mode: "single", upstreamModel: QODER_MODEL },
  isOfficial: true,
  category: "official",
  icon: "qoder",
};

export const qoderGeminiPreset: GeminiProviderPreset = {
  name: "Qoder COSY",
  websiteUrl: "https://qoder.com",
  settingsConfig: {
    env: {},
  },
  model: QODER_MODEL,
  description: "Qoder COSY",
  category: "official",
  icon: "qoder",
};

const directProfileVisuals: Record<string, DirectProfileVisual> = {
  "gemini.github_copilot": {
    name: "GitHub Copilot",
    websiteUrl: "https://github.com/features/copilot",
    icon: "github",
    iconColor: "#000000",
  },
  "gemini.cursor_api_key": {
    name: "Cursor API Key",
    websiteUrl: "https://cursor.com",
    icon: "cursor",
  },
  "gemini.cursor_oauth": {
    name: "Cursor OAuth",
    websiteUrl: "https://cursor.com",
    icon: "cursor",
  },
  kimi_coding_api_key: {
    name: "Kimi Code API Key",
    websiteUrl: "https://kimi.com",
    icon: "kimi",
    iconColor: "#6366F1",
  },
  zhipu_glm_cn: {
    name: "Zhipu (China)",
    websiteUrl: "https://bigmodel.cn",
    icon: "zhipu",
    iconColor: "#0F62FE",
  },
  zhipu_glm_global: {
    name: "Zhipu (Global)",
    websiteUrl: "https://z.ai",
    icon: "zhipu",
    iconColor: "#0F62FE",
  },
  bailian_coding_plan_cn: {
    name: "Alibaba (China)",
    websiteUrl: "https://bailian.console.aliyun.com",
    icon: "bailian",
    iconColor: "#FF6A00",
  },
  bailian_coding_plan_global: {
    name: "Alibaba (Global/Singapore)",
    websiteUrl: "https://www.alibabacloud.com/product/coding",
    icon: "bailian",
    iconColor: "#FF6A00",
  },
  minimax_cn: {
    name: "MiniMax (China)",
    websiteUrl: "https://platform.minimaxi.com",
    icon: "minimax",
    iconColor: "#FF6B6B",
  },
  minimax_global: {
    name: "MiniMax (Global)",
    websiteUrl: "https://platform.minimax.io",
    icon: "minimax",
    iconColor: "#FF6B6B",
  },
  volcengine_coding_plan: {
    name: "Volcengine",
    websiteUrl: "https://www.volcengine.com",
    icon: "doubao",
    iconColor: "#1E37FC",
  },
  xiaomi_mimo_token_plan: {
    name: "Xiaomi MiMo (China)",
    websiteUrl: "https://platform.xiaomimimo.com",
    icon: "xiaomimimo",
    iconColor: "#FF6900",
  },
  xiaomi_mimo_token_plan_sgp: {
    name: "Xiaomi MiMo (Singapore)",
    websiteUrl: "https://platform.xiaomimimo.com",
    icon: "xiaomimimo",
    iconColor: "#FF6900",
  },
};

export function directProfileVisualPreset(
  profileId: string,
  app: DirectPresetApp,
): ProviderPreset | CodexProviderPreset | GeminiProviderPreset | undefined {
  const familyId = profileId.slice(profileId.indexOf(".") + 1);
  const visual = directProfileVisuals[profileId] ?? directProfileVisuals[familyId];
  if (!visual) return undefined;

  if (app === "codex") {
    return {
      ...visual,
      auth: {},
      config: "",
      category: "official",
    };
  }
  if (app === "gemini") {
    return {
      ...visual,
      settingsConfig: { env: {} },
      category: "official",
    };
  }
  return {
    ...visual,
    settingsConfig: { env: {} },
    isOfficial: true,
    category: "official",
  };
}
