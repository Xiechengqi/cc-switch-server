import React from "react";
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
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { copyText } from "@/lib/clipboard";
import type { CodeBuddySite, ManagedAuthProvider } from "@/lib/api";
import {
  logoutAccountsAndClearSelection,
  removeAccountAndUpdateSelection,
} from "./accountSelectionActions";
import { ManagedAuthStatusNotice } from "./ManagedAuthStatusNotice";
import { useManagedAuth } from "./hooks/useManagedAuth";

interface KimiOAuthSectionProps {
  className?: string;
  selectedAccountId?: string | null;
  onAccountSelect?: (accountId: string | null) => void;
  showLoggedInAccounts?: boolean;
  allowDefaultAccountOption?: boolean;
}

interface DeviceCodeManagedAccountSectionProps extends KimiOAuthSectionProps {
  authProvider: ManagedAuthProvider;
  authStatusLabel: string;
  loginLabel: string;
  userCodeHint?: string;
  loginOptions?: {
    codeBuddySite?: CodeBuddySite | null;
    accountId?: string | null;
  };
  manualCallback?: {
    label: string;
    hint: string;
    placeholder: string;
    submitLabel: string;
    requiredMessage: string;
    successMessage: string;
  };
}

export const DeviceCodeManagedAccountSection: React.FC<
  DeviceCodeManagedAccountSectionProps
> = ({
  authProvider,
  authStatusLabel,
  loginLabel,
  userCodeHint,
  loginOptions,
  manualCallback,
  className,
  selectedAccountId,
  onAccountSelect,
  showLoggedInAccounts = false,
  allowDefaultAccountOption = false,
}) => {
  const { t } = useTranslation();
  const [copiedCode, setCopiedCode] = React.useState(false);
  const [copiedLink, setCopiedLink] = React.useState(false);
  const [callbackUrl, setCallbackUrl] = React.useState("");
  const {
    accounts,
    hasAnyAccount,
    isLoadingStatus,
    isFetchingStatus,
    isStatusError,
    deviceCode,
    error,
    isPolling,
    isAddingAccount,
    isRemovingAccount,
    isSettingDefaultAccount,
    isSubmittingOauthCallback,
    defaultAccountId,
    addAccountWithMode,
    cancelAuth,
    logoutAsync,
    removeAccountAsync,
    setDefaultAccount,
    submitOauthCallback,
    refetchStatus,
  } = useManagedAuth(authProvider);

  React.useEffect(() => {
    if (!selectedAccountId && accounts.length === 1) {
      onAccountSelect?.(accounts[0].id);
    }
  }, [accounts, onAccountSelect, selectedAccountId]);

  React.useEffect(() => {
    setCallbackUrl("");
  }, [deviceCode?.device_code]);

  const accountLabel = (account: (typeof accounts)[number]) =>
    account.email || account.login;

  const copyUserCode = async () => {
    if (!deviceCode?.user_code) return;
    await copyText(deviceCode.user_code);
    setCopiedCode(true);
    setTimeout(() => setCopiedCode(false), 2_000);
  };

  const copyVerificationUrl = async () => {
    const verificationUrl =
      deviceCode?.verification_uri_complete || deviceCode?.verification_uri;
    if (!verificationUrl) return;
    await copyText(verificationUrl);
    setCopiedLink(true);
    setTimeout(() => setCopiedLink(false), 2_000);
  };

  const startLogin = () => {
    addAccountWithMode(undefined, loginOptions);
  };

  const submitManualCallback = async () => {
    if (!manualCallback) return;
    const value = callbackUrl.trim();
    if (!value) {
      toast.error(manualCallback.requiredMessage);
      return;
    }
    try {
      const account = await submitOauthCallback(value);
      setCallbackUrl("");
      onAccountSelect?.(account.id);
      toast.success(manualCallback.successMessage);
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause));
    }
  };

  const handleRemoveAccount = async (
    accountId: string,
    event: React.MouseEvent,
  ) => {
    event.preventDefault();
    event.stopPropagation();
    try {
      await removeAccountAndUpdateSelection({
        accountId,
        selectedAccountId,
        removeAccount: removeAccountAsync,
        onAccountSelect,
      });
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause));
    }
  };

  const handleLogout = async () => {
    try {
      await logoutAccountsAndClearSelection({
        logout: logoutAsync,
        onAccountSelect,
      });
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause));
    }
  };

  if (isLoadingStatus || isStatusError) {
    return (
      <ManagedAuthStatusNotice
        className={className}
        title={authStatusLabel}
        error={error}
        isError={isStatusError}
        isFetching={isFetchingStatus}
        onRetry={() => void refetchStatus()}
      />
    );
  }

  return (
    <div className={`space-y-4 ${className || ""}`}>
      <div className="flex items-center justify-between gap-3">
        <Label>{authStatusLabel}</Label>
        <Badge
          variant={hasAnyAccount ? "default" : "secondary"}
          className={hasAnyAccount ? "bg-green-500 hover:bg-green-600" : ""}
        >
          {hasAnyAccount
            ? t("accountAuth.accountCount", { count: accounts.length })
            : t("accountAuth.notAuthenticated")}
        </Badge>
      </div>

      {hasAnyAccount && showLoggedInAccounts ? (
        <div className="space-y-2">
          <Label className="text-sm text-muted-foreground">
            {t("accountAuth.loggedInAccounts")}
          </Label>
          <div className="space-y-1">
            {accounts.map((account) => (
              <div
                key={account.id}
                className="flex flex-wrap items-center justify-between gap-2 rounded-md border bg-muted/30 p-2"
              >
                <div className="flex min-w-0 items-center gap-2">
                  <User className="h-5 w-5 shrink-0 text-muted-foreground" />
                  <span className="truncate text-sm font-medium">
                    {accountLabel(account)}
                  </span>
                  {defaultAccountId === account.id ? (
                    <Badge variant="secondary" className="text-xs">
                      {t("accountAuth.defaultAccount")}
                    </Badge>
                  ) : null}
                  {selectedAccountId === account.id ? (
                    <Badge variant="outline" className="text-xs">
                      {t("accountAuth.selected")}
                    </Badge>
                  ) : null}
                </div>
                <div className="flex shrink-0 items-center gap-1">
                  {defaultAccountId !== account.id ? (
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      className="h-7 px-2 text-xs text-muted-foreground"
                      onClick={() => setDefaultAccount(account.id)}
                      disabled={isSettingDefaultAccount}
                    >
                      {t("accountAuth.setAsDefault")}
                    </Button>
                  ) : null}
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="h-7 w-7 text-muted-foreground hover:text-red-500"
                    onClick={(event) =>
                      void handleRemoveAccount(account.id, event)
                    }
                    disabled={isRemovingAccount}
                    title={t("accountAuth.removeAccount")}
                  >
                    <X className="h-4 w-4" />
                  </Button>
                </div>
              </div>
            ))}
          </div>
        </div>
      ) : null}

      {hasAnyAccount && onAccountSelect ? (
        <div className="space-y-2">
          <Label className="text-sm text-muted-foreground">
            {t("accountAuth.selectAccount")}
          </Label>
          <Select
            value={
              selectedAccountId ??
              (allowDefaultAccountOption ? "none" : undefined)
            }
            onValueChange={(value) =>
              onAccountSelect(value === "none" ? null : value)
            }
          >
            <SelectTrigger>
              <SelectValue
                placeholder={t("accountAuth.selectAccountPlaceholder")}
              />
            </SelectTrigger>
            <SelectContent>
              {allowDefaultAccountOption ? (
                <SelectItem value="none">
                  {t("accountAuth.useDefaultAccount")}
                </SelectItem>
              ) : null}
              {accounts.map((account) => (
                <SelectItem key={account.id} value={account.id}>
                  {accountLabel(account)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      ) : null}

      {!isPolling ? (
        <Button
          type="button"
          variant="outline"
          className="w-full"
          onClick={startLogin}
          disabled={isAddingAccount}
        >
          {isAddingAccount ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          ) : hasAnyAccount ? (
            <Plus className="mr-2 h-4 w-4" />
          ) : (
            <Sparkles className="mr-2 h-4 w-4" />
          )}
          {hasAnyAccount ? t("accountAuth.addAnotherAccount") : loginLabel}
        </Button>
      ) : null}

      {isPolling && deviceCode ? (
        <div
          className="space-y-3 rounded-md border border-border bg-muted/50 p-4"
          aria-live="polite"
        >
          <div className="flex items-center justify-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
            {t("accountAuth.waitingForBrowser")}
          </div>
          {deviceCode.user_code ? (
            <div className="text-center">
              {userCodeHint ? (
                <p className="mb-1 text-xs text-muted-foreground">
                  {userCodeHint}
                </p>
              ) : null}
              <div className="flex items-center justify-center gap-2">
                <code className="rounded border bg-background px-4 py-2 font-mono text-xl font-bold">
                  {deviceCode.user_code}
                </code>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  onClick={() => void copyUserCode()}
                  title={t("common.copy")}
                >
                  {copiedCode ? (
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
              {t("accountAuth.openLinkHint")}
            </p>
            <div className="flex min-w-0 items-center gap-2">
              <a
                href={
                  deviceCode.verification_uri_complete ||
                  deviceCode.verification_uri
                }
                target="_blank"
                rel="noopener noreferrer"
                className="min-w-0 flex-1 truncate text-sm text-blue-500 hover:underline"
                title={
                  deviceCode.verification_uri_complete ||
                  deviceCode.verification_uri
                }
              >
                {deviceCode.verification_uri_complete ||
                  deviceCode.verification_uri}
              </a>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                onClick={() => void copyVerificationUrl()}
                title={t("accountAuth.copyLink")}
              >
                {copiedLink ? (
                  <Check className="h-4 w-4 text-green-500" />
                ) : (
                  <Copy className="h-4 w-4" />
                )}
              </Button>
              <Button asChild type="button" variant="outline" size="icon">
                <a
                  href={
                    deviceCode.verification_uri_complete ||
                    deviceCode.verification_uri
                  }
                  target="_blank"
                  rel="noopener noreferrer"
                  title={t("accountAuth.openManually")}
                >
                  <ExternalLink className="h-4 w-4" />
                </a>
              </Button>
            </div>
          </div>
          {manualCallback ? (
            <div className="space-y-2 rounded-md border bg-background/80 p-3">
              <Label htmlFor={`${authProvider}-callback-url`}>
                {manualCallback.label}
              </Label>
              <p className="text-xs text-muted-foreground">
                {manualCallback.hint}
              </p>
              <Textarea
                id={`${authProvider}-callback-url`}
                value={callbackUrl}
                onChange={(event) => setCallbackUrl(event.target.value)}
                placeholder={manualCallback.placeholder}
                rows={3}
                disabled={isSubmittingOauthCallback}
              />
              <Button
                type="button"
                className="w-full"
                onClick={() => void submitManualCallback()}
                disabled={
                  isSubmittingOauthCallback || callbackUrl.trim().length === 0
                }
              >
                {isSubmittingOauthCallback ? (
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                ) : null}
                {manualCallback.submitLabel}
              </Button>
            </div>
          ) : null}
          <Button
            type="button"
            variant="outline"
            className="w-full"
            onClick={cancelAuth}
          >
            {t("common.cancel")}
          </Button>
        </div>
      ) : null}

      {error ? <p className="text-sm text-destructive">{error}</p> : null}

      {hasAnyAccount ? (
        <Button
          type="button"
          variant="outline"
          className="w-full"
          onClick={() => void handleLogout()}
          disabled={isAddingAccount || isRemovingAccount}
        >
          <LogOut className="mr-2 h-4 w-4" />
          {t("accountAuth.logoutAll")}
        </Button>
      ) : null}
    </div>
  );
};

export const KimiOAuthSection: React.FC<KimiOAuthSectionProps> = (props) => {
  const { t } = useTranslation();
  return (
    <DeviceCodeManagedAccountSection
      {...props}
      authProvider="kimi_code"
      authStatusLabel={t("kimiOauth.authStatus")}
      loginLabel={t("kimiOauth.loginWithKimi")}
      userCodeHint={t("kimiOauth.userCodeHint")}
    />
  );
};

export default KimiOAuthSection;
