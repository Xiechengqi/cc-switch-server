import React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  Check,
  Copy,
  ExternalLink,
  FileJson,
  Loader2,
  LogOut,
  Plus,
  Sparkles,
  User,
  X,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { AccountSubscriptionExpiryControl } from "@/components/settings/AccountSubscriptionExpiryControl";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { authApi, type ManagedAuthDeviceCodeResponse } from "@/lib/api";
import { copyText } from "@/lib/clipboard";
import { useManagedAuth } from "./hooks/useManagedAuth";

interface GrokOAuthSectionProps {
  className?: string;
  selectedAccountId?: string | null;
  onAccountSelect?: (accountId: string | null) => void;
  showLoggedInAccounts?: boolean;
  allowDefaultAccountOption?: boolean;
}

export const GrokOAuthSection: React.FC<GrokOAuthSectionProps> = ({
  className,
  selectedAccountId,
  onAccountSelect,
  showLoggedInAccounts = false,
  allowDefaultAccountOption = true,
}) => {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [copied, setCopied] = React.useState(false);
  const [showAuthJsonImport, setShowAuthJsonImport] = React.useState(false);
  const [authJsonInput, setAuthJsonInput] = React.useState("");
  const [isImporting, setIsImporting] = React.useState(false);
  const [loginRequest, setLoginRequest] =
    React.useState<ManagedAuthDeviceCodeResponse | null>(null);
  const [oauthCodeInput, setOauthCodeInput] = React.useState("");
  const [isStartingLogin, setIsStartingLogin] = React.useState(false);
  const [isSubmittingCode, setIsSubmittingCode] = React.useState(false);
  const [loginError, setLoginError] = React.useState<string | null>(null);
  const {
    accounts,
    hasAnyAccount,
    error,
    isRemovingAccount,
    isSettingDefaultAccount,
    defaultAccountId,
    logout,
    removeAccount,
    setDefaultAccount,
    refetchStatus,
  } = useManagedAuth("grok_oauth");

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
    if (!loginRequest?.verification_uri) return;
    await copyText(loginRequest.verification_uri);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const startLogin = async () => {
    setIsStartingLogin(true);
    setLoginError(null);
    try {
      const response = await authApi.authStartLogin(
        "grok_oauth",
        undefined,
        "web_paste",
      );
      setLoginRequest(response);
      setOauthCodeInput("");
    } catch (error) {
      setLoginError(error instanceof Error ? error.message : String(error));
    } finally {
      setIsStartingLogin(false);
    }
  };

  const submitOauthCode = async () => {
    if (!loginRequest) return;
    const code = oauthCodeInput.trim();
    if (!code) {
      toast.error(
        t("grokOauth.callbackRequired", {
          defaultValue: "请粘贴 Grok 回调 URL、query string 或 code",
        }),
      );
      return;
    }
    setIsSubmittingCode(true);
    setLoginError(null);
    try {
      const account = await authApi.authSubmitOauthCode(
        "grok_oauth",
        loginRequest.device_code,
        code,
      );
      await refetchStatus();
      await queryClient.invalidateQueries({
        queryKey: ["managed-auth-status", "grok_oauth"],
      });
      setLoginRequest(null);
      setOauthCodeInput("");
      onAccountSelect?.(account.id);
      toast.success(
        t("grokOauth.loginSuccess", {
          defaultValue: "Grok 账号已登录",
        }),
      );
    } catch (error) {
      setLoginError(error instanceof Error ? error.message : String(error));
    } finally {
      setIsSubmittingCode(false);
    }
  };

  const importAuthJson = async () => {
    const raw = authJsonInput.trim();
    if (!raw) {
      toast.error(
        t("grokOauth.authJsonRequired", {
          defaultValue: "请粘贴 ~/.grok/auth.json 内容",
        }),
      );
      return;
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch {
      toast.error(
        t("grokOauth.authJsonInvalid", {
          defaultValue: "auth.json 不是合法 JSON",
        }),
      );
      return;
    }

    setIsImporting(true);
    try {
      const response = await authApi.importGrokAuthJson(parsed);
      await refetchStatus();
      await queryClient.invalidateQueries({
        queryKey: ["managed-auth-status", "grok_oauth"],
      });
      setAuthJsonInput("");
      setShowAuthJsonImport(false);
      onAccountSelect?.(response.account.id);
      toast.success(
        t("grokOauth.importSuccess", {
          defaultValue: "Grok auth.json 已导入",
        }),
      );
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setIsImporting(false);
    }
  };

  return (
    <div className={`space-y-4 ${className || ""}`}>
      <div className="flex items-center justify-between">
        <Label>
          {t("grokOauth.authStatus", {
            defaultValue: "Grok OAuth 认证",
          })}
        </Label>
        <Badge
          variant={hasAnyAccount ? "default" : "secondary"}
          className={hasAnyAccount ? "bg-green-500 hover:bg-green-600" : ""}
        >
          {hasAnyAccount
            ? t("grokOauth.accountCount", {
                count: accounts.length,
                defaultValue: `${accounts.length} 个账号`,
              })
            : t("grokOauth.notAuthenticated", {
                defaultValue: "未认证",
              })}
        </Badge>
      </div>

      {hasAnyAccount && showLoggedInAccounts && (
        <div className="space-y-2">
          <Label className="text-sm text-muted-foreground">
            {t("grokOauth.loggedInAccounts", {
              defaultValue: "已登录账号",
            })}
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
                      {t("grokOauth.defaultAccount", {
                        defaultValue: "默认",
                      })}
                    </Badge>
                  )}
                  {selectedAccountId === account.id && (
                    <Badge variant="outline" className="text-xs">
                      {t("grokOauth.selected", {
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
                      {t("grokOauth.setAsDefault", {
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
                    title={t("grokOauth.removeAccount", {
                      defaultValue: "移除账号",
                    })}
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

      {hasAnyAccount && onAccountSelect && (
        <div className="space-y-2">
          <Label className="text-sm text-muted-foreground">
            {t("grokOauth.selectAccount", {
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
                placeholder={t("grokOauth.selectAccountPlaceholder", {
                  defaultValue: "选择一个 Grok 账号",
                })}
              />
            </SelectTrigger>
            <SelectContent>
              {allowDefaultAccountOption && (
                <SelectItem value="none">
                  <span className="text-muted-foreground">
                    {t("grokOauth.useDefaultAccount", {
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

      <div className="grid gap-3 sm:grid-cols-2">
        <Button
          type="button"
          onClick={() => void startLogin()}
          className="w-full"
          variant="outline"
          disabled={isStartingLogin}
        >
          {isStartingLogin ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          ) : hasAnyAccount ? (
            <Plus className="mr-2 h-4 w-4" />
          ) : (
            <Sparkles className="mr-2 h-4 w-4" />
          )}
          {hasAnyAccount
            ? t("grokOauth.addAnotherAccount", {
                defaultValue: "添加其他账号",
              })
            : t("grokOauth.loginWithGrok", {
                defaultValue: "使用 Grok 登录",
              })}
        </Button>
        <Button
          type="button"
          variant="outline"
          className="w-full"
          onClick={() => setShowAuthJsonImport((value) => !value)}
        >
          <FileJson className="mr-2 h-4 w-4" />
          {t("grokOauth.importAuthJson", {
            defaultValue: "导入 auth.json",
          })}
        </Button>
      </div>

      {showAuthJsonImport && (
        <div className="space-y-2">
          <Textarea
            value={authJsonInput}
            onChange={(event) => setAuthJsonInput(event.currentTarget.value)}
            className="min-h-32 font-mono text-xs"
            spellCheck={false}
          />
          <Button
            type="button"
            onClick={() => void importAuthJson()}
            disabled={isImporting}
            className="w-full"
          >
            {isImporting && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            {t("grokOauth.import", { defaultValue: "导入" })}
          </Button>
        </div>
      )}

      {loginRequest && (
        <div className="space-y-3 rounded-lg border border-border bg-muted/50 p-4">
          <div className="flex items-center justify-center gap-2 text-sm text-muted-foreground">
            {t("grokOauth.waitingForBrowser", {
              defaultValue: "请打开授权链接，完成后粘贴浏览器回调 URL 或 code",
            })}
          </div>
          <div className="rounded-md border bg-background/80 p-3">
            <div className="flex items-center gap-2">
              <a
                href={loginRequest.verification_uri}
                target="_blank"
                rel="noopener noreferrer"
                className="min-w-0 flex-1 truncate text-sm text-blue-500 hover:underline"
                title={loginRequest.verification_uri}
              >
                {loginRequest.verification_uri}
              </a>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                onClick={() => void copyVerificationUrl()}
              >
                {copied ? (
                  <Check className="h-4 w-4 text-green-500" />
                ) : (
                  <Copy className="h-4 w-4" />
                )}
              </Button>
              <a
                href={loginRequest.verification_uri}
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex"
              >
                <Button type="button" variant="outline" size="sm">
                  {t("grokOauth.openManually", { defaultValue: "打开链接" })}
                  <ExternalLink className="ml-1 h-3 w-3" />
                </Button>
              </a>
            </div>
          </div>
          <Textarea
            value={oauthCodeInput}
            onChange={(event) => setOauthCodeInput(event.currentTarget.value)}
            className="min-h-24 font-mono text-xs"
            placeholder={t("grokOauth.callbackPlaceholder", {
              defaultValue:
                "http://127.0.0.1:56121/callback?code=...&state=... 或只粘贴 code",
            })}
            spellCheck={false}
          />
          <Button
            type="button"
            className="w-full"
            onClick={() => void submitOauthCode()}
            disabled={isSubmittingCode}
          >
            {isSubmittingCode && (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            )}
            {t("grokOauth.finishLogin", { defaultValue: "完成登录" })}
          </Button>
          <Button
            type="button"
            variant="outline"
            className="w-full"
            onClick={() => {
              setLoginRequest(null);
              setOauthCodeInput("");
              setLoginError(null);
            }}
          >
            {t("common.cancel", { defaultValue: "取消" })}
          </Button>
        </div>
      )}

      {loginError && <p className="text-sm text-destructive">{loginError}</p>}
      {error && <p className="text-sm text-destructive">{error}</p>}

      {hasAnyAccount && (
        <Button
          type="button"
          variant="outline"
          className="w-full"
          onClick={logout}
        >
          <LogOut className="mr-2 h-4 w-4" />
          {t("grokOauth.logoutAll", {
            defaultValue: "退出所有账号",
          })}
        </Button>
      )}
    </div>
  );
};

export default GrokOAuthSection;
