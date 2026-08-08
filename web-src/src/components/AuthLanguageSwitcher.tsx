import { useTranslation } from "react-i18next";

import { normalizeLanguage, type Language } from "@/i18n";

const AUTH_LANGUAGE_OPTIONS: Array<{ value: Language; label: string }> = [
  { value: "zh", label: "中文" },
  { value: "en", label: "EN" },
];

export function AuthLanguageSwitcher() {
  const { i18n, t } = useTranslation();
  const language = normalizeLanguage(i18n.resolvedLanguage ?? i18n.language);

  const setLanguage = (next: Language) => {
    try {
      window.localStorage.setItem("language", next);
    } catch (error) {
      console.warn("[i18n] Failed to persist language preference", error);
    }
    void i18n.changeLanguage(next);
  };

  return (
    <div
      className="auth-language-switch"
      role="group"
      aria-label={t("settings.language")}
    >
      {AUTH_LANGUAGE_OPTIONS.map((option) => (
        <button
          key={option.value}
          type="button"
          className={language === option.value ? "active" : ""}
          aria-pressed={language === option.value}
          onClick={() => setLanguage(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}
