import React from "react";
import { useTranslation } from "react-i18next";

import { DeviceCodeManagedAccountSection } from "./KimiOAuthSection";

interface TraeOAuthSectionProps {
  authProvider?: "trae_solo" | "trae_raw_chat" | "trae_raw_chat_intl";
  className?: string;
  selectedAccountId?: string | null;
  onAccountSelect?: (accountId: string | null) => void;
  showLoggedInAccounts?: boolean;
}

export const TraeOAuthSection: React.FC<TraeOAuthSectionProps> = (props) => {
  const { t } = useTranslation();

  const { authProvider = "trae_solo", ...common } = props;
  const label =
    authProvider === "trae_solo"
      ? "Trae CN Solo"
      : authProvider === "trae_raw_chat"
        ? "Trae CN raw_chat"
        : "Trae Intl raw_chat";

  return (
    <DeviceCodeManagedAccountSection
      {...common}
      authProvider={authProvider}
      authStatusLabel={label}
      loginLabel={`${t("traeOauth.signIn", { defaultValue: "Sign in" })} · ${label}`}
      manualCallback={{
        label: t("traeOauth.callbackUrl", {
          defaultValue: "Callback URL",
        }),
        hint: t("traeOauth.callbackHint", {
          defaultValue:
            "Finish authorization on the Trae website. If the browser then cannot open 127.0.0.1, copy the complete URL from its address bar and paste it here to finish remote sign-in.",
        }),
        placeholder: t("traeOauth.callbackPlaceholder", {
          defaultValue:
            "http://127.0.0.1:15721/authorize?...",
        }),
        submitLabel: t("traeOauth.submitCallback", {
          defaultValue: "Complete sign-in",
        }),
        requiredMessage: t("traeOauth.callbackRequired", {
          defaultValue: "Paste the complete Trae callback URL.",
        }),
        successMessage: t("traeOauth.loginSuccess", {
          defaultValue: "Trae account signed in.",
        }),
      }}
    />
  );
};

export default TraeOAuthSection;
