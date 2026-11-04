import React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { authImportBuddyCredentials } from "@/lib/api/auth";
import { oauthQuotaRootKey } from "@/lib/query/oauthQuotaKeys";
import { useManagedAccountsQuery } from "@/lib/query/accounts";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Textarea } from "@/components/ui/textarea";
import { useTranslation } from "react-i18next";

import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { CodeBuddySite } from "@/lib/api";
import { DeviceCodeManagedAccountSection } from "./KimiOAuthSection";
import { WorkBuddyCheckinControl } from "./WorkBuddyCheckinControl";

interface CodeBuddyOAuthSectionProps {
  authProvider?: "codebuddy_oauth" | "workbuddy_oauth";
  context?: "provider" | "auth-center";
  className?: string;
  selectedAccountId?: string | null;
  selectedAccountGeneration?: number;
  loginSite?: CodeBuddySite;
  onLoginSiteChange?: (site: CodeBuddySite) => void;
  onAccountSelect?: (accountId: string | null) => void;
  showLoggedInAccounts?: boolean;
}

export const CodeBuddyOAuthSection: React.FC<CodeBuddyOAuthSectionProps> = ({
  className,
  authProvider = "codebuddy_oauth",
  context = "provider",
  selectedAccountGeneration,
  loginSite,
  onLoginSiteChange,
  ...props
}) => {
  const { t } = useTranslation();
  const product = authProvider === "workbuddy_oauth" ? "WorkBuddy" : "CodeBuddy";
  const locale = product === "WorkBuddy" ? "workBuddyOauth" : "codeBuddyOauth";
  const accountsQuery = useManagedAccountsQuery({
    enabled: Boolean(props.selectedAccountId),
  });
  const account = accountsQuery.data?.find(
    (account) =>
      account.provider === authProvider &&
      account.id === props.selectedAccountId &&
      (selectedAccountGeneration == null ||
        account.authIdentityGeneration === selectedAccountGeneration),
  );
  const accountSite = account?.buddy?.site;
  const boundSite =
    accountSite === "cn" || accountSite === "intl" ? accountSite : undefined;
  const [localSite, setLocalSite] = React.useState<{
    product: string;
    site: CodeBuddySite;
  }>();
  const [loginBusy, setLoginBusy] = React.useState(false);
  const [activeLoginSite, setActiveLoginSite] = React.useState<CodeBuddySite>();
  const [activeImportSite, setActiveImportSite] = React.useState<CodeBuddySite>();
  // Legacy records derive their display default without changing the draft.
  // A late account query must never overwrite an explicit user selection.
  const site =
    activeLoginSite ??
    activeImportSite ??
    loginSite ??
    (!onLoginSiteChange && localSite?.product === authProvider
      ? localSite.site
      : undefined) ??
    boundSite ??
    "intl";
  const onLoginBusyChange = React.useCallback(
    (busy: boolean, startedSite?: CodeBuddySite | null) => {
      setLoginBusy(busy);
      setActiveLoginSite(busy ? startedSite ?? undefined : undefined);
    },
    [],
  );

  const queryClient = useQueryClient();
  const [credentials, setCredentials] = React.useState("");
  const [importBusy, setImportBusy] = React.useState(false);
  const [importError, setImportError] = React.useState<string | null>(null);
  const [previewCount, setPreviewCount] = React.useState<number | null>(null);
  const scopeKey = `${authProvider}:${site}`;
  const importScope = React.useRef({ key: scopeKey });
  if (importScope.current.key !== scopeKey) {
    importScope.current = { key: scopeKey };
  }
  React.useEffect(() => {
    const scope = { key: scopeKey };
    importScope.current = scope;
    setCredentials("");
    setPreviewCount(null);
    setImportError(null);
    setImportBusy(false);
    return () => {
      if (importScope.current === scope) {
        importScope.current = { key: "cancelled" };
      }
    };
  }, [scopeKey]);
  const importCredentials = async () => {
    if (importBusy || loginBusy) return;
    const scope = importScope.current;
    setActiveImportSite(site);
    setImportBusy(true);
    setImportError(null);
    try {
      const result = await authImportBuddyCredentials(
        authProvider,
        site,
        JSON.parse(credentials),
        previewCount === null,
      );
      // A successful write must refresh shared views even if its form was
      // unmounted. Only updates to this form depend on the operation scope.
      if (!result.preview) {
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: ["managed-auth-status", authProvider] }),
          queryClient.invalidateQueries({ queryKey: ["managed-auth-accounts"] }),
          queryClient.invalidateQueries({ queryKey: ["account-checkin"] }),
          queryClient.invalidateQueries({ queryKey: ["subscription"] }),
          queryClient.invalidateQueries({ queryKey: oauthQuotaRootKey(authProvider) }),
          queryClient.invalidateQueries({ queryKey: ["providers"] }),
          queryClient.invalidateQueries({ queryKey: ["share"] }),
        ]);
      }
      if (importScope.current !== scope) return;
      if (result.preview) {
        setPreviewCount(result.accounts.length);
      } else {
        setCredentials("");
        setPreviewCount(null);
        if (result.accounts.length === 1) props.onAccountSelect?.(result.accounts[0].id);
      }
    } catch {
      if (importScope.current !== scope) return;
      setPreviewCount(null);
      setImportError(t(`${locale}.importError`, { defaultValue: "Import failed. Check the product, site, token pair and UID; encrypted desktop exports require native login." }));
    } finally {
      if (importScope.current === scope) {
        setImportBusy(false);
        setActiveImportSite(undefined);
      }
    }
  };

  return (
    <div className={`space-y-4 ${className || ""}`}>
      {props.selectedAccountId ? (
        <p className="text-sm" role="status">
          {t(`${locale}.boundAccountSite`, {
            site: t(`${locale}.${boundSite ? `${boundSite}Site` : "unknownSite"}`),
          })}
        </p>
      ) : null}
      {context === "provider" && authProvider === "workbuddy_oauth" && account ? (
        <WorkBuddyCheckinControl key={`${account.id}:${account.authIdentityGeneration}`} account={account} />
      ) : null}
      <div className="space-y-2">
        <Label htmlFor={`${authProvider}-login-site`}>
          {t(`${locale}.loginSite`, {
            defaultValue: "Site for new login / import",
          })}
        </Label>
        <Select
          value={site}
          disabled={importBusy || loginBusy}
          onValueChange={(value) => {
            if (importBusy || loginBusy) return;
            if (value === "intl" || value === "cn") {
              if (onLoginSiteChange) onLoginSiteChange(value);
              else setLocalSite({ product: authProvider, site: value });
              setPreviewCount(null);
            }
          }}
        >
          <SelectTrigger id={`${authProvider}-login-site`}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="intl">
              {t(`${locale}.intlSite`, {
                defaultValue: "International",
              })}
            </SelectItem>
            <SelectItem value="cn">
              {t(`${locale}.cnSite`, { defaultValue: "China" })}
            </SelectItem>
          </SelectContent>
        </Select>
        <p className="text-xs text-muted-foreground">
          {t(`${locale}.${context === "auth-center" ? "authCenterSiteHint" : "siteHint"}`, {
            defaultValue:
              context === "auth-center"
                ? "This selection is kept on this page for new logins and imports only. Existing accounts keep their actual site."
                : "Save the provider to remember this preference for new logins and imports. Existing accounts keep their actual site.",
          })}
        </p>
      </div>
      <details className="space-y-2">
        <summary className="cursor-pointer text-sm">{t(`${locale}.importTitle`, { defaultValue: "Import credentials" })}</summary>
        <p className="text-xs text-muted-foreground">{t(`${locale}.importHint`, { defaultValue: "The selected product and site apply to every entry. Paste a token pair with UID/domain or an account/auth export. Preview validates locally; it does not contact the provider." })}</p>
        <Textarea
          value={credentials} disabled={importBusy || loginBusy}
          autoComplete="off" spellCheck={false}
          aria-label={t(`${locale}.importTitle`, { defaultValue: "Import credentials" })}
          onChange={(event) => { setCredentials(event.target.value); setPreviewCount(null); setImportError(null); }}
        />
        {importError && <p role="alert" className="text-sm text-destructive">{importError}</p>}
        <Button type="button" variant="outline" disabled={importBusy || loginBusy || !credentials.trim()} onClick={() => void importCredentials()}>
          {previewCount === null
            ? t(`${locale}.previewImport`, { defaultValue: "Preview import" })
            : t(`${locale}.applyImport`, { defaultValue: "Import {{count}} accounts", count: previewCount })}
        </Button>
      </details>
      <DeviceCodeManagedAccountSection
        {...props}
        authProvider={authProvider}
        authStatusLabel={t(`${locale}.authStatus`, {
          defaultValue: `${product} OAuth authentication`,
        })}
        loginLabel={t(`${locale}.loginWithCodeBuddy`, {
          defaultValue: `Sign in with ${product}`,
        })}
        loginOptions={{ codeBuddySite: site }}
        loginDisabled={importBusy}
        onLoginBusyChange={onLoginBusyChange}
        renderAccountDetails={context === "auth-center" && authProvider === "workbuddy_oauth"
          ? (account) => <WorkBuddyCheckinControl key={`${account.id}:${account.authIdentityGeneration}`} account={account} editable />
          : undefined}
        renderAccountMetadata={(account) => {
          const actualSite = account.buddy?.site;
          return (
            <Badge variant="outline" className="shrink-0 text-xs">
              {t(`${locale}.${actualSite === "cn" || actualSite === "intl" ? `${actualSite}Site` : "unknownSite"}`)}
            </Badge>
          );
        }}
      />
    </div>
  );
};

export default CodeBuddyOAuthSection;
