import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  ExternalLink,
  Github,
  ShieldCheck,
  Sparkles as SparklesIcon,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
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
    <div className="space-y-6">
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

      <section className="rounded-xl border border-border/60 bg-card/60 p-6">
        <div className="mb-4 flex items-center gap-3">
          <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-muted">
            <ClaudeIcon size={20} />
          </div>
          <div className="min-w-0 flex-1">
            <h4 className="font-medium">Claude Official</h4>
            <p className="text-sm text-muted-foreground">
              {t("settings.authCenter.claudeOauthDescription", {
                defaultValue: "管理 Claude 官方订阅账号",
              })}
            </p>
          </div>
          <a
            href="https://claude.ai"
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex items-center gap-1 text-sm text-blue-500 hover:underline"
          >
            {t("settings.authCenter.claudeLink", {
              defaultValue: "Claude 订阅链接",
            })}
            <ExternalLink className="h-3.5 w-3.5" />
          </a>
        </div>

        <ClaudeOAuthSection showLoggedInAccounts />
      </section>

      <section className="rounded-xl border border-border/60 bg-card/60 p-6">
        <div className="mb-4 flex items-center gap-3">
          <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-muted">
            <Github className="h-5 w-5" />
          </div>
          <div>
            <h4 className="font-medium">GitHub Copilot</h4>
            <p className="text-sm text-muted-foreground">
              {t("settings.authCenter.copilotDescription", {
                defaultValue: "管理 GitHub Copilot 账号",
              })}
            </p>
          </div>
        </div>

        <CopilotAuthSection showLoggedInAccounts />
      </section>

      <section className="rounded-xl border border-border/60 bg-card/60 p-6">
        <div className="mb-4 flex items-center gap-3">
          <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-muted">
            <CodexIcon size={20} />
          </div>
          <div>
            <h4 className="font-medium">OpenAI OAuth</h4>
            <p className="text-sm text-muted-foreground">
              {t("settings.authCenter.codexOauthDescription", {
                defaultValue: "管理 ChatGPT 账号",
              })}
            </p>
          </div>
        </div>

        <CodexOAuthSection showLoggedInAccounts />
      </section>

      <section className="rounded-xl border border-border/60 bg-card/60 p-6">
        <div className="mb-4 flex items-center gap-3">
          <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-muted">
            <ProviderIcon icon="kiro" name="Kiro" size={24} />
          </div>
          <div>
            <h4 className="font-medium">Kiro OAuth</h4>
            <p className="text-sm text-muted-foreground">
              {t("settings.authCenter.kiroOauthDescription", {
                defaultValue: "管理 Kiro AWS Builder ID 账号",
              })}
            </p>
          </div>
        </div>

        <KiroOAuthSection showLoggedInAccounts />
      </section>

      <section className="rounded-xl border border-border/60 bg-card/60 p-6">
        <div className="mb-4 flex items-center gap-3">
          <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-muted">
            <ProviderIcon icon="cursor" name="Cursor" size={24} />
          </div>
          <div>
            <h4 className="font-medium">Cursor OAuth</h4>
            <p className="text-sm text-muted-foreground">
              {t("settings.authCenter.cursorOauthDescription", {
                defaultValue: "管理 Cursor 订阅账号",
              })}
            </p>
          </div>
        </div>

        <CursorOAuthSection showLoggedInAccounts />
      </section>

      <section className="rounded-xl border border-border/60 bg-card/60 p-6">
        <div className="mb-4 flex items-center gap-3">
          <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-muted">
            <GeminiIcon size={20} />
          </div>
          <div>
            <h4 className="font-medium">Google Gemini</h4>
            <p className="text-sm text-muted-foreground">
              {t("settings.authCenter.geminiOauthDescription", {
                defaultValue: "管理 Google Gemini 账号",
              })}
            </p>
          </div>
        </div>

        <GeminiOAuthSection showLoggedInAccounts />
      </section>

      <section className="rounded-xl border border-border/60 bg-card/60 p-6">
        <div className="mb-4 flex items-center gap-3">
          <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-muted">
            <SparklesIcon />
          </div>
          <div>
            <h4 className="font-medium">Antigravity OAuth</h4>
            <p className="text-sm text-muted-foreground">
              {t("settings.authCenter.antigravityOauthDescription", {
                defaultValue: "管理 Antigravity 订阅账号",
              })}
            </p>
          </div>
        </div>

        <AntigravityOAuthSection showLoggedInAccounts />
      </section>

      <section className="rounded-xl border border-border/60 bg-card/60 p-6">
        <div className="mb-4 flex items-center gap-3">
          <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-muted">
            <DeepSeekIcon size={20} />
          </div>
          <div>
            <h4 className="font-medium">DeepSeek(Account)</h4>
            <p className="text-sm text-muted-foreground">
              {t("settings.authCenter.deepseekAccountDescription", {
                defaultValue: "管理 DeepSeek 账号",
              })}
            </p>
          </div>
        </div>

        <DeepSeekAccountSection showLoggedInAccounts />
      </section>
    </div>
  );
}
