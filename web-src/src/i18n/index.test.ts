import { describe, expect, it } from "vitest";

import i18n, { normalizeLanguage } from "./index";
import serverEn from "./server-locales/en.json";
import serverJa from "./server-locales/ja.json";
import serverZh from "./server-locales/zh.json";
import serverZhTW from "./server-locales/zh-TW.json";

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
  "provider.unsavedChanges.editMessage",
  "provider.unsavedChanges.discard",
  "provider.unsavedChanges.keepEditing",
  "provider.share.enable",
  "provider.share.sharing",
  "provider.share.stop",
  "provider.share.resume",
  "provider.share.resumeShort",
  "provider.share.delete",
  "provider.share.deleteConfirmTitle",
  "provider.share.deleteConfirmMessage",
  "provider.share.deleteRemember",
  "settings.serverVersion.rollback",
  "settings.serverVersion.rollbackConfirmTitle",
  "settings.serverVersion.rollbackConfirmMessage",
  "share.confirmDeleteTitle",
  "share.confirmDeleteMessage",
  "share.freeAccess.label",
] as const;

const codexFeatureKeys = [
  "fastMode",
  "fastModeDescription",
  "imageGeneration",
  "imageGenerationDescription",
  "websocket",
  "websocketDescription",
] as const;

const providerBundleKeys = [
  "cardConnection",
  "categoryApiKey",
  "categoryCustom",
  "categorySubscription",
  "familySearchPlaceholder",
  "stepNavigation",
  "stepFamily",
  "stepSupply",
  "stepShare",
  "gapCredential",
  "validation.accountRequired",
] as const;

describe("language normalization", () => {
  it.each([
    ["zh", "zh"],
    ["zh-CN", "zh"],
    ["zh_Hans_CN", "zh"],
    ["zh-TW", "zh-TW"],
    ["zh-Hant-HK", "zh-TW"],
    ["ja-JP", "ja"],
    ["en-US", "en"],
    ["zh-XX", "en"],
    ["fr-FR", "en"],
    [undefined, "en"],
  ] as const)("maps %s to %s", (input, expected) => {
    expect(normalizeLanguage(input)).toBe(expected);
  });
});

describe("i18n resources", () => {
  it("uses the Server Provider creation wording", () => {
    expect(serverZh.providerBundle.stepFamily).toBe("选择类型");
    expect(serverZh.providerBundle.stepSupply).toBe("配置");
    expect(serverZh.providerBundle.stepShare).toBe("远程分享");
    expect(serverZh.providerBundle.categorySubscription).toBe("订阅账号");
    expect(serverZh.providerBundle.categoryApiKey).toBe("API Key");
    expect(serverZh.providerBundle.categoryCustom).toBe("自定义");
  });

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

  it("loads the standalone Server locale resources", () => {
    for (const language of languages) {
      expect(i18n.t("provider.share.sectionTitle", { lng: language })).toBe(
        serverLocales[language].provider.share.sectionTitle,
      );
    }
  });

  it("provides Server Provider Bundle editor copy in every supported language", () => {
    for (const language of languages) {
      for (const key of providerBundleKeys) {
        const path = key.split(".");
        const expected = path.reduce<unknown>(
          (value, segment) =>
            value && typeof value === "object"
              ? (value as Record<string, unknown>)[segment]
              : undefined,
          serverLocales[language].providerBundle,
        );
        expect(i18n.t(`providerBundle.${key}`, { lng: language })).toBe(
          expected,
        );
      }
    }
  });

  it.each(["userLimit"])(
    "provides Server Share %s copy in every supported language",
    (block) => {
      const keys = Object.keys(
        serverEn.share[block as keyof typeof serverEn.share] as Record<
          string,
          string
        >,
      );
      expect(keys.length).toBeGreaterThan(0);
      for (const language of languages) {
        const bundle = serverLocales[language].share[block] as Record<
          string,
          string
        >;
        expect(Object.keys(bundle).sort(), `${language}:key-set`).toEqual(
          [...keys].sort(),
        );
        for (const key of keys) {
          expect(
            i18n.t(`share.${block}.${key}`, { lng: language }),
            `${language}:share.${block}.${key}`,
          ).toBe(bundle[key]);
        }
      }
    },
  );

  it("provides Server Codex feature copy in every supported language", () => {
    for (const language of languages) {
      for (const key of codexFeatureKeys) {
        expect(i18n.t(`codexOauth.${key}`, { lng: language })).toBe(
          serverLocales[language].codexOauth[key],
        );
      }
    }
  });

  it("shares account copy while retaining provider-specific overrides", () => {
    for (const language of languages) {
      expect(i18n.t("claudeOauth.retry", { lng: language })).toBe(
        serverLocales[language].accountAuth.retry,
      );
      expect(
        i18n.t("codexOauth.selectAccountPlaceholder", { lng: language }),
      ).toBe(serverLocales[language].codexOauth.selectAccountPlaceholder);
    }
  });
});
