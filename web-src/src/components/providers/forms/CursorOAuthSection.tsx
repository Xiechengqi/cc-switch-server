import React from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
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
  Download,
  ExternalLink,
  Loader2,
  Sparkles,
  User,
  X,
} from "lucide-react";
import { useCursorOauth } from "./hooks/useCursorOauth";
import { copyText } from "@/lib/clipboard";
import { removeAccountAndUpdateSelection } from "./accountSelectionActions";
import { ManagedAuthStatusNotice } from "./ManagedAuthStatusNotice";

interface CursorOAuthSectionProps {
  className?: string;
  selectedAccountId?: string | null;
  onAccountSelect?: (accountId: string | null) => void;
  showLoggedInAccounts?: boolean;
}

export function resolveCursorAccountSelection(
  accounts: ReadonlyArray<{ id: string }>,
  selectedAccountId?: string | null,
): string | null | undefined {
  if (selectedAccountId) {
    return accounts.some((account) => account.id === selectedAccountId)
      ? undefined
      : null;
  }
  return accounts.length === 1 ? accounts[0].id : undefined;
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
    authStatus,
    accounts,
    hasAnyAccount,
    isLoadingStatus,
    isFetchingStatus,
    isStatusError,
    pollingState,
    deviceCode,
    error,
    isPolling,
    isImportingCursorLocalAuth,
    isRemovingAccount,
    addAccount,
    cancelAuth,
    importCursorLocalAuth,
    removeAccountAsync,
    refetchStatus,
  } = useCursorOauth();

  React.useEffect(() => {
    if (!authStatus) return;
    const nextSelection = resolveCursorAccountSelection(
      accounts,
      selectedAccountId,
    );
    if (nextSelection !== undefined) {
      onAccountSelect?.(nextSelection);
    }
  }, [accounts, authStatus, onAccountSelect, selectedAccountId]);

  if (isLoadingStatus || isStatusError) {
    return (
      <ManagedAuthStatusNotice
        className={className}
        title={t("cursorOauth.authStatus", {
          defaultValue: "Cursor OAuth 认证",
        })}
        error={error}
        isError={isStatusError}
        isFetching={isFetchingStatus}
        onRetry={() => void refetchStatus()}
      />
    );
  }

  const accountDisplayName = (account: {
    email?: string | null;
    login: string;
  }) => account.email || account.login;

  const handleRemoveAccount = async (
    accountId: string,
    e: React.MouseEvent,
  ) => {
    e.stopPropagation();
    e.preventDefault();
    try {
      await removeAccountAndUpdateSelection({
        accountId,
        selectedAccountId,
        removeAccount: removeAccountAsync,
        onAccountSelect,
      });
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
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
                  {selectedAccountId === account.id && (
                    <Badge variant="outline" className="text-xs">
                      {t("cursorOauth.selected", {
                        defaultValue: "已选中",
                      })}
                    </Badge>
                  )}
                </div>
                <div className="flex shrink-0 items-center gap-1">
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
              defaultValue: "选择供应商绑定账号",
            })}
          </Label>
          <Select
            value={selectedAccountId ?? undefined}
            onValueChange={onAccountSelect}
          >
            <SelectTrigger>
              <SelectValue
                placeholder={t("cursorOauth.selectAccountPlaceholder", {
                  defaultValue: "选择一个 Cursor 账号",
                })}
              />
            </SelectTrigger>
            <SelectContent>
              {accounts.map((account) => (
                <SelectItem key={account.id} value={account.id}>
                  <span className="flex items-center gap-2">
                    <User className="h-4 w-4 text-muted-foreground" />
                    <span>{accountDisplayName(account)}</span>
                  </span>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      )}

      {pollingState === "idle" && (
        <div className="grid gap-2 sm:grid-cols-2">
          <Button
            type="button"
            onClick={addAccount}
            className="w-full"
            variant="outline"
          >
            <Sparkles className="mr-2 h-4 w-4" />
            {hasAnyAccount
              ? t("cursorOauth.addAnotherAccount", {
                  defaultValue: "添加其他账号",
                })
              : t("cursorOauth.loginWithCursor", {
                  defaultValue: "使用 Cursor 登录",
                })}
          </Button>
          <Button
            type="button"
            onClick={importCursorLocalAuth}
            className="w-full"
            variant="secondary"
            disabled={isImportingCursorLocalAuth}
            title={t("cursorOauth.importLocalServerTitle", {
              defaultValue:
                "从运行 cc-switch-server 的这台机器读取 Cursor IDE 或 cursor-agent 登录状态",
            })}
          >
            {isImportingCursorLocalAuth ? (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            ) : (
              <Download className="mr-2 h-4 w-4" />
            )}
            {t("cursorOauth.importLocalServer", {
              defaultValue: "从本机 Cursor 导入",
            })}
          </Button>
        </div>
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

      {error && (
        <div className="space-y-2">
          <p className="text-sm text-red-500">{error}</p>
          {pollingState === "error" && (
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
          )}
        </div>
      )}
    </div>
  );
};

export default CursorOAuthSection;
