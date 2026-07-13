import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";

export interface SubdomainSuggestion {
  subdomain: string;
  available: boolean;
  checked: boolean;
  attempts: number;
}

interface SubdomainGeneratorButtonProps {
  disabled?: boolean;
  onGenerated: (subdomain: string) => void;
  onError?: (message: string) => void;
  suggest: () => Promise<SubdomainSuggestion>;
  size?: "default" | "sm" | "lg" | "icon";
  variant?: "default" | "outline" | "secondary" | "ghost";
}

export function SubdomainGeneratorButton({
  disabled = false,
  onGenerated,
  onError,
  suggest,
  size = "sm",
  variant = "outline",
}: SubdomainGeneratorButtonProps) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);

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
            : t("server.auth.generateSubdomainFailed", {
                defaultValue: "随机生成子域名失败",
              });
      onError?.(message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Button
      type="button"
      variant={variant}
      size={size}
      disabled={disabled || busy}
      onClick={() => void handleClick()}
    >
      {busy
        ? t("server.auth.subdomainGenerating", { defaultValue: "生成中…" })
        : t("server.auth.generateSubdomain", { defaultValue: "随机生成" })}
    </Button>
  );
}
