import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { useSettingsQuery, useSaveSettingsMutation } from "@/lib/query";
import { getWebRuntimeContext } from "@/lib/runtime";
import type { Settings } from "@/types";
import { useSettingsForm, type SettingsFormState } from "./useSettingsForm";

interface SaveResult {
  requiresRestart: false;
}

export interface UseSettingsResult {
  settings: SettingsFormState | null;
  isLoading: boolean;
  isSaving: boolean;
  configDir: string;
  updateSettings: (updates: Partial<SettingsFormState>) => void;
  saveSettings: (
    overrides?: Partial<SettingsFormState>,
    options?: { silent?: boolean },
  ) => Promise<SaveResult | null>;
  autoSaveSettings: (
    updates: Partial<SettingsFormState>,
  ) => Promise<SaveResult | null>;
  resetSettings: () => void;
}

export type { SettingsFormState };

function toServerSettings(settings: SettingsFormState): Settings {
  return { ...settings };
}

function persistLanguage(language: SettingsFormState["language"]): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem("language", language);
  } catch (error) {
    console.warn("[settings] Failed to persist language preference", error);
  }
}

/**
 * Server-only settings state.
 *
 * The Server Web UI persists its JSON settings through /web-api and reads the
 * runtime config directory from /web-api/context. Desktop directory pickers,
 * auto-launch, tray refresh, plugin mutation, and application restart do not
 * belong in this product boundary.
 */
export function useSettings(): UseSettingsResult {
  const { t } = useTranslation();
  const { data } = useSettingsQuery();
  const saveMutation = useSaveSettingsMutation();
  const {
    settings,
    isLoading: isFormLoading,
    initialLanguage,
    updateSettings,
    resetSettings: resetForm,
    syncLanguage,
  } = useSettingsForm();
  const [configDir, setConfigDir] = useState("");
  const [isRuntimeLoading, setIsRuntimeLoading] = useState(true);

  useEffect(() => {
    let active = true;
    void getWebRuntimeContext()
      .then((context) => {
        if (active) setConfigDir(context.runtime?.configDir ?? "");
      })
      .catch((error) => {
        console.warn("[settings] Failed to load Server runtime context", error);
      })
      .finally(() => {
        if (active) setIsRuntimeLoading(false);
      });
    return () => {
      active = false;
    };
  }, []);

  const persist = useCallback(
    async (
      nextSettings: SettingsFormState,
      options?: { silent?: boolean },
    ): Promise<SaveResult> => {
      const payload = toServerSettings(nextSettings);
      try {
        await saveMutation.mutateAsync(payload);
        persistLanguage(nextSettings.language);
        if (!options?.silent) {
          toast.success(
            t("notifications.settingsSaved", {
              defaultValue: "设置已保存",
            }),
            { closeButton: true },
          );
        }
        return { requiresRestart: false };
      } catch (error) {
        console.error("[settings] Failed to save Server settings", error);
        toast.error(
          t("notifications.settingsSaveFailed", {
            defaultValue: "保存设置失败: {{error}}",
            error: (error as Error)?.message ?? String(error),
          }),
        );
        throw error;
      }
    },
    [saveMutation, t],
  );

  const autoSaveSettings = useCallback(
    async (
      updates: Partial<SettingsFormState>,
    ): Promise<SaveResult | null> => {
      if (!settings) return null;
      return persist({ ...settings, ...updates }, { silent: true });
    },
    [persist, settings],
  );

  const saveSettings = useCallback(
    async (
      overrides?: Partial<SettingsFormState>,
      options?: { silent?: boolean },
    ): Promise<SaveResult | null> => {
      if (!settings) return null;
      return persist({ ...settings, ...overrides }, options);
    },
    [persist, settings],
  );

  const resetSettings = useCallback(() => {
    resetForm(data ?? null);
    syncLanguage(initialLanguage);
  }, [data, initialLanguage, resetForm, syncLanguage]);

  const isLoading = useMemo(
    () => isFormLoading || isRuntimeLoading,
    [isFormLoading, isRuntimeLoading],
  );

  return {
    settings,
    isLoading,
    isSaving: saveMutation.isPending,
    configDir,
    updateSettings,
    saveSettings,
    autoSaveSettings,
    resetSettings,
  };
}
