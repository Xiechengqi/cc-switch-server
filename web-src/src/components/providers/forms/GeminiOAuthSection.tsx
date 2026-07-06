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
  ExternalLink,
  Copy,
  Check,
  Plus,
  Sparkles,
  User,
  X,
  LogOut,
} from "lucide-react";
import { useGeminiOauth } from "./hooks/useGeminiOauth";
import { copyText } from "@/lib/clipboard";

interface GeminiOAuthSectionProps {
  className?: string;
  selectedAccountId?: string | null;
  onAccountSelect?: (accountId: string | null) => void;
  showLoggedInAccounts?: boolean;
  allowDefaultAccountOption?: boolean;
}

export const GeminiOAuthSection: React.FC<GeminiOAuthSectionProps> = ({
  className,
  selectedAccountId,
  onAccountSelect,
  showLoggedInAccounts = false,
  allowDefaultAccountOption = true,
}) => {
  const { t } = useTranslation();
  const [copied, setCopied] = React.useState(false);
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
    defaultAccountId,
    addAccount,
    cancelAuth,
    logout,
    removeAccount,
    setDefaultAccount,
  } = useGeminiOauth();

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

  const copyVerificationUrl = async () => {
    if (!deviceCode?.verification_uri) return;
    await copyText(deviceCode.verification_uri);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className={`space-y-4 ${className || ""}`}>
      <div className="flex items-center justify-between">
        <Label>
          {t("geminiOauth.authStatus", {
            defaultValue: "Google Gemini 认证",
          })}
        </Label>
        <Badge
          variant={hasAnyAccount ? "default" : "secondary"}
          className={hasAnyAccount ? "bg-green-500 hover:bg-green-600" : ""}
        >
          {hasAnyAccount
            ? t("geminiOauth.accountCount", {
                count: accounts.length,
                defaultValue: `${accounts.length} 个账号`,
              })
            : t("geminiOauth.notAuthenticated", {
                defaultValue: "未认证",
              })}
        </Badge>
      </div>

      {hasAnyAccount && showLoggedInAccounts && (
        <div className="space-y-2">
          <Label className="text-sm text-muted-foreground">
            {t("geminiOauth.loggedInAccounts", {
              defaultValue: "已登录账号",
            })}
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
                      {t("geminiOauth.defaultAccount", {
                        defaultValue: "默认",
                      })}
                    </Badge>
                  )}
                  {selectedAccountId === account.id && (
                    <Badge variant="outline" className="text-xs">
                      {t("geminiOauth.selected", {
                        defaultValue: "已选中",
                      })}
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
                      {t("geminiOauth.setAsDefault", {
                        defaultValue: "设为默认",
                      })}
                    </Button>
                  )}
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="h-7 w-7 text-muted-foreground hover:text-red-500"
                    onClick={(e) => handleRemoveAccount(account.id, e)}
                    disabled={isRemovingAccount}
                    title={t("geminiOauth.removeAccount", {
                      defaultValue: "移除账号",
                    })}
                  >
                    <X className="h-4 w-4" />
                  </Button>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {hasAnyAccount && onAccountSelect && (
        <div className="space-y-2">
          <Label className="text-sm text-muted-foreground">
            {t("geminiOauth.selectAccount", {
              defaultValue: "选择账号",
            })}
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
                placeholder={t("geminiOauth.selectAccountPlaceholder", {
                  defaultValue: "选择一个 Google 账号",
                })}
              />
            </SelectTrigger>
            <SelectContent>
              {allowDefaultAccountOption && (
                <SelectItem value="none">
                  <span className="text-muted-foreground">
                    {t("geminiOauth.useDefaultAccount", {
                      defaultValue: "使用默认账号",
                    })}
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

      {!hasAnyAccount && pollingState === "idle" && (
        <Button type="button" onClick={addAccount} className="w-full" variant="outline">
          <Sparkles className="mr-2 h-4 w-4" />
          {t("geminiOauth.loginWithGoogle", {
            defaultValue: "使用 Google 登录",
          })}
        </Button>
      )}

      {hasAnyAccount && pollingState === "idle" && (
        <Button
          type="button"
          onClick={addAccount}
          className="w-full"
          variant="outline"
          disabled={isAddingAccount}
        >
          <Plus className="mr-2 h-4 w-4" />
          {t("geminiOauth.addAnotherAccount", {
            defaultValue: "添加其他账号",
          })}
        </Button>
      )}

      {isPolling && deviceCode && (
        <div className="space-y-3 rounded-lg border border-border bg-muted/50 p-4">
          <div className="flex items-center justify-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
            {t("geminiOauth.waitingForBrowser", {
              defaultValue: "请手动打开下方授权链接并完成登录...",
            })}
          </div>
          <div className="rounded-md border bg-background/80 p-3">
            <p className="mb-2 text-xs text-muted-foreground">
              {t("geminiOauth.openLinkHint", {
                defaultValue:
                  "授权链接不会自动打开，请点击或复制后在浏览器中访问：",
              })}
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
              <Button type="button" variant="ghost" size="icon" onClick={copyVerificationUrl}>
                {copied ? <Check className="h-4 w-4 text-green-500" /> : <Copy className="h-4 w-4" />}
              </Button>
              <a
                href={deviceCode.verification_uri}
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex"
              >
                <Button type="button" variant="outline" size="sm">
                  {t("geminiOauth.openManually", { defaultValue: "打开链接" })}
                  <ExternalLink className="ml-1 h-3 w-3" />
                </Button>
              </a>
            </div>
          </div>
          <Button type="button" variant="outline" className="w-full" onClick={cancelAuth}>
            {t("common.cancel", { defaultValue: "取消" })}
          </Button>
        </div>
      )}

      {error && (
        <p className="text-sm text-destructive">
          {error}
        </p>
      )}

      {hasAnyAccount && (
        <Button type="button" variant="outline" className="w-full" onClick={logout}>
          <LogOut className="mr-2 h-4 w-4" />
          {t("geminiOauth.logoutAll", {
            defaultValue: "退出所有账号",
          })}
        </Button>
      )}
    </div>
  );
};

export default GeminiOAuthSection;
