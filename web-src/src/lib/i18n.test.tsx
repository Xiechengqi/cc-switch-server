import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it } from "vitest";

import i18n from "@/i18n";

import { I18nProvider, useI18n } from "./i18n";

function TranslationProbe({
  translationKey,
  defaultValue,
}: {
  translationKey: string;
  defaultValue?: string;
}) {
  const { t } = useI18n();
  return <span>{t(translationKey, { defaultValue, name: "Ada" })}</span>;
}

describe("useI18n", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("uses defaultValue for a missing translation key", () => {
    const html = renderToStaticMarkup(
      <I18nProvider>
        <TranslationProbe
          translationKey="test.missing.key"
          defaultValue="Hello {{name}}"
        />
      </I18nProvider>,
    );

    expect(html).toContain("Hello Ada");
  });

  it("resolves through the provider language instead of global i18n state", async () => {
    window.localStorage.setItem("language", "ja");
    await i18n.changeLanguage("zh");

    const html = renderToStaticMarkup(
      <I18nProvider>
        <TranslationProbe translationKey="common.cancel" />
      </I18nProvider>,
    );

    expect(html).toContain("キャンセル");
  });
});
