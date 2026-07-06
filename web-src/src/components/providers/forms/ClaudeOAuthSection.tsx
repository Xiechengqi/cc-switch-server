import React from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Label } from "@/components/ui/label";
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
  ExternalLink,
  Copy,
  Check,
  Plus,
  Sparkles,
  User,
  X,
} from "lucide-react";
import { useClaudeOauth } from "./hooks/useClaudeOauth";
import { copyText } from "@/lib/clipboard";
import { Input } from "@/components/ui/input";
import type { ClaudeOAuthFlowMode } from "./hooks/useClaudeOauth";

interface ClaudeOAuthSectionProps {
  className?: string;
  /** 当前选中的 Claude 账号 ID */
  selectedAccountId?: string | null;
  /** 账号选择回调 */
  onAccountSelect?: (accountId: string | null) => void;
  /** 是否显示已登录账号管理列表 */
  showLoggedInAccounts?: boolean;
}

/**
 * Claude OAuth 认证区块
 *
 * 通过 Anthropic OAuth PKCE 浏览器流程登录 Claude 官方订阅账号，
 * 用于在本地代理模式下使用 Claude.ai 的官方订阅额度。
 */
export const ClaudeOAuthSection: React.FC<ClaudeOAuthSectionProps> = ({
  className,
  selectedAccountId,
  onAccountSelect,
  showLoggedInAccounts = false,
}) => {
  const { t } = useTranslation();
  const [copied, setCopied] = React.useState(false);

  const {
    accounts,
    hasAnyAccount,
    authState,
    deviceCode,
    error,
    isWaitingBrowser,
    isWaitingPaste,
    isSubmittingPaste,
    isAddingAccount,
    canUseLocalCallback,
    isRemovingAccount,
    isSettingDefaultAccount,
    defaultAccountId,
    addAccount,
    cancelAuth,
    submitPasteCode,
    logout,
    removeAccount,
    setDefaultAccount,
  } = useClaudeOauth();
  const [pasteCode, setPasteCode] = React.useState("");
  React.useEffect(() => {
    // Reset paste input when a new flow starts or when we leave the paste state.
    if (!isWaitingPaste) {
      setPasteCode("");
    }
  }, [isWaitingPaste]);

  const copyVerificationUrl = async () => {
    if (!deviceCode?.verification_uri) {
      return;
    }
    await copyText(deviceCode.verification_uri);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
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

  const startClaudeLogin = (flowMode: ClaudeOAuthFlowMode) => {
    addAccount(flowMode);
  };

  const renderLoginButtons = (mode: "login" | "add" | "retry") => {
    const showIcon =
      mode === "add" ? (
        <Plus className="mr-2 h-4 w-4" />
      ) : (
        <Sparkles className="mr-2 h-4 w-4" />
      );

    return (
      <div className="space-y-2">
        {canUseLocalCallback && (
          <Button
            type="button"
            onClick={() => startClaudeLogin("localhost")}
            className="w-full justify-start"
            variant="outline"
            disabled={isAddingAccount}
          >
            {showIcon}
            {t("claudeOauth.localCallbackLogin", "本地回调登录")}
          </Button>
        )}
        <Button
          type="button"
          onClick={() => startClaudeLogin("web_paste")}
          className="w-full justify-start"
          variant="outline"
          disabled={isAddingAccount}
        >
          <ExternalLink className="mr-2 h-4 w-4" />
          {t("claudeOauth.officialCallbackLogin", "官方链接登录")}
        </Button>
        <p className="text-xs text-muted-foreground">
          {canUseLocalCallback
            ? t(
                "claudeOauth.callbackModeHint",
                "本地回调会使用 127.0.0.1 自动完成授权；官方链接会在 platform.claude.com 显示授权码，复制后粘贴回这里完成登录。",
              )
            : t(
                "claudeOauth.remoteCallbackModeHint",
                "当前通过 web 入口访问，本地回调不可达，请使用官方链接回调并粘贴授权码完成登录。",
              )}
        </p>
      </div>
    );
  };

  return (
    <div className={`space-y-4 ${className || ""}`}>
      {/* 认证状态标题 */}
      <div className="flex items-center justify-between">
        <Label>{t("claudeOauth.authStatus", "Claude 订阅认证")}</Label>
        <Badge
          variant={hasAnyAccount ? "default" : "secondary"}
          className={hasAnyAccount ? "bg-green-500 hover:bg-green-600" : ""}
        >
          {hasAnyAccount
            ? t("claudeOauth.accountCount", {
                count: accounts.length,
                defaultValue: `${accounts.length} 个账号`,
              })
            : t("claudeOauth.notAuthenticated", "未认证")}
        </Badge>
      </div>

      {/* 已登录账号列表 */}
      {hasAnyAccount && showLoggedInAccounts && (
        <div className="space-y-2">
          <Label className="text-sm text-muted-foreground">
            {t("claudeOauth.loggedInAccounts", "已登录账号")}
          </Label>
          <div className="space-y-1">
            {accounts.map((account) => (
              <div
                key={account.id}
                className="flex items-center justify-between rounded-md border bg-muted/30 p-2"
              >
                <div className="flex min-w-0 items-center gap-2">
                  <User className="h-5 w-5 shrink-0 text-muted-foreground" />
                  <span className="truncate text-sm font-medium">
                    {account.login}
                  </span>
                  {defaultAccountId === account.id && (
                    <Badge variant="secondary" className="text-xs">
                      {t("claudeOauth.defaultAccount", "默认")}
                    </Badge>
                  )}
                  {selectedAccountId === account.id && (
                    <Badge variant="outline" className="text-xs">
                      {t("claudeOauth.selected", "已选中")}
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
                      {t("claudeOauth.setAsDefault", "设为默认")}
                    </Button>
                  )}
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="h-7 w-7 text-muted-foreground hover:text-red-500"
                    onClick={(e) => handleRemoveAccount(account.id, e)}
                    disabled={isRemovingAccount}
                    title={t("claudeOauth.removeAccount", "移除账号")}
                  >
                    <X className="h-4 w-4" />
                  </Button>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* 账号选择器 */}
      {hasAnyAccount && onAccountSelect && (
        <div className="space-y-2">
          <Label className="text-sm text-muted-foreground">
            {t("claudeOauth.selectAccount", "选择账号")}
          </Label>
          <Select
            value={selectedAccountId || "none"}
            onValueChange={handleAccountSelect}
          >
            <SelectTrigger>
              <SelectValue
                placeholder={t(
                  "claudeOauth.selectAccountPlaceholder",
                  "选择一个 Claude 账号",
                )}
              />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="none">
                <span className="text-muted-foreground">
                  {t("claudeOauth.useDefaultAccount", "使用默认账号")}
                </span>
              </SelectItem>
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

      {/* 未认证 - 登录按钮 */}
      {!hasAnyAccount && authState === "idle" && (
        <div className="space-y-2">
          <Label className="text-sm text-muted-foreground">
            {t("claudeOauth.loginWithClaude", "使用 Claude.ai 登录")}
          </Label>
          {renderLoginButtons("login")}
        </div>
      )}

      {/* 已有账号 - 添加更多按钮 */}
      {hasAnyAccount && authState === "idle" && (
        <div className="space-y-2">
          <Label className="text-sm text-muted-foreground">
            {t("claudeOauth.addAnotherAccount", "添加其他账号")}
          </Label>
          {renderLoginButtons("add")}
        </div>
      )}

      {/* 等待浏览器授权状态 */}
      {isWaitingBrowser && deviceCode && (
        <div className="space-y-3 p-4 rounded-lg border border-border bg-muted/50">
          <div className="flex items-center justify-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
            {t(
              "claudeOauth.waitingForBrowser",
              "请手动打开下方授权链接并完成登录...",
            )}
          </div>

          <div className="rounded-md border bg-background/80 p-3">
            <p className="mb-2 text-xs text-muted-foreground">
              {t(
                "claudeOauth.openLinkHint",
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
                title={t("claudeOauth.copyLink", "复制链接")}
              >
                {copied ? (
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
                  {t("claudeOauth.openManually", "打开链接")}
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

      {/* Web-paste 模式：用户从 platform.claude.com 复制 code 粘回。 */}
      {isWaitingPaste && deviceCode && (
        <div className="space-y-3 p-4 rounded-lg border border-border bg-muted/50">
          <div className="text-sm text-muted-foreground">
            {t(
              "claudeOauth.webPasteHint",
              "1. 点击下方链接在浏览器中完成 claude.ai 授权。\n2. 授权后会跳到 platform.claude.com 显示一段授权码。\n3. 复制该授权码并粘到下面的输入框，点击「提交」即可完成添加账号。",
            )}
          </div>

          <div className="rounded-md border bg-background/80 p-3 space-y-2">
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
                title={t("claudeOauth.copyLink", "复制链接")}
              >
                {copied ? (
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
                  {t("claudeOauth.openManually", "打开链接")}
                  <ExternalLink className="ml-1 h-3 w-3" />
                </Button>
              </a>
            </div>
          </div>

          <div className="space-y-2">
            <Label className="text-xs text-muted-foreground">
              {t(
                "claudeOauth.pasteCodeLabel",
                "从 platform.claude.com 复制的授权码",
              )}
            </Label>
            <div className="flex items-center gap-2">
              <Input
                value={pasteCode}
                onChange={(e) => setPasteCode(e.target.value)}
                placeholder={t(
                  "claudeOauth.pasteCodePlaceholder",
                  "粘贴授权码 …",
                )}
                spellCheck={false}
                autoComplete="off"
                disabled={isSubmittingPaste}
                onKeyDown={(e) => {
                  if (
                    e.key === "Enter" &&
                    pasteCode.trim() &&
                    !isSubmittingPaste
                  ) {
                    e.preventDefault();
                    submitPasteCode(pasteCode);
                  }
                }}
                className="font-mono"
              />
              <Button
                type="button"
                size="sm"
                onClick={() => submitPasteCode(pasteCode)}
                disabled={!pasteCode.trim() || isSubmittingPaste}
              >
                {isSubmittingPaste ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  t("claudeOauth.submitPasteCode", "提交")
                )}
              </Button>
            </div>
            {error && <p className="text-xs text-red-500">{error}</p>}
          </div>

          <div className="text-center">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={cancelAuth}
              disabled={isSubmittingPaste}
            >
              {t("common.cancel", "取消")}
            </Button>
          </div>
        </div>
      )}

      {/* 错误状态 */}
      {authState === "error" && error && (
        <div className="space-y-2">
          <p className="text-sm text-red-500">{error}</p>
          {renderLoginButtons("retry")}
          <div className="flex justify-end">
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
          {t("claudeOauth.logoutAll", "注销所有账号")}
        </Button>
      )}
    </div>
  );
};

export default ClaudeOAuthSection;
