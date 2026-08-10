import { AlertTriangle, LoaderCircle, RotateCcw } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import {
  findAccountCapability,
  resolveManagedAccountCapabilityState,
  useAccountCapabilitiesQuery,
} from "@/lib/query/accounts";
import { AntigravityOAuthSection } from "./AntigravityOAuthSection";
import { ClaudeOAuthSection } from "./ClaudeOAuthSection";
import { CodexOAuthSection } from "./CodexOAuthSection";
import { CopilotAuthSection } from "./CopilotAuthSection";
import { CursorOAuthSection } from "./CursorOAuthSection";
import { DeepSeekAccountSection } from "./DeepSeekAccountSection";
import { GeminiOAuthSection } from "./GeminiOAuthSection";
import { GrokOAuthSection } from "./GrokOAuthSection";
import { KiroOAuthSection } from "./KiroOAuthSection";
import { KimiOAuthSection } from "./KimiOAuthSection";

export const MANAGED_ACCOUNT_SECTION_PROVIDER_TYPES = [
  "claude_oauth",
  "codex_oauth",
  "grok_oauth",
  "github_copilot",
  "gemini_cli",
  "antigravity_oauth",
  "agy_oauth",
  "cursor_oauth",
  "kiro_oauth",
  "kimi_code",
  "deepseek_account",
] as const;

export type ManagedAccountSectionProviderType =
  (typeof MANAGED_ACCOUNT_SECTION_PROVIDER_TYPES)[number];

const MANAGED_ACCOUNT_SECTION_PROVIDER_TYPE_SET = new Set<string>(
  MANAGED_ACCOUNT_SECTION_PROVIDER_TYPES,
);

export function resolveManagedAccountSectionProviderType(
  providerType: string,
): ManagedAccountSectionProviderType | null {
  return MANAGED_ACCOUNT_SECTION_PROVIDER_TYPE_SET.has(providerType)
    ? (providerType as ManagedAccountSectionProviderType)
    : null;
}

interface ManagedAccountSectionProps {
  providerType: string;
  selectedAccountId: string | null;
  onAccountSelect: (accountId: string | null) => void;
}

export function ManagedAccountSection({
  providerType,
  selectedAccountId,
  onAccountSelect,
}: ManagedAccountSectionProps) {
  const { t } = useTranslation();
  const capabilityQuery = useAccountCapabilitiesQuery();
  const capability = findAccountCapability(capabilityQuery.data, providerType);
  const capabilityState = resolveManagedAccountCapabilityState(
    capabilityQuery.status,
    capability,
  );

  if (capabilityState === "loading") {
    return (
      <div className="flex items-center gap-2 text-sm text-muted-foreground">
        <LoaderCircle className="h-4 w-4 animate-spin" />
        {t("common.loading")}
      </div>
    );
  }

  if (capabilityState === "load_error") {
    return (
      <div
        className="flex flex-wrap items-center justify-between gap-3 rounded-md border border-destructive/40 p-3 text-sm text-destructive"
        role="alert"
      >
        <span className="flex min-w-0 items-center gap-2">
          <AlertTriangle className="h-4 w-4 shrink-0" />
          {t("settings.authCenter.capabilityLoadFailed", {
            defaultValue: "无法加载账号能力，供应商账号绑定已暂停。",
          })}
        </span>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={capabilityQuery.isFetching}
          onClick={() => void capabilityQuery.refetch()}
        >
          {capabilityQuery.isFetching ? (
            <LoaderCircle className="mr-2 h-4 w-4 animate-spin" />
          ) : (
            <RotateCcw className="mr-2 h-4 w-4" />
          )}
          {t("common.retry")}
        </Button>
      </div>
    );
  }

  const sectionProviderType =
    resolveManagedAccountSectionProviderType(providerType);
  if (capabilityState === "unsupported" || !sectionProviderType) {
    return (
      <div className="flex items-center gap-2 text-sm text-destructive">
        <AlertTriangle className="h-4 w-4" />
        {t("serverProviderForm.unsupportedAccountType", {
          type: providerType,
        })}
      </div>
    );
  }

  const common = {
    selectedAccountId,
    onAccountSelect,
    showLoggedInAccounts: false,
  };

  switch (sectionProviderType) {
    case "claude_oauth":
      return <ClaudeOAuthSection {...common} />;
    case "codex_oauth":
      return <CodexOAuthSection {...common} accountSelectionMode="provider" />;
    case "grok_oauth":
      return <GrokOAuthSection {...common} allowDefaultAccountOption={false} />;
    case "github_copilot":
      return <CopilotAuthSection {...common} />;
    case "gemini_cli":
      return (
        <GeminiOAuthSection {...common} allowDefaultAccountOption={false} />
      );
    case "antigravity_oauth":
      return (
        <AntigravityOAuthSection {...common} authProvider="antigravity_oauth" />
      );
    case "agy_oauth":
      return <AntigravityOAuthSection {...common} authProvider="agy_oauth" />;
    case "cursor_oauth":
      return <CursorOAuthSection {...common} />;
    case "kiro_oauth":
      return <KiroOAuthSection {...common} />;
    case "kimi_code":
      return <KimiOAuthSection {...common} />;
    case "deepseek_account":
      return <DeepSeekAccountSection {...common} />;
  }

  const exhaustiveProviderType: never = sectionProviderType;
  return exhaustiveProviderType;
}
