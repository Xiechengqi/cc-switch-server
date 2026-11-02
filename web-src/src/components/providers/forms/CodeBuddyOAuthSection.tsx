import React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { authImportBuddyCredentials } from "@/lib/api/auth";
import { oauthQuotaRootKey } from "@/lib/query/oauthQuotaKeys";
import { Button } from "@/components/ui/button";
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

interface CodeBuddyOAuthSectionProps {
  authProvider?: "codebuddy_oauth" | "workbuddy_oauth";
  className?: string;
  selectedAccountId?: string | null;
  onAccountSelect?: (accountId: string | null) => void;
  showLoggedInAccounts?: boolean;
}

export const CodeBuddyOAuthSection: React.FC<CodeBuddyOAuthSectionProps> = ({
  className,
  authProvider = "codebuddy_oauth",
  ...props
}) => {
  const { t } = useTranslation();
  const product = authProvider === "workbuddy_oauth" ? "WorkBuddy" : "CodeBuddy";
  const locale = product === "WorkBuddy" ? "workBuddyOauth" : "codeBuddyOauth";
  const [site, setSite] = React.useState<CodeBuddySite>("intl");

  const queryClient = useQueryClient();
  const [credentials, setCredentials] = React.useState("");
  const [importBusy, setImportBusy] = React.useState(false);
  const [importError, setImportError] = React.useState<string | null>(null);
  const [previewCount, setPreviewCount] = React.useState<number | null>(null);
  const importScope = React.useRef(`${authProvider}:${site}`);
  importScope.current = `${authProvider}:${site}`;
  React.useEffect(() => {
    importScope.current = `${authProvider}:${site}`;
    setCredentials("");
    setPreviewCount(null);
    setImportError(null);
    setImportBusy(false);
    return () => { importScope.current = "cancelled"; };
  }, [authProvider, site]);
  const importCredentials = async () => {
    const scope = importScope.current;
    setImportBusy(true);
    setImportError(null);
    try {
      const result = await authImportBuddyCredentials(
        authProvider, site, JSON.parse(credentials), previewCount === null,
      );
      if (importScope.current !== scope) return;
      if (result.preview) {
        setPreviewCount(result.accounts.length);
      } else {
        setCredentials("");
        setPreviewCount(null);
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: ["managed-auth-status", authProvider] }),
          queryClient.invalidateQueries({ queryKey: ["managed-auth-accounts"] }),
          queryClient.invalidateQueries({ queryKey: ["subscription"] }),
          queryClient.invalidateQueries({ queryKey: oauthQuotaRootKey(authProvider) }),
          queryClient.invalidateQueries({ queryKey: ["providers"] }),
          queryClient.invalidateQueries({ queryKey: ["share"] }),
        ]);
        if (importScope.current !== scope) return;
        if (result.accounts.length === 1) props.onAccountSelect?.(result.accounts[0].id);
      }
    } catch {
      if (importScope.current !== scope) return;
      setPreviewCount(null);
      setImportError(t(`${locale}.importError`, { defaultValue: "Import failed. Check the product, site, token pair and UID; encrypted desktop exports require native login." }));
    } finally {
      if (importScope.current === scope) setImportBusy(false);
    }
  };

  return (
    <div className={`space-y-4 ${className || ""}`}>
      <div className="space-y-2">
        <Label htmlFor={`${authProvider}-login-site`}>
          {t(`${locale}.loginSite`, {
            defaultValue: "Site for new login",
          })}
        </Label>
        <Select
          value={site}
          disabled={importBusy}
          onValueChange={(value) => {
            if (value === "intl" || value === "cn") { setSite(value); setPreviewCount(null); }
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
          {t(`${locale}.siteHint`, {
            defaultValue:
              "The selected site is permanently bound to the imported account and is never changed by proxy errors.",
          })}
        </p>
      </div>
      <details className="space-y-2">
        <summary className="cursor-pointer text-sm">{t(`${locale}.importTitle`, { defaultValue: "Import credentials" })}</summary>
        <p className="text-xs text-muted-foreground">{t(`${locale}.importHint`, { defaultValue: "The selected product and site apply to every entry. Paste a token pair with UID/domain or an account/auth export. Preview validates locally; it does not contact the provider." })}</p>
        <Textarea
          value={credentials} disabled={importBusy}
          autoComplete="off" spellCheck={false}
          aria-label={t(`${locale}.importTitle`, { defaultValue: "Import credentials" })}
          onChange={(event) => { setCredentials(event.target.value); setPreviewCount(null); setImportError(null); }}
        />
        {importError && <p role="alert" className="text-sm text-destructive">{importError}</p>}
        <Button type="button" variant="outline" disabled={importBusy || !credentials.trim()} onClick={() => void importCredentials()}>
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
      />
    </div>
  );
};

export default CodeBuddyOAuthSection;
