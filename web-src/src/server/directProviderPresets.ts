import type { ProviderPreset } from "@/config/claudeProviderPresets";
import type { CodexProviderPreset } from "@/config/codexProviderPresets";
import type { GeminiProviderPreset } from "@/config/geminiProviderPresets";

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
