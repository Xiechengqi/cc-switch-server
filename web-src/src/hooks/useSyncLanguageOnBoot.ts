import { useEffect } from "react";
import { useTranslation } from "react-i18next";

import { useSettingsQuery } from "@/lib/query";

type Language = "zh" | "zh-TW" | "en" | "ja";

function normalizeLanguage(lang?: string | null): Language | null {
  if (!lang) return null;
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
  if (normalized === "en") return "en";
  if (normalized === "ja") return "ja";
  if (normalized.startsWith("zh")) return "zh";
  return null;
}

/**
 * App 启动期语言同步。
 *
 * 背景：`src/i18n/index.ts` 初始化时只从 `localStorage["language"]` 读语言，
 * 取不到就退回 `navigator.language`（多数系统 = en-US）。用户在设置页选过
 * 的语言原本只持久化到后端 settings JSON，没有写 localStorage，且
 * `useSettingsForm` 的语言同步副作用只在 SettingsPage mount 时才会触发。
 *
 * 这导致老用户每次重启都先看到英文，必须进一次设置页才会切到中文。
 *
 * 本 hook 在 DesktopApp 根部调用：后端 settings 一拉到就同步到 i18n + 写回
 * localStorage，覆盖「老用户从未访问过设置页」的兜底场景。
 * `useSettingsForm.syncLanguage` 也已经在改语言时 setItem localStorage，
 * 所以新用户首次设置后立刻就被 i18n 启动路径直接命中，无需依赖本 hook。
 */
export function useSyncLanguageOnBoot(): void {
  const { i18n } = useTranslation();
  const { data } = useSettingsQuery();

  useEffect(() => {
    const desired = normalizeLanguage(data?.language);
    if (!desired) return;
    const current = normalizeLanguage(i18n.language);
    if (current !== desired) {
      void i18n.changeLanguage(desired);
    }
    if (typeof window !== "undefined") {
      try {
        const stored = window.localStorage.getItem("language");
        if (stored !== desired) {
          window.localStorage.setItem("language", desired);
        }
      } catch (error) {
        console.warn("[i18n] Failed to persist language preference", error);
      }
    }
  }, [data?.language, i18n]);
}
