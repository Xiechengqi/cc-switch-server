import React from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Loader2,
  LogOut,
  Copy,
  Check,
  ExternalLink,
  Plus,
  Sparkles,
  User,
  X,
  Image,
} from "lucide-react";
import { useCodexOauth } from "./hooks/useCodexOauth";
import { copyText } from "@/lib/clipboard";
import {
  ENABLE_CODEX_BANKED_RESET,
  ENABLE_CODEX_CLI_REMOTE_CALLBACK,
} from "@/config/constants";
import { isRemoteWebMode } from "@/lib/api/auth";
import CodexBankedResetPanel from "./CodexBankedResetPanel";
import { AccountSubscriptionExpiryControl } from "@/components/settings/AccountSubscriptionExpiryControl";

interface CodexOAuthSectionProps {
  className?: string;
  /** 当前选中的 ChatGPT 账号 ID */
  selectedAccountId?: string | null;
  /** 账号选择回调 */
  onAccountSelect?: (accountId: string | null) => void;
  /** 是否允许选择“使用默认账号” */
  allowDefaultAccountOption?: boolean;
  /** 是否显示已登录账号管理列表 */
  showLoggedInAccounts?: boolean;
  /** 是否开启 Codex FAST mode */
  fastModeEnabled?: boolean;
  /** FAST mode 切换回调 */
  onFastModeChange?: (enabled: boolean) => void;
  /** 是否启用 Codex OAuth 生成图片能力 */
  imageGenerationEnabled?: boolean;
  /** 生成图片能力切换回调 */
  onImageGenerationChange?: (enabled: boolean) => void;
  /** Responses image_generation tool strip policy */
  imageToolStripPolicy?: "never" | "on-error" | "always";
  /** Responses image_generation tool strip policy callback */
  onImageToolStripPolicyChange?: (
    policy: "never" | "on-error" | "always",
  ) => void;
  /** 是否启用 Codex Responses WebSocket，上游故障时可回退 SSE */
  websocketEnabled?: boolean;
  /** WebSocket 切换回调 */
  onWebsocketChange?: (enabled: boolean) => void;
  /** 是否显示只读 Banked Reset 次数与到期明细面板 */
  showBankedResetPanel?: boolean;
}

/**
 * Codex OAuth 认证区块
 *
 * 通过 OpenAI Device Code 流程登录 ChatGPT Plus/Pro 账号，
 * 用于将 Claude Code 请求反代到 Codex 后端 API。
 */
export const CodexOAuthSection: React.FC<CodexOAuthSectionProps> = ({
  className,
  selectedAccountId,
  onAccountSelect,
  allowDefaultAccountOption = true,
  showLoggedInAccounts = false,
  fastModeEnabled = false,
  onFastModeChange,
  imageGenerationEnabled = false,
  onImageGenerationChange,
  imageToolStripPolicy = "never",
  onImageToolStripPolicyChange,
  websocketEnabled = true,
  onWebsocketChange,
  showBankedResetPanel = false,
}) => {
  const { t } = useTranslation();
  const [copied, setCopied] = React.useState(false);
  const [linkCopied, setLinkCopied] = React.useState(false);

  const {
    accounts,
    hasAnyAccount,
    pollingState,
    deviceCode,
    error,
    isPolling,
    isAddingAccount,
    isRemovingAccount,
    isSettingDefaultAccount,
    isSettingWorkspace,
    defaultAccountId,
    cancelAuth,
    logout,
    removeAccount,
    setDefaultAccount,
    setWorkspace,
    addAccountWithMode,
  } = useCodexOauth();
  const isRemoteClientWeb = isRemoteWebMode();
  const isHttpsClientOrigin =
    typeof window !== "undefined" && window.location.protocol === "https:";
  const canUseRemoteCliCallback =
    isRemoteClientWeb &&
    ENABLE_CODEX_CLI_REMOTE_CALLBACK &&
    isHttpsClientOrigin;
  const codexCliCallbackUrl =
    canUseRemoteCliCallback && typeof window !== "undefined"
      ? `${window.location.origin}/web-api/oauth/openai-cli/callback`
      : null;
  const cliLoginTitle =
    isRemoteClientWeb && !canUseRemoteCliCallback
      ? t("codexOauth.cliUnavailableInRemoteWeb", {
          defaultValue:
            "远程 Web 模式下 localhost 回调不可达，请使用 Device Code",
        })
      : undefined;
  const startCliLogin = () =>
    addAccountWithMode?.("cli", { codexCallbackUrl: codexCliCallbackUrl });
  const activeAccount =
    accounts.find((account) => account.id === selectedAccountId) ??
    accounts.find((account) => account.id === defaultAccountId) ??
    accounts[0];
  const activeWorkspaceId =
    activeAccount?.selected_workspace_id ?? activeAccount?.workspaces?.[0]?.id;

  const copyUserCode = async () => {
    if (deviceCode?.user_code) {
      await copyText(deviceCode.user_code);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  };

  const copyVerificationUrl = async () => {
    if (!deviceCode?.verification_uri) return;
    await copyText(deviceCode.verification_uri);
    setLinkCopied(true);
    setTimeout(() => setLinkCopied(false), 2000);
  };

  const handleAccountSelect = (value: string) => {
    onAccountSelect?.(value === "none" ? null : value);
  };

  const handleRemoveAccount = (accountId: string, e: React.MouseEvent) => {
    e.stopPropagation();
    e.preventDefault();
    removeAccount(accountId);
    if (selectedAccountId === accountId) {
      onAccountSelect?.(null);
    }
  };

  return (
    <div className={`space-y-4 ${className || ""}`}>
      {/* 认证状态标题 */}
      <div className="flex items-center justify-between">
        <Label>{t("codexOauth.authStatus", "认证状态")}</Label>
        <Badge
          variant={hasAnyAccount ? "default" : "secondary"}
          className={hasAnyAccount ? "bg-green-500 hover:bg-green-600" : ""}
        >
          {hasAnyAccount
            ? t("codexOauth.accountCount", {
                count: accounts.length,
                defaultValue: `${accounts.length} 个账号`,
              })
            : t("codexOauth.notAuthenticated", "未认证")}
        </Badge>
      </div>

      {onFastModeChange && (
        <div className="flex items-center justify-between rounded-md border bg-muted/30 p-3">
          <div className="space-y-1 pr-4">
            <Label className="text-sm font-medium">
              {t("codexOauth.fastMode", "FAST mode")}
            </Label>
            <p className="text-xs text-muted-foreground">
              {t("codexOauth.fastModeDescription", {
                defaultValue:
                  'Send service_tier="priority" for lower latency. Turn it off if the ChatGPT Codex backend rejects the parameter.',
              })}
            </p>
          </div>
          <Switch
            checked={fastModeEnabled}
            onCheckedChange={onFastModeChange}
            aria-label={t("codexOauth.fastMode", "FAST mode")}
          />
        </div>
      )}

      {onImageGenerationChange && (
        <div className="flex items-center justify-between rounded-md border bg-muted/30 p-3">
          <div className="space-y-1 pr-4">
            <div className="flex items-center gap-2">
              <Image className="h-4 w-4 text-muted-foreground" />
              <Label className="text-sm font-medium">
                {t("codexOauth.imageGeneration", "生成图片")}
              </Label>
            </div>
            <p className="text-xs text-muted-foreground">
              {t("codexOauth.imageGenerationDescription", {
                defaultValue:
                  "Enable OpenAI-compatible /v1/images/generations through the ChatGPT Codex backend.",
              })}
            </p>
          </div>
          <Switch
            checked={imageGenerationEnabled}
            onCheckedChange={onImageGenerationChange}
            aria-label={t("codexOauth.imageGeneration", "生成图片")}
          />
        </div>
      )}

      {onImageToolStripPolicyChange && (
        <div className="flex items-center justify-between gap-4 rounded-md border bg-muted/30 p-3">
          <div className="space-y-1 pr-4">
            <Label className="text-sm font-medium">
              {t("codexOauth.imageToolStripPolicy", "Image tool policy")}
            </Label>
            <p className="text-xs text-muted-foreground">
              {t("codexOauth.imageToolStripPolicyDescription", {
                defaultValue:
                  "Control whether Codex Responses requests may declare image_generation tools.",
              })}
            </p>
          </div>
          <Select
            value={imageToolStripPolicy}
            onValueChange={(value) =>
              onImageToolStripPolicyChange(
                value as "never" | "on-error" | "always",
              )
            }
          >
            <SelectTrigger className="w-[150px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="never">
                {t("codexOauth.imageToolStripNever", "Never strip")}
              </SelectItem>
              <SelectItem value="on-error">
                {t("codexOauth.imageToolStripOnError", "On error")}
              </SelectItem>
              <SelectItem value="always">
                {t("codexOauth.imageToolStripAlways", "Always strip")}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>
      )}

      {onWebsocketChange && (
        <div className="flex items-center justify-between rounded-md border bg-muted/30 p-3">
          <div className="space-y-1 pr-4">
            <Label className="text-sm font-medium">
              {t("codexOauth.websocket", "Responses WebSocket")}
            </Label>
            <p className="text-xs text-muted-foreground">
              {t("codexOauth.websocketDescription", {
                defaultValue:
                  "Keep enabled normally. Disable it to force clients onto POST /v1/responses SSE during an upstream WebSocket incident.",
              })}
            </p>
          </div>
          <Switch
            checked={websocketEnabled}
            onCheckedChange={onWebsocketChange}
            aria-label={t("codexOauth.websocket", "Responses WebSocket")}
          />
        </div>
      )}

      {/* 已登录账号列表 */}
      {hasAnyAccount && showLoggedInAccounts && (
        <div className="space-y-2">
          <Label className="text-sm text-muted-foreground">
            {t("codexOauth.loggedInAccounts", "已登录账号")}
          </Label>
          <div className="space-y-1">
            {accounts.map((account) => (
              <div
                key={account.id}
                className="flex flex-wrap items-center justify-between rounded-md border bg-muted/30 p-2"
              >
                <div className="flex min-w-0 items-center gap-2">
                  <User className="h-5 w-5 shrink-0 text-muted-foreground" />
                  <span className="truncate text-sm font-medium">
                    {account.login}
                  </span>
                  {defaultAccountId === account.id && (
                    <Badge variant="secondary" className="text-xs">
                      {t("codexOauth.defaultAccount", "默认")}
                    </Badge>
                  )}
                  {selectedAccountId === account.id && (
                    <Badge variant="outline" className="text-xs">
                      {t("codexOauth.selected", "已选中")}
                    </Badge>
                  )}
                </div>
                <div className="flex shrink-0 items-center gap-1">
                  {defaultAccountId !== account.id && (
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      className="h-7 px-2 text-xs text-muted-foreground"
                      onClick={() => setDefaultAccount(account.id)}
                      disabled={isSettingDefaultAccount}
                    >
                      {t("codexOauth.setAsDefault", "设为默认")}
                    </Button>
                  )}
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="h-7 w-7 text-muted-foreground hover:text-red-500"
                    onClick={(e) => handleRemoveAccount(account.id, e)}
                    disabled={isRemovingAccount}
                    title={t("codexOauth.removeAccount", "移除账号")}
                  >
                    <X className="h-4 w-4" />
                  </Button>
                </div>
                <AccountSubscriptionExpiryControl account={account} />
              </div>
            ))}
          </div>
        </div>
      )}

      {/* 账号选择器 */}
      {hasAnyAccount && onAccountSelect && (
        <div className="space-y-2">
          <Label className="text-sm text-muted-foreground">
            {t("codexOauth.selectAccount", "选择账号")}
          </Label>
          <Select
            value={
              selectedAccountId ??
              (allowDefaultAccountOption ? "none" : undefined)
            }
            onValueChange={handleAccountSelect}
          >
            <SelectTrigger>
              <SelectValue
                placeholder={t(
                  "codexOauth.selectAccountPlaceholder",
                  "选择一个 ChatGPT 账号",
                )}
              />
            </SelectTrigger>
            <SelectContent>
              {allowDefaultAccountOption && (
                <SelectItem value="none">
                  <span className="text-muted-foreground">
                    {t("codexOauth.useDefaultAccount", "使用默认账号")}
                  </span>
                </SelectItem>
              )}
              {accounts.map((account) => (
                <SelectItem key={account.id} value={account.id}>
                  <div className="flex items-center gap-2">
                    <User className="h-4 w-4 text-muted-foreground" />
                    <span>{account.login}</span>
                  </div>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      )}

      {activeAccount?.workspaces && activeAccount.workspaces.length > 1 && (
        <div className="space-y-2">
          <Label className="text-sm text-muted-foreground">
            {t("codexOauth.workspace", "ChatGPT Workspace")}
          </Label>
          <Select
            value={activeWorkspaceId}
            onValueChange={(workspaceId) =>
              setWorkspace(activeAccount.id, workspaceId)
            }
            disabled={isSettingWorkspace}
          >
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {activeAccount.workspaces.map((workspace) => (
                <SelectItem key={workspace.id} value={workspace.id}>
                  {workspace.name === workspace.id
                    ? workspace.id
                    : `${workspace.name} (${workspace.id})`}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <p className="text-xs text-muted-foreground">
            {t("codexOauth.workspaceDescription", {
              defaultValue:
                "Only workspaces present in the verified OpenAI token claims can be selected.",
            })}
          </p>
        </div>
      )}

      {ENABLE_CODEX_BANKED_RESET && showBankedResetPanel && hasAnyAccount && (
        <CodexBankedResetPanel
          accountId={activeAccount?.id}
          workspaceId={activeWorkspaceId}
        />
      )}

      {/* 未认证 - 登录按钮 */}
      {!hasAnyAccount && pollingState === "idle" && (
        <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
          <Button
            type="button"
            onClick={startCliLogin}
            className="w-full"
            variant="outline"
            disabled={
              (isRemoteClientWeb && !canUseRemoteCliCallback) || isAddingAccount
            }
            title={cliLoginTitle}
          >
            <Sparkles className="mr-2 h-4 w-4" />
            {t("codexOauth.loginWithCli", "CLI OAuth")}
          </Button>
          <Button
            type="button"
            onClick={() => addAccountWithMode?.("device")}
            className="w-full"
            variant="outline"
            disabled={isAddingAccount}
          >
            <ExternalLink className="mr-2 h-4 w-4" />
            {t("codexOauth.loginWithDevice", "Device Code")}
          </Button>
        </div>
      )}

      {/* 已有账号 - 添加更多按钮 */}
      {hasAnyAccount && pollingState === "idle" && (
        <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
          <Button
            type="button"
            onClick={startCliLogin}
            variant="outline"
            disabled={
              isAddingAccount || (isRemoteClientWeb && !canUseRemoteCliCallback)
            }
            title={cliLoginTitle}
          >
            <Plus className="mr-2 h-4 w-4" />
            {t("codexOauth.addCliAccount", "添加 CLI OAuth")}
          </Button>
          <Button
            type="button"
            onClick={() => addAccountWithMode?.("device")}
            variant="outline"
            disabled={isAddingAccount}
          >
            <Plus className="mr-2 h-4 w-4" />
            {t("codexOauth.addDeviceAccount", "添加 Device Code")}
          </Button>
        </div>
      )}

      {/* 轮询中状态 */}
      {isPolling && deviceCode && (
        <div className="space-y-3 p-4 rounded-lg border border-border bg-muted/50">
          <div className="flex items-center justify-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
            {t(
              "codexOauth.waitingForAuth",
              "请手动打开下方授权链接并完成登录...",
            )}
          </div>

          {deviceCode.user_code ? (
            <div className="text-center">
              <p className="text-xs text-muted-foreground mb-1">
                {t("codexOauth.enterCode", "在浏览器中输入以下代码：")}
              </p>
              <div className="flex items-center justify-center gap-2">
                <code className="text-2xl font-mono font-bold tracking-wider bg-background px-4 py-2 rounded border">
                  {deviceCode.user_code}
                </code>
                <Button
                  type="button"
                  size="icon"
                  variant="ghost"
                  onClick={copyUserCode}
                  title={t("codexOauth.copyCode", "复制代码")}
                >
                  {copied ? (
                    <Check className="h-4 w-4 text-green-500" />
                  ) : (
                    <Copy className="h-4 w-4" />
                  )}
                </Button>
              </div>
            </div>
          ) : null}

          <div className="rounded-md border bg-background/80 p-3">
            <p className="mb-2 text-xs text-muted-foreground">
              {t(
                "codexOauth.openLinkHint",
                "授权链接不会自动打开，请点击或复制后在浏览器中访问：",
              )}
            </p>
            <div className="flex items-center gap-2">
              <a
                href={deviceCode.verification_uri}
                target="_blank"
                rel="noopener noreferrer"
                className="min-w-0 flex-1 truncate text-sm text-blue-500 hover:underline"
                title={deviceCode.verification_uri}
              >
                {deviceCode.verification_uri}
              </a>
              <Button
                type="button"
                size="icon"
                variant="ghost"
                onClick={copyVerificationUrl}
                title={t("codexOauth.copyLink", "复制链接")}
              >
                {linkCopied ? (
                  <Check className="h-4 w-4 text-green-500" />
                ) : (
                  <Copy className="h-4 w-4" />
                )}
              </Button>
              <a
                href={deviceCode.verification_uri}
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex"
              >
                <Button type="button" variant="outline" size="sm">
                  {t("codexOauth.openManually", "打开链接")}
                  <ExternalLink className="ml-1 h-3 w-3" />
                </Button>
              </a>
            </div>
          </div>

          <div className="text-center">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={cancelAuth}
            >
              {t("common.cancel", "取消")}
            </Button>
          </div>
        </div>
      )}

      {/* 错误状态 */}
      {pollingState === "error" && error && (
        <div className="space-y-2">
          <p className="text-sm text-red-500">{error}</p>
          <div className="flex gap-2">
            <Button
              type="button"
              onClick={() =>
                isRemoteClientWeb && !canUseRemoteCliCallback
                  ? addAccountWithMode?.("device")
                  : startCliLogin()
              }
              variant="outline"
              size="sm"
            >
              {t("codexOauth.retry", "重试")}
            </Button>
            <Button
              type="button"
              onClick={cancelAuth}
              variant="ghost"
              size="sm"
            >
              {t("common.cancel", "取消")}
            </Button>
          </div>
        </div>
      )}

      {/* 注销所有账号 */}
      {hasAnyAccount && accounts.length > 1 && (
        <Button
          type="button"
          variant="outline"
          onClick={logout}
          className="w-full text-red-500 hover:text-red-600 hover:bg-red-50 dark:hover:bg-red-950"
        >
          <LogOut className="mr-2 h-4 w-4" />
          {t("codexOauth.logoutAll", "注销所有账号")}
        </Button>
      )}
    </div>
  );
};

export default CodexOAuthSection;
