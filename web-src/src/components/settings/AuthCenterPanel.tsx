import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ChevronDown, Github, ShieldCheck, Sparkles as SparklesIcon } from "lucide-react";
import { motion } from "framer-motion";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
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
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
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
import { GrokOAuthSection } from "@/components/providers/forms/GrokOAuthSection";
import { KiroOAuthSection } from "@/components/providers/forms/KiroOAuthSection";
import { ProviderIcon } from "@/components/ProviderIcon";
import { settingsApi } from "@/lib/api";
import { useSettingsQuery } from "@/lib/query";
import { cn } from "@/lib/utils";
import {
  DEFAULT_OAUTH_QUOTA_REFRESH_INTERVAL_MINUTES,
  DEFAULT_OAUTH_QUOTA_REFRESH_TIMEOUT_SECONDS,
  getOauthQuotaRefreshIntervalMinutes,
  getOauthQuotaRefreshTimeoutSeconds,
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

export function AuthCenterPanel({ serverMode = false }: { serverMode?: boolean }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data: settings } = useSettingsQuery();
  const [centerOpen, setCenterOpen] = useState(false);
  const currentRefreshInterval = getOauthQuotaRefreshIntervalMinutes(settings);
  const currentRefreshTimeout = getOauthQuotaRefreshTimeoutSeconds(settings);
  const [refreshIntervalInput, setRefreshIntervalInput] = useState(
    String(DEFAULT_OAUTH_QUOTA_REFRESH_INTERVAL_MINUTES),
  );
  const [refreshTimeoutInput, setRefreshTimeoutInput] = useState(
    String(DEFAULT_OAUTH_QUOTA_REFRESH_TIMEOUT_SECONDS),
  );

  useEffect(() => {
    setRefreshIntervalInput(String(currentRefreshInterval));
  }, [currentRefreshInterval]);

  useEffect(() => {
    setRefreshTimeoutInput(String(currentRefreshTimeout));
  }, [currentRefreshTimeout]);

  const parsedRefreshIntervalValue = Number(refreshIntervalInput);
  const parsedRefreshInterval =
    Number.isFinite(parsedRefreshIntervalValue) &&
    Number.isInteger(parsedRefreshIntervalValue) &&
    parsedRefreshIntervalValue >= 1
      ? parsedRefreshIntervalValue
      : null;

  const parsedRefreshTimeoutValue = Number(refreshTimeoutInput);
  const parsedRefreshTimeout =
    Number.isFinite(parsedRefreshTimeoutValue) &&
    Number.isInteger(parsedRefreshTimeoutValue) &&
    parsedRefreshTimeoutValue >= 1 &&
    parsedRefreshTimeoutValue <= 120
      ? parsedRefreshTimeoutValue
      : null;

  const hasQuotaSettingChanges = useMemo(
    () =>
      parsedRefreshInterval !== currentRefreshInterval ||
      parsedRefreshTimeout !== currentRefreshTimeout,
    [
      currentRefreshInterval,
      currentRefreshTimeout,
      parsedRefreshInterval,
      parsedRefreshTimeout,
    ],
  );

  const centerSubtitle = useMemo(
    () =>
      t("settings.authCenter.quotaSettingsSummary", {
        interval: currentRefreshInterval,
        timeout: currentRefreshTimeout,
        defaultValue: "间隔 {{interval}} 分钟 · 超时 {{timeout}} 秒",
      }),
    [currentRefreshInterval, currentRefreshTimeout, t],
  );

  const invalidateQuotaQueries = async () => {
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
  };

  const handleSaveQuotaSettings = async () => {
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
    if (parsedRefreshTimeout == null) {
      toast.error(
        t("settings.authCenter.quotaRefreshTimeoutInvalid", {
          defaultValue: "刷新超时必须是 1-120 之间的整数秒",
        }),
      );
      setRefreshTimeoutInput(String(currentRefreshTimeout));
      return;
    }

    const { webdavSync: _, ...rest } = settings;
    await settingsApi.save({
      ...rest,
      oauthQuotaRefreshIntervalMinutes: parsedRefreshInterval,
      oauthQuotaRefreshTimeoutSeconds: parsedRefreshTimeout,
    });
    await invalidateQuotaQueries();
    toast.success(
      t("settings.authCenter.quotaSettingsSaved", {
        defaultValue: "OAuth 用量刷新设置已保存",
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
      <Collapsible open={centerOpen} onOpenChange={setCenterOpen}>
        <div className="rounded-xl border border-border bg-card/50 transition-colors hover:bg-muted/50">
          <CollapsibleTrigger asChild>
            <button
              type="button"
              className="flex w-full items-center justify-between gap-4 p-4 text-left hover:bg-muted/50"
            >
              <div className="flex min-w-0 items-center gap-3">
                <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-background ring-1 ring-border">
                  <ShieldCheck className="h-4 w-4 text-primary" />
                </div>
                <div className="min-w-0 space-y-1">
                  <p className="text-sm font-medium leading-none">
                    {t("settings.authCenter.title", {
                      defaultValue: "OAuth 认证中心",
                    })}
                  </p>
                  <p className="truncate text-xs text-muted-foreground">
                    {centerSubtitle}
                  </p>
                </div>
              </div>
              <ChevronDown
                className={cn(
                  "h-4 w-4 shrink-0 text-muted-foreground transition-transform duration-200",
                  centerOpen && "rotate-180",
                )}
              />
            </button>
          </CollapsibleTrigger>

          <CollapsibleContent>
            <div className="space-y-5 border-t border-border/50 px-4 pb-4 pt-4">
              <p className="text-sm text-muted-foreground">
                {serverMode
                  ? t("settings.authCenter.serverDescription", {
                      defaultValue:
                        "管理用于反代上游的官方 OAuth 账号。用量刷新仅作用于当前激活的供应商。",
                    })
                  : t("settings.authCenter.description", {
                      defaultValue:
                        "在 Claude Code 中使用您的其他订阅，请注意合规风险。",
                    })}
              </p>

              <div className="grid gap-4 md:grid-cols-2">
                <div className="space-y-2 rounded-lg border border-border/50 bg-background/60 p-4">
                  <Label htmlFor="oauth-quota-refresh-interval">
                    {t("settings.authCenter.quotaRefreshIntervalTitle", {
                      defaultValue: "用量刷新间隔",
                    })}
                  </Label>
                  <p className="text-sm text-muted-foreground">
                    {t("settings.authCenter.quotaRefreshIntervalDescription", {
                      defaultValue:
                        "控制 OAuth 账号 5h / 7day 用量进度条的自动刷新频率，仅当前激活供应商会自动轮询。",
                    })}
                  </p>
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
                  </div>
                </div>

                <div className="space-y-2 rounded-lg border border-border/50 bg-background/60 p-4">
                  <Label htmlFor="oauth-quota-refresh-timeout">
                    {t("settings.authCenter.quotaRefreshTimeoutTitle", {
                      defaultValue: "用量刷新超时",
                    })}
                  </Label>
                  <p className="text-sm text-muted-foreground">
                    {t("settings.authCenter.quotaRefreshTimeoutDescription", {
                      defaultValue:
                        "单次 OAuth 用量刷新请求的最大等待时间，超时后会按失败冷却策略重试。",
                    })}
                  </p>
                  <div className="flex items-center gap-2">
                    <Input
                      id="oauth-quota-refresh-timeout"
                      type="number"
                      min={1}
                      max={120}
                      step={1}
                      value={refreshTimeoutInput}
                      onChange={(event) =>
                        setRefreshTimeoutInput(event.currentTarget.value)
                      }
                      className="w-24"
                      disabled={!settings}
                    />
                    <span className="text-sm text-muted-foreground">
                      {t("settings.authCenter.quotaRefreshTimeoutSeconds", {
                        defaultValue: "秒",
                      })}
                    </span>
                  </div>
                </div>
              </div>

              <div className="flex justify-end border-t border-border/50 pt-4">
                <Button
                  type="button"
                  onClick={() => void handleSaveQuotaSettings()}
                  disabled={!settings || !hasQuotaSettingChanges}
                >
                  {t("common.save", { defaultValue: "保存" })}
                </Button>
              </div>
            </div>
          </CollapsibleContent>
        </div>
      </Collapsible>

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

        {!serverMode ? (
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
        ) : null}

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

        {!serverMode ? (
          <>
        <AuthProviderAccordionItem
          value="grok"
          icon={<ProviderIcon icon="grok" name="Grok" size={24} />}
          title="Grok OAuth"
          description={t("settings.authCenter.grokOauthDescription", {
            defaultValue: "管理 Grok/xAI 订阅账号",
          })}
        >
          <GrokOAuthSection showLoggedInAccounts />
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
          </>
        ) : null}

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

        {!serverMode ? (
          <>
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
          </>
        ) : null}
      </Accordion>
    </motion.div>
  );
}
