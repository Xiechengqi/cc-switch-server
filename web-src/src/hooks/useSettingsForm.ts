import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { useSettingsQuery } from "@/lib/query";
import { SERVER_DEFAULT_SETTINGS } from "@/lib/serverDefaultSettings";
import type { Settings } from "@/types";

type Language = "zh" | "zh-TW" | "en" | "ja";

export type SettingsFormState = Omit<Settings, "language"> & {
  language: Language;
};

function normalizeLanguage(lang?: string | null): Language {
  if (!lang) return "en";
  const normalized = lang.toLowerCase().replace(/_/g, "-");
  if (normalized === "zh") return "zh";
  if (
    normalized === "zh-tw" ||
    normalized.startsWith("zh-hant") ||
    normalized.startsWith("zh-hk") ||
    normalized.startsWith("zh-mo")
  ) {
    return "zh-TW";
  }
  if (normalized === "en" || normalized === "ja") return normalized;
  return normalized.startsWith("zh") ? "zh" : "en";
}

function isSupportedLanguage(lang?: string | null): boolean {
  if (!lang) return false;
  const normalized = lang.toLowerCase().replace(/_/g, "-");
  return (
    normalized === "en" || normalized === "ja" || normalized.startsWith("zh")
  );
}

function normalizeSettings(
  data: Settings | null | undefined,
  language: Language,
): SettingsFormState {
  return {
    oauthQuotaRefreshIntervalMinutes:
      data?.oauthQuotaRefreshIntervalMinutes ??
      SERVER_DEFAULT_SETTINGS.oauthQuotaRefreshIntervalMinutes,
    oauthQuotaRefreshTimeoutSeconds:
      data?.oauthQuotaRefreshTimeoutSeconds ??
      SERVER_DEFAULT_SETTINGS.oauthQuotaRefreshTimeoutSeconds,
    language,
    backupIntervalHours:
      data?.backupIntervalHours ?? SERVER_DEFAULT_SETTINGS.backupIntervalHours,
    backupRetainCount:
      data?.backupRetainCount ?? SERVER_DEFAULT_SETTINGS.backupRetainCount,
    shareRouterDomain: data?.shareRouterDomain,
    upgradePolicy:
      data?.upgradePolicy ?? SERVER_DEFAULT_SETTINGS.upgradePolicy,
  };
}

export interface UseSettingsFormResult {
  settings: SettingsFormState | null;
  isLoading: boolean;
  initialLanguage: Language;
  updateSettings: (updates: Partial<SettingsFormState>) => void;
  resetSettings: (serverData: Settings | null) => void;
  readPersistedLanguage: () => Language;
  syncLanguage: (lang: Language) => void;
}

export function useSettingsForm(): UseSettingsFormResult {
  const { i18n } = useTranslation();
  const { data, isLoading } = useSettingsQuery();
  const [settingsState, setSettingsState] =
    useState<SettingsFormState | null>(null);
  const initialLanguageRef = useRef<Language>("en");

  const readPersistedLanguage = useCallback((): Language => {
    if (typeof window !== "undefined") {
      const stored = window.localStorage.getItem("language");
      if (isSupportedLanguage(stored)) return normalizeLanguage(stored);
    }
    return normalizeLanguage(i18n.language);
  }, [i18n.language]);

  const syncLanguage = useCallback(
    (lang: Language) => {
      if (typeof window !== "undefined") {
        try {
          window.localStorage.setItem("language", lang);
        } catch (error) {
          console.warn("[i18n] Failed to persist language preference", error);
        }
      }
      if (normalizeLanguage(i18n.language) !== lang) {
        void i18n.changeLanguage(lang);
      }
    },
    [i18n],
  );

  useEffect(() => {
    if (!data && isLoading) return;
    const language = normalizeLanguage(
      data?.language ?? readPersistedLanguage(),
    );
    setSettingsState(normalizeSettings(data, language));
    initialLanguageRef.current = language;
    syncLanguage(language);
  }, [data, isLoading, readPersistedLanguage, syncLanguage]);

  const updateSettings = useCallback(
    (updates: Partial<SettingsFormState>) => {
      setSettingsState((current) => {
        const next = {
          ...(current ??
            normalizeSettings(undefined, readPersistedLanguage())),
          ...updates,
        };
        if (updates.language) {
          next.language = normalizeLanguage(updates.language);
          syncLanguage(next.language);
        }
        return next;
      });
    },
    [readPersistedLanguage, syncLanguage],
  );

  const resetSettings = useCallback(
    (serverData: Settings | null) => {
      if (!serverData) return;
      const language = normalizeLanguage(
        serverData.language ?? readPersistedLanguage(),
      );
      setSettingsState(normalizeSettings(serverData, language));
      syncLanguage(initialLanguageRef.current);
    },
    [readPersistedLanguage, syncLanguage],
  );

  return {
    settings: settingsState,
    isLoading,
    initialLanguage: initialLanguageRef.current,
    updateSettings,
    resetSettings,
    readPersistedLanguage,
    syncLanguage,
  };
}
