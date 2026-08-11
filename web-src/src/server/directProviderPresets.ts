import type { ProviderPreset } from "@/config/claudeProviderPresets";
import type { CodexProviderPreset } from "@/config/codexProviderPresets";
import type { GeminiProviderPreset } from "@/config/geminiProviderPresets";

const KIMI_CODE_BASE_URL = "https://api.kimi.com/coding/v1";
const KIMI_CODE_MODEL = "kimi-for-coding";

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
  name: "Kimi Code",
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
  name: "Kimi Code",
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
  name: "Kimi Code",
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
