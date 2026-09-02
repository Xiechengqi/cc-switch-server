import React from "react";
import { useTranslation } from "react-i18next";

import { DeviceCodeManagedAccountSection } from "./KimiOAuthSection";

interface TraeOAuthSectionProps {
  className?: string;
  selectedAccountId?: string | null;
  onAccountSelect?: (accountId: string | null) => void;
  showLoggedInAccounts?: boolean;
}

export const TraeOAuthSection: React.FC<TraeOAuthSectionProps> = (props) => {
  const { t } = useTranslation();

  return (
    <DeviceCodeManagedAccountSection
      {...props}
      authProvider="trae_solo"
      authStatusLabel={t("traeOauth.authStatus", {
        defaultValue: "Trae CN Solo authentication",
      })}
      loginLabel={t("traeOauth.loginWithTrae", {
        defaultValue: "Sign in with Trae CN Solo",
      })}
      manualCallback={{
        label: t("traeOauth.callbackUrl", {
          defaultValue: "Callback URL",
        }),
        hint: t("traeOauth.callbackHint", {
          defaultValue:
            "If the browser cannot reach the Server localhost callback, paste the complete callback URL from the address bar here.",
        }),
        placeholder: t("traeOauth.callbackPlaceholder", {
          defaultValue:
            "http://localhost:15721/api/accounts/trae/login/callback?...",
        }),
        submitLabel: t("traeOauth.submitCallback", {
          defaultValue: "Complete sign-in",
        }),
        requiredMessage: t("traeOauth.callbackRequired", {
          defaultValue: "Paste the complete Trae callback URL.",
        }),
        successMessage: t("traeOauth.loginSuccess", {
          defaultValue: "Trae CN Solo account signed in.",
        }),
      }}
    />
  );
};

export default TraeOAuthSection;
