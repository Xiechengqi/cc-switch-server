import type { ProviderCategory } from "@/types";

/**
 * Gemini 预设供应商的视觉主题配置
 */
export interface GeminiPresetTheme {
  /** 图标类型：'gemini' | 'generic' */
  icon?: "gemini" | "generic";
  /** 背景色（选中状态），支持 hex 颜色 */
  backgroundColor?: string;
  /** 文字色（选中状态），支持 hex 颜色 */
  textColor?: string;
}

export interface GeminiProviderPreset {
  name: string;
  nameKey?: string; // i18n key for localized display name
  websiteUrl: string;
  apiKeyUrl?: string;
  settingsConfig: object;
  baseURL?: string;
  model?: string;
  description?: string;
  category?: ProviderCategory;
  isPartner?: boolean;
  primePartner?: boolean; // 置顶合作伙伴（顶级）：徽章显示为心形
  partnerPromotionKey?: string;
  endpointCandidates?: string[];
  theme?: GeminiPresetTheme;
  providerType?: "google_gemini_oauth" | "antigravity_oauth" | "agy_oauth";
  requiresOAuth?: boolean;
  // 图标配置
  icon?: string; // 图标名称
  iconColor?: string; // 图标颜色
}

export const geminiProviderPresets: GeminiProviderPreset[] = [
  {
    name: "Google Official",
    websiteUrl: "https://ai.google.dev/",
    apiKeyUrl: "https://aistudio.google.com/apikey",
    settingsConfig: {
      env: {},
    },
    description: "Google 官方 Gemini API (OAuth)",
    category: "official",
    providerType: "google_gemini_oauth",
    requiresOAuth: true,
    partnerPromotionKey: "google-official",
    theme: {
      icon: "gemini",
      backgroundColor: "#4285F4",
      textColor: "#FFFFFF",
    },
    icon: "gemini",
    iconColor: "#4285F4",
  },
  {
    name: "Antigravity OAuth",
    websiteUrl: "https://antigravity.google",
    settingsConfig: {
      env: {
        GOOGLE_GEMINI_BASE_URL: "https://daily-cloudcode-pa.googleapis.com",
        GEMINI_MODEL: "gemini-3.5-flash-medium",
      },
      config: {
        general: {
          previewFeatures: true,
        },
      },
    },
    baseURL: "https://daily-cloudcode-pa.googleapis.com",
    model: "gemini-3.5-flash-medium",
    description: "Antigravity OAuth",
    category: "official",
    providerType: "antigravity_oauth",
    requiresOAuth: true,
    partnerPromotionKey: "antigravity-oauth",
    theme: {
      icon: "gemini",
      backgroundColor: "#1A73E8",
      textColor: "#FFFFFF",
    },
    icon: "gemini",
    iconColor: "#1A73E8",
  },
  {
    name: "Antigravity CLI (agy)",
    websiteUrl: "https://antigravity.google",
    settingsConfig: {
      env: {
        GOOGLE_GEMINI_BASE_URL: "https://daily-cloudcode-pa.googleapis.com",
        GEMINI_MODEL: "gemini-3.5-flash-medium",
      },
      config: {
        general: {
          previewFeatures: true,
        },
      },
    },
    baseURL: "https://daily-cloudcode-pa.googleapis.com",
    model: "gemini-3.5-flash-medium",
    description: "Antigravity CLI (agy)",
    category: "official",
    providerType: "agy_oauth",
    requiresOAuth: true,
    partnerPromotionKey: "antigravity-oauth",
    theme: {
      icon: "gemini",
      backgroundColor: "#111827",
      textColor: "#FFFFFF",
    },
    icon: "gemini",
    iconColor: "#111827",
  },
  {
    name: "OpenRouter",
    websiteUrl: "https://openrouter.ai",
    apiKeyUrl: "https://openrouter.ai/keys",
    settingsConfig: {
      env: {
        GOOGLE_GEMINI_BASE_URL: "https://openrouter.ai/api",
        GEMINI_MODEL: "gemini-3.5-flash",
      },
    },
    baseURL: "https://openrouter.ai/api",
    model: "gemini-3.5-flash",
    description: "OpenRouter",
    category: "aggregator",
    icon: "openrouter",
    iconColor: "#6566F1",
  },
];

export function getGeminiPresetByName(
  name: string,
): GeminiProviderPreset | undefined {
  return geminiProviderPresets.find((preset) => preset.name === name);
}

export function getGeminiPresetByUrl(
  url: string,
): GeminiProviderPreset | undefined {
  if (!url) return undefined;
  return geminiProviderPresets.find(
    (preset) =>
      preset.baseURL &&
      url.toLowerCase().includes(preset.baseURL.toLowerCase()),
  );
}
