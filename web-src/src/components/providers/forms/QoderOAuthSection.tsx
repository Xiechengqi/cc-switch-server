import React from "react";
import {
  Check,
  Copy,
  ExternalLink,
  KeyRound,
  Loader2,
  LogIn,
  LogOut,
  User,
  X,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { authApi, type QoderSite } from "@/lib/api";
import { copyText } from "@/lib/clipboard";
import {
  logoutAccountsAndClearSelection,
  removeAccountAndUpdateSelection,
} from "./accountSelectionActions";
import { ManagedAuthStatusNotice } from "./ManagedAuthStatusNotice";
import { useManagedAuth } from "./hooks/useManagedAuth";

interface QoderOAuthSectionProps {
  className?: string;
  selectedAccountId?: string | null;
  onAccountSelect?: (accountId: string | null) => void;
  showLoggedInAccounts?: boolean;
}

export const QoderOAuthSection: React.FC<QoderOAuthSectionProps> = ({
  className,
  selectedAccountId,
  onAccountSelect,
  showLoggedInAccounts = false,
}) => {
  const { t } = useTranslation();
  const [loginMode, setLoginMode] = React.useState<"device" | "pat">("device");
  const [site, setSite] = React.useState<QoderSite>("global");
  const [personalToken, setPersonalToken] = React.useState("");
  const [isImporting, setIsImporting] = React.useState(false);
  const [copiedLink, setCopiedLink] = React.useState(false);
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
    defaultAccountId,
    addAccountWithMode,
    cancelAuth,
    logoutAsync,
    removeAccountAsync,
    setDefaultAccount,
    invalidateAccountViews,
    refetchStatus,
  } = useManagedAuth("qoder_cosy");

  React.useEffect(() => {
    if (!selectedAccountId && accounts.length === 1) {
      onAccountSelect?.(accounts[0].id);
    }
  }, [accounts, onAccountSelect, selectedAccountId]);

  const accountLabel = (account: (typeof accounts)[number]) =>
    account.email || account.login;

  const siteLabel = (account: (typeof accounts)[number]) =>
    account.qoder?.site === "cn"
      ? t("qoderOauth.cnSite")
      : t("qoderOauth.globalSite");

  const railLabel = (account: (typeof accounts)[number]) => {
    switch (account.qoder?.credentialRail) {
      case "pat_job_token":
        return t("qoderOauth.patRail");
      case "cn_oauth":
        return t("qoderOauth.oauthRail");
      case "global_oauth":
      default:
        return t("qoderOauth.oauthRail");
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

  const startDeviceLogin = () => {
    addAccountWithMode("device", { qoderSite: site });
  };

  const importPat = async () => {
    const token = personalToken.trim();
    if (!token.startsWith("pt-") || token.length <= 3) {
      toast.error(t("qoderOauth.patInvalid"));
      return;
    }
    setIsImporting(true);
    try {
      const response = await authApi.importQoderPat(token);
      await invalidateAccountViews();
      onAccountSelect?.(response.account.id);
      setPersonalToken("");
      toast.success(t("qoderOauth.importSuccess"));
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setIsImporting(false);
    }
  };

  const verificationUrl =
    deviceCode?.verification_uri_complete || deviceCode?.verification_uri;

  const copyVerificationUrl = async () => {
    if (!verificationUrl) return;
    await copyText(verificationUrl);
    setCopiedLink(true);
    setTimeout(() => setCopiedLink(false), 2_000);
  };

  if (isLoadingStatus || isStatusError) {
    return (
      <ManagedAuthStatusNotice
        className={className}
        title={t("qoderOauth.authStatus")}
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
        <Label>{t("qoderOauth.authStatus")}</Label>
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
                <div className="flex min-w-0 flex-wrap items-center gap-2">
                  <User className="h-5 w-5 shrink-0 text-muted-foreground" />
                  <span className="max-w-64 truncate text-sm font-medium">
                    {accountLabel(account)}
                  </span>
                  <Badge variant="outline" className="text-xs">
                    {siteLabel(account)}
                  </Badge>
                  <Badge variant="secondary" className="text-xs">
                    {railLabel(account)}
                  </Badge>
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
            value={selectedAccountId ?? undefined}
            onValueChange={onAccountSelect}
          >
            <SelectTrigger>
              <SelectValue
                placeholder={t("accountAuth.selectAccountPlaceholder")}
              />
            </SelectTrigger>
            <SelectContent>
              {accounts.map((account) => (
                <SelectItem key={account.id} value={account.id}>
                  {accountLabel(account)} · {siteLabel(account)} ·{" "}
                  {railLabel(account)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      ) : null}

      {!isPolling ? (
        <Tabs
          value={loginMode}
          onValueChange={(value) => setLoginMode(value as "device" | "pat")}
          className="space-y-3"
        >
          <TabsList className="grid w-full grid-cols-2">
            <TabsTrigger value="device" className="min-w-0">
              <LogIn className="mr-2 h-4 w-4" />
              {t("qoderOauth.deviceLogin")}
            </TabsTrigger>
            <TabsTrigger value="pat" className="min-w-0">
              <KeyRound className="mr-2 h-4 w-4" />
              {t("qoderOauth.patImport")}
            </TabsTrigger>
          </TabsList>

          <TabsContent value="device" className="space-y-3">
            <Tabs
              value={site}
              onValueChange={(value) => setSite(value as QoderSite)}
            >
              <TabsList className="grid w-full grid-cols-2">
                <TabsTrigger value="global" className="min-w-0">
                  {t("qoderOauth.globalSite")}
                </TabsTrigger>
                <TabsTrigger value="cn" className="min-w-0">
                  {t("qoderOauth.cnSite")}
                </TabsTrigger>
              </TabsList>
            </Tabs>
            <Button
              type="button"
              variant="outline"
              className="w-full"
              onClick={startDeviceLogin}
              disabled={isAddingAccount || isImporting}
            >
              {isAddingAccount ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : (
                <LogIn className="mr-2 h-4 w-4" />
              )}
              {hasAnyAccount
                ? t("accountAuth.addAnotherAccount")
                : t("qoderOauth.loginWithQoder")}
            </Button>
          </TabsContent>

          <TabsContent value="pat" className="space-y-3">
            <Label htmlFor="qoder-personal-token">
              {t("qoderOauth.patLabel")}
            </Label>
            <Input
              id="qoder-personal-token"
              type="password"
              autoComplete="off"
              value={personalToken}
              onChange={(event) => setPersonalToken(event.target.value)}
              placeholder={t("qoderOauth.patPlaceholder")}
              disabled={isImporting || isAddingAccount}
            />
            <Button
              type="button"
              variant="outline"
              className="w-full"
              onClick={() => void importPat()}
              disabled={
                isImporting || isAddingAccount || personalToken.trim() === ""
              }
            >
              {isImporting ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : (
                <KeyRound className="mr-2 h-4 w-4" />
              )}
              {t("qoderOauth.importPat")}
            </Button>
          </TabsContent>
        </Tabs>
      ) : null}

      {isPolling && verificationUrl ? (
        <div
          className="space-y-3 rounded-md border border-border bg-muted/50 p-4"
          aria-live="polite"
        >
          <div className="flex items-center justify-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
            {t("accountAuth.waitingForBrowser")}
          </div>
          <div className="flex min-w-0 items-center gap-2 rounded-md border bg-background/80 p-3">
            <a
              href={verificationUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="min-w-0 flex-1 truncate text-sm text-blue-500 hover:underline"
              title={verificationUrl}
            >
              {verificationUrl}
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
                href={verificationUrl}
                target="_blank"
                rel="noopener noreferrer"
                title={t("accountAuth.openManually")}
              >
                <ExternalLink className="h-4 w-4" />
              </a>
            </Button>
          </div>
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
          disabled={isAddingAccount || isRemovingAccount || isImporting}
        >
          <LogOut className="mr-2 h-4 w-4" />
          {t("accountAuth.logoutAll")}
        </Button>
      ) : null}
    </div>
  );
};

export default QoderOAuthSection;
