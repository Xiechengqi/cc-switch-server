import { describe, expect, it } from "vitest";

import i18n from "./index";
import en from "./locales/en.json";
import ja from "./locales/ja.json";
import zh from "./locales/zh.json";
import zhTW from "./locales/zh-TW.json";
import serverEn from "./server-locales/en.json";
import serverJa from "./server-locales/ja.json";
import serverZh from "./server-locales/zh.json";
import serverZhTW from "./server-locales/zh-TW.json";

const locales = { en, ja, zh, "zh-TW": zhTW } as const;
const serverLocales = {
  en: serverEn,
  ja: serverJa,
  zh: serverZh,
  "zh-TW": serverZhTW,
} as const;

const languages = ["en", "ja", "zh", "zh-TW"] as const;

const criticalKeys = [
  "confirm.deleteProvider",
  "confirm.deleteProviderMessage",
  "provider.unsavedChanges.title",
  "provider.unsavedChanges.addMessage",
  "provider.unsavedChanges.editMessage",
  "provider.unsavedChanges.discard",
  "provider.unsavedChanges.keepEditing",
  "provider.share.deleteConfirmTitle",
  "provider.share.deleteConfirmMessage",
  "provider.share.deleteRemember",
  "settings.serverVersion.rollback",
  "settings.serverVersion.rollbackConfirmTitle",
  "settings.serverVersion.rollbackConfirmMessage",
  "share.confirmDeleteTitle",
  "share.confirmDeleteMessage",
  "share.freeConfirm",
] as const;

describe("i18n resources", () => {
  it("provides every critical dialog string in all supported languages", () => {
    for (const language of languages) {
      for (const key of criticalKeys) {
        expect(i18n.exists(key, { lng: language }), `${language}:${key}`).toBe(
          true,
        );
        expect(i18n.t(key, { lng: language }), `${language}:${key}`).not.toBe(
          key,
        );
      }
    }
  });

  it("deep-merges server overlays without dropping desktop translations", () => {
    for (const language of languages) {
      expect(i18n.t("provider.share.sectionTitle", { lng: language })).toBe(
        serverLocales[language].provider.share.sectionTitle,
      );
      expect(i18n.t("provider.name", { lng: language })).toBe(
        locales[language].provider.name,
      );
    }
  });

  it("shares account copy while retaining provider-specific overrides", () => {
    for (const language of languages) {
      expect(i18n.t("claudeOauth.retry", { lng: language })).toBe(
        locales[language].accountAuth.retry,
      );
      expect(
        i18n.t("codexOauth.selectAccountPlaceholder", { lng: language }),
      ).toBe(locales[language].codexOauth.selectAccountPlaceholder);
    }
  });
});
