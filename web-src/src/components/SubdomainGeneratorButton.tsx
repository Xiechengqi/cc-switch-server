import { useState } from "react";
import { Loader2, Shuffle } from "lucide-react";

import { useI18n } from "@/lib/i18n";

export interface SubdomainSuggestion {
  subdomain: string;
  available: boolean;
  checked: boolean;
  attempts: number;
}

interface SubdomainGeneratorButtonProps {
  disabled?: boolean;
  embedded?: boolean;
  className?: string;
  onGenerated: (subdomain: string) => void;
  onError?: (message: string) => void;
  suggest: () => Promise<SubdomainSuggestion>;
}

export function SubdomainGeneratorButton({
  disabled = false,
  embedded = true,
  className = "",
  onGenerated,
  onError,
  suggest,
}: SubdomainGeneratorButtonProps) {
  const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  const label = t("server.auth.generateSubdomain");

  async function handleClick() {
    if (disabled || busy) return;
    setBusy(true);
    try {
      const outcome = await suggest();
      onGenerated(outcome.subdomain);
    } catch (reason) {
      const message =
        reason instanceof Error
          ? reason.message
          : typeof reason === "string"
            ? reason
            : t("server.auth.generateSubdomainFailed");
      onError?.(message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <button
      type="button"
      className={
        embedded
          ? `subdomain-field-action${className ? ` ${className}` : ""}`
          : `icon-button subdomain-generate-button${className ? ` ${className}` : ""}`
      }
      disabled={disabled || busy}
      aria-label={label}
      title={label}
      onClick={() => void handleClick()}
    >
      {busy ? <Loader2 size={15} className="spin" /> : <Shuffle size={15} />}
    </button>
  );
}
