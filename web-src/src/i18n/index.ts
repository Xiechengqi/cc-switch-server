import i18n from "i18next";
import { initReactI18next } from "react-i18next";

import en from "./locales/en.json";
import ja from "./locales/ja.json";
import zh from "./locales/zh.json";
import zhTW from "./locales/zh-TW.json";
import serverEn from "./server-locales/en.json";
import serverJa from "./server-locales/ja.json";
import serverZh from "./server-locales/zh.json";
import serverZhTW from "./server-locales/zh-TW.json";

type Language = "zh" | "zh-TW" | "en" | "ja";

const DEFAULT_LANGUAGE: Language = "zh";

const getInitialLanguage = (): Language => {
  if (typeof window !== "undefined") {
    try {
      const stored = window.localStorage.getItem("language");
      if (
        stored === "zh" ||
        stored === "zh-TW" ||
        stored === "en" ||
        stored === "ja"
      ) {
        return stored;
      }
    } catch (error) {
      console.warn("[i18n] Failed to read stored language preference", error);
    }
  }

  const navigatorLang =
    typeof navigator !== "undefined"
      ? (navigator.language?.toLowerCase() ??
        navigator.languages?.[0]?.toLowerCase())
      : undefined;

  if (navigatorLang === "zh") {
    return "zh";
  }

  if (
    navigatorLang?.startsWith("zh-tw") ||
    navigatorLang?.startsWith("zh-hk") ||
    navigatorLang?.startsWith("zh-mo") ||
    navigatorLang?.startsWith("zh-hant")
  ) {
    return "zh-TW";
  }

  if (navigatorLang?.startsWith("zh")) {
    return "zh";
  }

  if (navigatorLang?.startsWith("ja")) {
    return "ja";
  }

  if (navigatorLang?.startsWith("en")) {
    return "en";
  }

  return DEFAULT_LANGUAGE;
};

const ACCOUNT_TRANSLATION_NAMESPACES = [
  "antigravityOauth",
  "claudeOauth",
  "codexOauth",
  "copilot",
  "cursorOauth",
  "deepseekAccount",
  "geminiOauth",
  "grokOauth",
  "kiroOauth",
] as const;

function mergeTranslationTrees(
  base: Record<string, unknown>,
  overlay: Record<string, unknown>,
): Record<string, unknown> {
  const merged = { ...base };
  for (const [key, value] of Object.entries(overlay)) {
    const existing = merged[key];
    merged[key] =
      existing &&
      typeof existing === "object" &&
      !Array.isArray(existing) &&
      value &&
      typeof value === "object" &&
      !Array.isArray(value)
        ? mergeTranslationTrees(
            existing as Record<string, unknown>,
            value as Record<string, unknown>,
          )
        : value;
  }
  return merged;
}

function withSharedAccountTranslations<T extends Record<string, unknown>>(
  locale: T,
): T {
  const shared = locale.accountAuth;
  if (!shared || typeof shared !== "object" || Array.isArray(shared)) {
    return locale;
  }
  const translation: Record<string, unknown> = { ...locale };
  for (const namespace of ACCOUNT_TRANSLATION_NAMESPACES) {
    const existing = locale[namespace];
    translation[namespace] = {
      ...shared,
      ...(existing && typeof existing === "object" && !Array.isArray(existing)
        ? existing
        : {}),
    };
  }
  return translation as T;
}

const resources = {
  en: {
    translation: withSharedAccountTranslations(
      mergeTranslationTrees(en, serverEn),
    ),
  },
  ja: {
    translation: withSharedAccountTranslations(
      mergeTranslationTrees(ja, serverJa),
    ),
  },
  zh: {
    translation: withSharedAccountTranslations(
      mergeTranslationTrees(zh, serverZh),
    ),
  },
  "zh-TW": {
    translation: withSharedAccountTranslations(
      mergeTranslationTrees(zhTW, serverZhTW),
    ),
  },
};

i18n.use(initReactI18next).init({
  resources,
  lng: getInitialLanguage(),
  fallbackLng: "en",

  interpolation: {
    escapeValue: false, // React 已经默认转义
  },

  showSupportNotice: false,

  debug: false,
});

export default i18n;
