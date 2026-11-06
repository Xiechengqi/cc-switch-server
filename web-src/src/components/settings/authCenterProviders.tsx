import type { ReactNode } from "react";
import type { TFunction } from "i18next";
import { Github, Sparkles } from "lucide-react";
import { ClaudeIcon, CodexIcon, DeepSeekIcon, GeminiIcon } from "@/components/BrandIcons";
import { ProviderIcon } from "@/components/ProviderIcon";
import { AntigravityOAuthSection } from "@/components/providers/forms/AntigravityOAuthSection";
import { AmazonQOAuthSection } from "@/components/providers/forms/AmazonQOAuthSection";
import { ClaudeOAuthSection } from "@/components/providers/forms/ClaudeOAuthSection";
import { CodeBuddyOAuthSection } from "@/components/providers/forms/CodeBuddyOAuthSection";
import { CodexOAuthSection } from "@/components/providers/forms/CodexOAuthSection";
import { CopilotAuthSection } from "@/components/providers/forms/CopilotAuthSection";
import { CursorOAuthSection } from "@/components/providers/forms/CursorOAuthSection";
import { DeepSeekAccountSection } from "@/components/providers/forms/DeepSeekAccountSection";
import { GeminiOAuthSection } from "@/components/providers/forms/GeminiOAuthSection";
import { GrokOAuthSection } from "@/components/providers/forms/GrokOAuthSection";
import { KiroOAuthSection } from "@/components/providers/forms/KiroOAuthSection";
import { KimiOAuthSection } from "@/components/providers/forms/KimiOAuthSection";
import { TraeOAuthSection } from "@/components/providers/forms/TraeOAuthSection";
import { QoderOAuthSection } from "@/components/providers/forms/QoderOAuthSection";
import type { ManagedAccountSectionProviderType } from "@/components/providers/forms/ManagedAccountSection";
import type { CodeBuddySite } from "@/lib/api/auth";
import type { BuddyProviderType } from "@/lib/api/providers";

interface AuthCenterProviderEntry {
  providerType: ManagedAccountSectionProviderType;
  value: string;
  icon: ReactNode;
  title: string;
  description: string;
  content: ReactNode;
}

interface BuddyLoginOptions {
  sites: Record<BuddyProviderType, CodeBuddySite>;
  onSiteChange: (product: BuddyProviderType, site: CodeBuddySite) => void;
}

// Capabilities decide eligibility; this registry supplies the product-specific
// renderer. audit-ui-provider-matrix checks it against Rust account ownership.
export function getAuthCenterProviders(
  t: TFunction,
  buddy: BuddyLoginOptions,
): AuthCenterProviderEntry[] {
  return [
    {
      providerType: "claude_oauth",
      value: "claude",
      icon: <ClaudeIcon size={20} />,
      title: "Claude Official",
      description: t("settings.authCenter.claudeOauthDescription"),
      content: <ClaudeOAuthSection showLoggedInAccounts />,
    },
    {
      providerType: "github_copilot",
      value: "copilot",
      icon: <Github className="h-5 w-5" />,
      title: "GitHub Copilot",
      description: t("settings.authCenter.copilotDescription"),
      content: <CopilotAuthSection showLoggedInAccounts />,
    },
    {
      providerType: "codex_oauth",
      value: "codex",
      icon: <CodexIcon size={20} />,
      title: "OpenAI OAuth",
      description: t("settings.authCenter.codexOauthDescription"),
      content: <CodexOAuthSection showLoggedInAccounts />,
    },
    {
      providerType: "grok_oauth",
      value: "grok",
      icon: <ProviderIcon icon="grok" name="Grok" size={24} />,
      title: "Grok OAuth",
      description: t("settings.authCenter.grokOauthDescription"),
      content: <GrokOAuthSection showLoggedInAccounts />,
    },
    {
      providerType: "kiro_oauth",
      value: "kiro",
      icon: <ProviderIcon icon="kiro" name="Kiro" size={24} />,
      title: "Kiro OAuth",
      description: t("settings.authCenter.kiroOauthDescription"),
      content: <KiroOAuthSection showLoggedInAccounts />,
    },
    {
      providerType: "amazon_q_oauth",
      value: "amazon-q",
      icon: <ProviderIcon icon="aws" name="Amazon Q" size={24} />,
      title: t("amazonQOauth.title"),
      description: t("amazonQOauth.description"),
      content: <AmazonQOAuthSection showLoggedInAccounts />,
    },
    {
      providerType: "kimi_code",
      value: "kimi-code",
      icon: <ProviderIcon icon="kimi" name="Kimi OAuth" size={24} />,
      title: "Kimi OAuth",
      description: t("kimiOauth.description"),
      content: <KimiOAuthSection showLoggedInAccounts />,
    },
    {
      providerType: "qoder_cosy",
      value: "qoder-cosy",
      icon: <ProviderIcon icon="qoder" name="Qoder OAuth" size={24} />,
      title: "Qoder OAuth",
      description: t("qoderOauth.description"),
      content: <QoderOAuthSection showLoggedInAccounts />,
    },
    {
      providerType: "codebuddy_oauth",
      value: "codebuddy",
      icon: <ProviderIcon icon="codebuddy" name="CodeBuddy" size={24} />,
      title: "CodeBuddy OAuth",
      description: t("settings.authCenter.codeBuddyOauthDescription"),
      content: (
        <CodeBuddyOAuthSection
          authProvider="codebuddy_oauth"
          context="auth-center"
          showLoggedInAccounts
          loginSite={buddy.sites.codebuddy_oauth}
          onLoginSiteChange={(site) => buddy.onSiteChange("codebuddy_oauth", site)}
        />
      ),
    },
    {
      providerType: "workbuddy_oauth",
      value: "workbuddy",
      icon: <ProviderIcon icon="workbuddy" name="WorkBuddy" size={24} />,
      title: "WorkBuddy OAuth",
      description: t("settings.authCenter.workBuddyOauthDescription"),
      content: (
        <CodeBuddyOAuthSection
          authProvider="workbuddy_oauth"
          context="auth-center"
          showLoggedInAccounts
          loginSite={buddy.sites.workbuddy_oauth}
          onLoginSiteChange={(site) => buddy.onSiteChange("workbuddy_oauth", site)}
        />
      ),
    },
    {
      providerType: "trae_solo", value: "trae_solo",
      icon: <Sparkles className="h-5 w-5" />, title: "Trae CN Solo",
      description: t("settings.authCenter.trae_solo", { defaultValue: "国内 SOLO 账号，支持原生工具调用。" }),
      content: <TraeOAuthSection authProvider="trae_solo" showLoggedInAccounts />,
    },
    {
      providerType: "trae_raw_chat", value: "trae_raw_chat",
      icon: <Sparkles className="h-5 w-5" />, title: "Trae CN raw_chat",
      description: t("settings.authCenter.trae_raw_chat", { defaultValue: "国内聊天账号，支持图片输入，可选工具提示词模拟。" }),
      content: <TraeOAuthSection authProvider="trae_raw_chat" showLoggedInAccounts />,
    },
    {
      providerType: "trae_raw_chat_intl", value: "trae_raw_chat_intl",
      icon: <Sparkles className="h-5 w-5" />, title: "Trae Intl raw_chat",
      description: t("settings.authCenter.trae_raw_chat_intl", { defaultValue: "国际站聊天账号，使用独立凭据并指定上游模型。" }),
      content: <TraeOAuthSection authProvider="trae_raw_chat_intl" showLoggedInAccounts />,
    },
    {
      providerType: "cursor_oauth",
      value: "cursor",
      icon: <ProviderIcon icon="cursor" name="Cursor" size={24} />,
      title: "Cursor OAuth",
      description: t("settings.authCenter.cursorOauthDescription"),
      content: <CursorOAuthSection showLoggedInAccounts />,
    },
    {
      providerType: "gemini_cli",
      value: "gemini",
      icon: <GeminiIcon size={20} />,
      title: "Google Gemini",
      description: t("settings.authCenter.geminiOauthDescription"),
      content: <GeminiOAuthSection showLoggedInAccounts />,
    },
    {
      providerType: "antigravity_oauth",
      value: "antigravity",
      icon: <Sparkles className="h-5 w-5" />,
      title: "Antigravity OAuth",
      description: t("settings.authCenter.antigravityOauthDescription"),
      content: <AntigravityOAuthSection showLoggedInAccounts />,
    },
    {
      providerType: "agy_oauth",
      value: "agy",
      icon: <Sparkles className="h-5 w-5" />,
      title: t("settings.authCenter.agyOauthTitle"),
      description: t("settings.authCenter.agyOauthDescription"),
      content: <AntigravityOAuthSection showLoggedInAccounts authProvider="agy_oauth" />,
    },
    {
      providerType: "deepseek_account",
      value: "deepseek",
      icon: <DeepSeekIcon size={20} />,
      title: "DeepSeek(Account)",
      description: t("settings.authCenter.deepseekAccountDescription"),
      content: <DeepSeekAccountSection showLoggedInAccounts />,
    },
  ];
}
