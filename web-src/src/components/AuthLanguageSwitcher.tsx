import { useI18n, type Language } from "@/lib/i18n";

const AUTH_LANGUAGE_OPTIONS: Array<{ value: Language; label: string }> = [
  { value: "zh", label: "中文" },
  { value: "en", label: "EN" },
];

export function AuthLanguageSwitcher() {
  const { language, setLanguage } = useI18n();

  return (
    <div className="auth-language-switch" role="group" aria-label="Language">
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
