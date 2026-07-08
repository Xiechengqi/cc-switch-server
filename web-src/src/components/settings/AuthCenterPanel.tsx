import { useEffect, useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Github, ShieldCheck, Sparkles as SparklesIcon } from "lucide-react";
import { motion } from "framer-motion";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import {
  ClaudeIcon,
  CodexIcon,
  DeepSeekIcon,
  GeminiIcon,
} from "@/components/BrandIcons";
import { CopilotAuthSection } from "@/components/providers/forms/CopilotAuthSection";
import { AntigravityOAuthSection } from "@/components/providers/forms/AntigravityOAuthSection";
import { CodexOAuthSection } from "@/components/providers/forms/CodexOAuthSection";
import { ClaudeOAuthSection } from "@/components/providers/forms/ClaudeOAuthSection";
import { CursorOAuthSection } from "@/components/providers/forms/CursorOAuthSection";
import { DeepSeekAccountSection } from "@/components/providers/forms/DeepSeekAccountSection";
import { GeminiOAuthSection } from "@/components/providers/forms/GeminiOAuthSection";
import { KiroOAuthSection } from "@/components/providers/forms/KiroOAuthSection";
import { ProviderIcon } from "@/components/ProviderIcon";
import { settingsApi } from "@/lib/api";
import { useSettingsQuery } from "@/lib/query";
import {
  DEFAULT_OAUTH_QUOTA_REFRESH_INTERVAL_MINUTES,
  getOauthQuotaRefreshIntervalMinutes,
} from "@/lib/query/oauthQuotaRefresh";

interface AuthProviderAccordionItemProps {
  value: string;
  icon: ReactNode;
  title: string;
  description: string;
  children: ReactNode;
}

function AuthProviderAccordionItem({
  value,
  icon,
  title,
  description,
  children,
}: AuthProviderAccordionItemProps) {
  return (
    <AccordionItem
      value={value}
      className="rounded-xl glass-card overflow-hidden"
    >
      <AccordionTrigger className="px-6 py-4 hover:no-underline hover:bg-muted/50 data-[state=open]:bg-muted/50">
        <div className="flex items-center gap-3">
          <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-background ring-1 ring-border">
            {icon}
          </div>
          <div className="text-left">
            <h3 className="text-base font-semibold">{title}</h3>
            <p className="text-sm font-normal text-muted-foreground">
              {description}
            </p>
          </div>
        </div>
      </AccordionTrigger>
      <AccordionContent className="border-t border-border/50 px-6 pb-6 pt-4">
        {children}
      </AccordionContent>
    </AccordionItem>
  );
}

export function AuthCenterPanel() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data: settings } = useSettingsQuery();
  const currentRefreshInterval = getOauthQuotaRefreshIntervalMinutes(settings);
  const [refreshIntervalInput, setRefreshIntervalInput] = useState(
    String(DEFAULT_OAUTH_QUOTA_REFRESH_INTERVAL_MINUTES),
  );

  useEffect(() => {
    setRefreshIntervalInput(String(currentRefreshInterval));
  }, [currentRefreshInterval]);

  const parsedRefreshIntervalValue = Number(refreshIntervalInput);
  const parsedRefreshInterval =
    Number.isFinite(parsedRefreshIntervalValue) &&
    Number.isInteger(parsedRefreshIntervalValue) &&
    parsedRefreshIntervalValue >= 1
      ? parsedRefreshIntervalValue
      : null;

  const handleSaveRefreshInterval = async () => {
    if (!settings) {
      return;
    }
    if (parsedRefreshInterval == null) {
      toast.error(
        t("settings.authCenter.quotaRefreshIntervalInvalid", {
          defaultValue: "刷新间隔必须是大于等于 1 的整数分钟",
        }),
      );
      setRefreshIntervalInput(String(currentRefreshInterval));
      return;
    }

    const { webdavSync: _, ...rest } = settings;
    await settingsApi.save({
      ...rest,
      oauthQuotaRefreshIntervalMinutes: parsedRefreshInterval,
    });
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["settings"] }),
      queryClient.invalidateQueries({ queryKey: ["subscription", "quota"] }),
      queryClient.invalidateQueries({ queryKey: ["claude_oauth", "quota"] }),
      queryClient.invalidateQueries({ queryKey: ["codex_oauth", "quota"] }),
      queryClient.invalidateQueries({
        queryKey: ["google_gemini_oauth", "quota"],
      }),
      queryClient.invalidateQueries({ queryKey: ["kiro_oauth", "quota"] }),
      queryClient.invalidateQueries({ queryKey: ["copilot", "quota"] }),
    ]);
    toast.success(
      t("settings.authCenter.quotaRefreshIntervalSaved", {
        defaultValue: "用量刷新间隔已保存",
      }),
    );
  };

  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3 }}
      className="space-y-4"
    >
      <section className="rounded-xl border border-border/60 bg-card/60 p-6">
        <div className="flex items-start justify-between gap-4">
          <div className="space-y-2">
            <div className="flex items-center gap-2">
              <ShieldCheck className="h-5 w-5 text-primary" />
              <h3 className="text-base font-semibold">
                {t("settings.authCenter.title", {
                  defaultValue: "OAuth 认证中心",
                })}
              </h3>
            </div>
            <p className="text-sm text-muted-foreground">
              {t("settings.authCenter.description", {
                defaultValue:
                  "在 Claude Code 中使用您的其他订阅，请注意合规风险。",
              })}
            </p>
          </div>
          <Badge variant="secondary">
            {t("settings.authCenter.beta", { defaultValue: "Beta" })}
          </Badge>
        </div>

        <div className="mt-5 rounded-lg border border-border/50 bg-background/60 p-4">
          <div className="flex flex-col gap-4 md:flex-row md:items-end md:justify-between">
            <div className="space-y-2">
              <Label htmlFor="oauth-quota-refresh-interval">
                {t("settings.authCenter.quotaRefreshIntervalTitle", {
                  defaultValue: "用量刷新间隔",
                })}
              </Label>
              <p className="max-w-2xl text-sm text-muted-foreground">
                {t("settings.authCenter.quotaRefreshIntervalDescription", {
                  defaultValue:
                    "控制 OAuth 账号 5h / 7day 用量进度条的自动刷新频率，仅当前激活供应商会自动轮询。",
                })}
              </p>
            </div>
            <div className="flex items-center gap-2">
              <Input
                id="oauth-quota-refresh-interval"
                type="number"
                min={1}
                step={1}
                value={refreshIntervalInput}
                onChange={(event) =>
                  setRefreshIntervalInput(event.currentTarget.value)
                }
                className="w-24"
                disabled={!settings}
              />
              <span className="text-sm text-muted-foreground">
                {t("settings.authCenter.quotaRefreshIntervalMinutes", {
                  defaultValue: "分钟",
                })}
              </span>
              <Button
                type="button"
                size="sm"
                onClick={handleSaveRefreshInterval}
                disabled={
                  !settings || parsedRefreshInterval === currentRefreshInterval
                }
              >
                {t("common.save", { defaultValue: "保存" })}
              </Button>
            </div>
          </div>
        </div>
      </section>

      <Accordion type="multiple" defaultValue={[]} className="w-full space-y-4">
        <AuthProviderAccordionItem
          value="claude"
          icon={<ClaudeIcon size={20} />}
          title="Claude Official"
          description={t("settings.authCenter.claudeOauthDescription", {
            defaultValue: "管理 Claude 官方订阅账号",
          })}
        >
          <ClaudeOAuthSection showLoggedInAccounts />
        </AuthProviderAccordionItem>

        <AuthProviderAccordionItem
          value="copilot"
          icon={<Github className="h-5 w-5" />}
          title="GitHub Copilot"
          description={t("settings.authCenter.copilotDescription", {
            defaultValue: "管理 GitHub Copilot 账号",
          })}
        >
          <CopilotAuthSection showLoggedInAccounts />
        </AuthProviderAccordionItem>

        <AuthProviderAccordionItem
          value="codex"
          icon={<CodexIcon size={20} />}
          title="OpenAI OAuth"
          description={t("settings.authCenter.codexOauthDescription", {
            defaultValue: "管理 ChatGPT 账号",
          })}
        >
          <CodexOAuthSection showLoggedInAccounts />
        </AuthProviderAccordionItem>

        <AuthProviderAccordionItem
          value="kiro"
          icon={<ProviderIcon icon="kiro" name="Kiro" size={24} />}
          title="Kiro OAuth"
          description={t("settings.authCenter.kiroOauthDescription", {
            defaultValue: "管理 Kiro AWS Builder ID 账号",
          })}
        >
          <KiroOAuthSection showLoggedInAccounts />
        </AuthProviderAccordionItem>

        <AuthProviderAccordionItem
          value="cursor"
          icon={<ProviderIcon icon="cursor" name="Cursor" size={24} />}
          title="Cursor OAuth"
          description={t("settings.authCenter.cursorOauthDescription", {
            defaultValue: "管理 Cursor 订阅账号",
          })}
        >
          <CursorOAuthSection showLoggedInAccounts />
        </AuthProviderAccordionItem>

        <AuthProviderAccordionItem
          value="gemini"
          icon={<GeminiIcon size={20} />}
          title="Google Gemini"
          description={t("settings.authCenter.geminiOauthDescription", {
            defaultValue: "管理 Google Gemini 账号",
          })}
        >
          <GeminiOAuthSection showLoggedInAccounts />
        </AuthProviderAccordionItem>

        <AuthProviderAccordionItem
          value="antigravity"
          icon={<SparklesIcon className="h-5 w-5" />}
          title="Antigravity OAuth"
          description={t("settings.authCenter.antigravityOauthDescription", {
            defaultValue: "管理 Antigravity 订阅账号",
          })}
        >
          <AntigravityOAuthSection showLoggedInAccounts />
        </AuthProviderAccordionItem>

        <AuthProviderAccordionItem
          value="deepseek"
          icon={<DeepSeekIcon size={20} />}
          title="DeepSeek(Account)"
          description={t("settings.authCenter.deepseekAccountDescription", {
            defaultValue: "管理 DeepSeek 账号",
          })}
        >
          <DeepSeekAccountSection showLoggedInAccounts />
        </AuthProviderAccordionItem>
      </Accordion>
    </motion.div>
  );
}
