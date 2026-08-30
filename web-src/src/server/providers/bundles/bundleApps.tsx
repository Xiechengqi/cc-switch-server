import { ClaudeIcon, CodexIcon, GeminiIcon } from "@/components/BrandIcons";
import type { CoreProviderApp } from "@/server/providerRegistry";
import { cn } from "@/lib/utils";

export const APP_LABELS: Record<CoreProviderApp, string> = {
  claude: "Claude",
  codex: "Codex",
  gemini: "Gemini",
};

export function AppLogo({
  app,
  size = 16,
  muted = false,
  className,
}: {
  app: CoreProviderApp;
  size?: number;
  /** Renders the App as "supported but not in play here". */
  muted?: boolean;
  className?: string;
}) {
  const Icon =
    app === "claude" ? ClaudeIcon : app === "codex" ? CodexIcon : GeminiIcon;
  return (
    <Icon
      size={size}
      className={cn(muted && "opacity-30 grayscale", className)}
    />
  );
}
