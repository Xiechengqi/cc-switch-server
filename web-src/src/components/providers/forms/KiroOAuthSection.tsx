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
import { useKiroOauth } from "./hooks/useKiroOauth";
import { copyText } from "@/lib/clipboard";

interface KiroOAuthSectionProps {
  className?: string;
  selectedAccountId?: string | null;
  onAccountSelect?: (accountId: string | null) => void;
  showLoggedInAccounts?: boolean;
}

export const KiroOAuthSection: React.FC<KiroOAuthSectionProps> = ({
  className,
  selectedAccountId,
  onAccountSelect,
  showLoggedInAccounts = false,
}) => {
  const { t } = useTranslation();
  const [copied, setCopied] = React.useState(false);
  const [copiedCode, setCopiedCode] = React.useState(false);
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
    migrationError,
    addAccount,
    cancelAuth,
    logout,
    removeAccount,
    setDefaultAccount,
  } = useKiroOauth();

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

  const copyUserCode = async () => {
    if (!deviceCode?.user_code) return;
    await copyText(deviceCode.user_code);
    setCopiedCode(true);
    setTimeout(() => setCopiedCode(false), 2000);
  };

  return (
    <div className={`space-y-4 ${className || ""}`}>
      <div className="flex items-center justify-between">
        <Label>
          {t("kiroOauth.authStatus", {
            defaultValue: "Kiro OAuth 认证",
          })}
        </Label>
        <Badge
          variant={hasAnyAccount ? "default" : "secondary"}
          className={hasAnyAccount ? "bg-green-500 hover:bg-green-600" : ""}
        >
          {hasAnyAccount
            ? t("kiroOauth.accountCount", {
                count: accounts.length,
                defaultValue: `${accounts.length} 个账号`,
              })
            : t("kiroOauth.notAuthenticated", {
                defaultValue: "未认证",
              })}
        </Badge>
      </div>

      {hasAnyAccount && showLoggedInAccounts && (
        <div className="space-y-2">
          <Label className="text-sm text-muted-foreground">
            {t("kiroOauth.loggedInAccounts", {
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
                      {t("kiroOauth.defaultAccount", {
                        defaultValue: "默认",
                      })}
                    </Badge>
                  )}
                  {selectedAccountId === account.id && (
                    <Badge variant="outline" className="text-xs">
                      {t("kiroOauth.selected", {
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
                      {t("kiroOauth.setAsDefault", {
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
                    title={t("kiroOauth.removeAccount", {
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
            {t("kiroOauth.selectAccount", {
              defaultValue: "选择账号",
            })}
          </Label>
          <Select
            value={selectedAccountId || "none"}
            onValueChange={handleAccountSelect}
          >
            <SelectTrigger>
              <SelectValue
                placeholder={t("kiroOauth.selectAccountPlaceholder", {
                  defaultValue: "选择一个 AWS Builder ID 账号",
                })}
              />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="none">
                <span className="text-muted-foreground">
                  {t("kiroOauth.useDefaultAccount", {
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

      {migrationError && (
        <div className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-800 dark:border-amber-900/60 dark:bg-amber-950/30 dark:text-amber-200">
          {migrationError}
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
          {t("kiroOauth.loginWithKiro", {
            defaultValue: "使用 AWS Builder ID 登录",
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
          {t("kiroOauth.addAnotherAccount", {
            defaultValue: "添加 AWS Builder ID 账号",
          })}
        </Button>
      )}

      {isPolling && deviceCode && (
        <div className="space-y-3 rounded-lg border border-border bg-muted/50 p-4">
          <div className="flex items-center justify-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
            {t("kiroOauth.waitingForBrowser", {
              defaultValue: "请手动打开下方授权链接并完成登录...",
            })}
          </div>
          {deviceCode.user_code && (
            <div className="rounded-md border bg-background/80 p-3">
              <p className="mb-2 text-xs text-muted-foreground">
                {t("kiroOauth.userCodeHint", {
                  defaultValue: "AWS Builder ID 设备验证码：",
                })}
              </p>
              <div className="flex items-center gap-2">
                <code className="flex-1 rounded bg-muted px-3 py-2 text-center text-lg font-semibold tracking-widest">
                  {deviceCode.user_code}
                </code>
                <Button
                  type="button"
                  size="icon"
                  variant="ghost"
                  onClick={copyUserCode}
                  title={t("kiroOauth.copyUserCode", {
                    defaultValue: "复制验证码",
                  })}
                >
                  {copiedCode ? (
                    <Check className="h-4 w-4 text-green-500" />
                  ) : (
                    <Copy className="h-4 w-4" />
                  )}
                </Button>
              </div>
            </div>
          )}
          <div className="rounded-md border bg-background/80 p-3">
            <p className="mb-2 text-xs text-muted-foreground">
              {t("kiroOauth.openLinkHint", {
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
                title={t("kiroOauth.copyLink", {
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
                  {t("kiroOauth.openManually", {
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
              {t("kiroOauth.retry", {
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
          {t("kiroOauth.logoutAll", {
            defaultValue: "注销所有账号",
          })}
        </Button>
      )}
    </div>
  );
};

export default KiroOAuthSection;
