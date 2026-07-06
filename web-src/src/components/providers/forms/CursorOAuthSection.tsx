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
  Check,
  Copy,
  ExternalLink,
  Loader2,
  LogOut,
  Plus,
  Sparkles,
  User,
  X,
} from "lucide-react";
import { useCursorOauth } from "./hooks/useCursorOauth";
import { copyText } from "@/lib/clipboard";

interface CursorOAuthSectionProps {
  className?: string;
  selectedAccountId?: string | null;
  onAccountSelect?: (accountId: string | null) => void;
  showLoggedInAccounts?: boolean;
}

export const CursorOAuthSection: React.FC<CursorOAuthSectionProps> = ({
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
  } = useCursorOauth();

  const handleAccountSelect = (value: string) => {
    onAccountSelect?.(value === "none" ? null : value);
  };

  const accountDisplayName = (account: {
    email?: string | null;
    login: string;
  }) =>
    account.email || account.login;

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
          {t("cursorOauth.authStatus", {
            defaultValue: "Cursor OAuth 认证",
          })}
        </Label>
        <Badge
          variant={hasAnyAccount ? "default" : "secondary"}
          className={hasAnyAccount ? "bg-green-500 hover:bg-green-600" : ""}
        >
          {hasAnyAccount
            ? t("cursorOauth.accountCount", {
                count: accounts.length,
                defaultValue: `${accounts.length} 个账号`,
              })
            : t("cursorOauth.notAuthenticated", {
                defaultValue: "未认证",
              })}
        </Badge>
      </div>

      {hasAnyAccount && showLoggedInAccounts && (
        <div className="space-y-2">
          <Label className="text-sm text-muted-foreground">
            {t("cursorOauth.loggedInAccounts", {
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
                    {accountDisplayName(account)}
                  </span>
                  {defaultAccountId === account.id && (
                    <Badge variant="secondary" className="text-xs">
                      {t("cursorOauth.defaultAccount", {
                        defaultValue: "默认",
                      })}
                    </Badge>
                  )}
                  {selectedAccountId === account.id && (
                    <Badge variant="outline" className="text-xs">
                      {t("cursorOauth.selected", {
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
                      {t("cursorOauth.setAsDefault", {
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
                    title={t("cursorOauth.removeAccount", {
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
            {t("cursorOauth.selectAccount", {
              defaultValue: "选择账号",
            })}
          </Label>
          <Select
            value={selectedAccountId || "none"}
            onValueChange={handleAccountSelect}
          >
            <SelectTrigger>
              <SelectValue
                placeholder={t("cursorOauth.selectAccountPlaceholder", {
                  defaultValue: "选择一个 Cursor 账号",
                })}
              />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="none">
                <span className="text-muted-foreground">
                  {t("cursorOauth.useDefaultAccount", {
                    defaultValue: "使用默认账号",
                  })}
                </span>
              </SelectItem>
              {accounts.map((account) => (
                <SelectItem key={account.id} value={account.id}>
                  <div className="flex items-center gap-2">
                    <User className="h-4 w-4 text-muted-foreground" />
                    <span>{accountDisplayName(account)}</span>
                  </div>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      )}

      {!hasAnyAccount && pollingState === "idle" && (
        <Button
          type="button"
          onClick={addAccount}
          className="w-full"
          variant="outline"
        >
          <Sparkles className="mr-2 h-4 w-4" />
          {t("cursorOauth.loginWithCursor", {
            defaultValue: "使用 Cursor 登录",
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
          {t("cursorOauth.addAnotherAccount", {
            defaultValue: "添加其他账号",
          })}
        </Button>
      )}

      {isPolling && deviceCode && (
        <div className="space-y-3 rounded-lg border border-border bg-muted/50 p-4">
          <div className="flex items-center justify-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
            {t("cursorOauth.waitingForBrowser", {
              defaultValue: "请手动打开下方授权链接并完成登录...",
            })}
          </div>
          <div className="rounded-md border bg-background/80 p-3">
            <p className="mb-2 text-xs text-muted-foreground">
              {t("cursorOauth.openLinkHint", {
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
              <Button
                type="button"
                size="icon"
                variant="ghost"
                onClick={copyVerificationUrl}
                title={t("cursorOauth.copyLink", {
                  defaultValue: "复制链接",
                })}
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
                  {t("cursorOauth.openManually", {
                    defaultValue: "打开链接",
                  })}
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
              {t("common.cancel", {
                defaultValue: "取消",
              })}
            </Button>
          </div>
        </div>
      )}

      {pollingState === "error" && error && (
        <div className="space-y-2">
          <p className="text-sm text-red-500">{error}</p>
          <div className="flex gap-2">
            <Button
              type="button"
              onClick={addAccount}
              variant="outline"
              size="sm"
            >
              {t("cursorOauth.retry", {
                defaultValue: "重试",
              })}
            </Button>
            <Button
              type="button"
              onClick={cancelAuth}
              variant="ghost"
              size="sm"
            >
              {t("common.cancel", {
                defaultValue: "取消",
              })}
            </Button>
          </div>
        </div>
      )}

      {hasAnyAccount && accounts.length > 1 && (
        <Button
          type="button"
          variant="outline"
          onClick={logout}
          className="w-full text-red-500 hover:bg-red-50 hover:text-red-600 dark:hover:bg-red-950"
        >
          <LogOut className="mr-2 h-4 w-4" />
          {t("cursorOauth.logoutAll", {
            defaultValue: "注销所有账号",
          })}
        </Button>
      )}
    </div>
  );
};

export default CursorOAuthSection;
