import React from "react";
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
  className?: string;
  selectedAccountId?: string | null;
  onAccountSelect?: (accountId: string | null) => void;
  showLoggedInAccounts?: boolean;
}

export const CodeBuddyOAuthSection: React.FC<CodeBuddyOAuthSectionProps> = ({
  className,
  ...props
}) => {
  const { t } = useTranslation();
  const [site, setSite] = React.useState<CodeBuddySite>("intl");

  return (
    <div className={`space-y-4 ${className || ""}`}>
      <div className="space-y-2">
        <Label htmlFor="codebuddy-login-site">
          {t("codeBuddyOauth.loginSite", {
            defaultValue: "Site for new login",
          })}
        </Label>
        <Select
          value={site}
          onValueChange={(value) => {
            if (value === "intl" || value === "cn") setSite(value);
          }}
        >
          <SelectTrigger id="codebuddy-login-site">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="intl">
              {t("codeBuddyOauth.intlSite", {
                defaultValue: "International",
              })}
            </SelectItem>
            <SelectItem value="cn">
              {t("codeBuddyOauth.cnSite", { defaultValue: "China" })}
            </SelectItem>
          </SelectContent>
        </Select>
        <p className="text-xs text-muted-foreground">
          {t("codeBuddyOauth.siteHint", {
            defaultValue:
              "The selected site is permanently bound to the imported account and is never changed by proxy errors.",
          })}
        </p>
      </div>
      <DeviceCodeManagedAccountSection
        {...props}
        authProvider="codebuddy_oauth"
        authStatusLabel={t("codeBuddyOauth.authStatus", {
          defaultValue: "CodeBuddy OAuth authentication",
        })}
        loginLabel={t("codeBuddyOauth.loginWithCodeBuddy", {
          defaultValue: "Sign in with CodeBuddy",
        })}
        loginOptions={{ codeBuddySite: site }}
      />
    </div>
  );
};

export default CodeBuddyOAuthSection;
