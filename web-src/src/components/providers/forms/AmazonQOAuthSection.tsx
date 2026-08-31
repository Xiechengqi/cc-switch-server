import React from "react";
import { useTranslation } from "react-i18next";

import { DeviceCodeManagedAccountSection } from "./KimiOAuthSection";

interface AmazonQOAuthSectionProps {
  className?: string;
  selectedAccountId?: string | null;
  onAccountSelect?: (accountId: string | null) => void;
  showLoggedInAccounts?: boolean;
}

export const AmazonQOAuthSection: React.FC<AmazonQOAuthSectionProps> = (
  props,
) => {
  const { t } = useTranslation();
  return (
    <DeviceCodeManagedAccountSection
      {...props}
      authProvider="amazon_q_oauth"
      authStatusLabel={t("amazonQOauth.authStatus")}
      loginLabel={t("amazonQOauth.loginWithAmazonQ")}
      userCodeHint={t("amazonQOauth.userCodeHint")}
    />
  );
};

export default AmazonQOAuthSection;
