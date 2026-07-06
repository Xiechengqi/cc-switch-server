import React from "react";
import { useTranslation } from "react-i18next";
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
import { Loader2, Plus, User, X } from "lucide-react";
import { useDeepSeekAccount } from "./hooks/useDeepSeekAccount";

interface DeepSeekAccountSectionProps {
  className?: string;
  selectedAccountId?: string | null;
  onAccountSelect?: (accountId: string | null) => void;
  showLoggedInAccounts?: boolean;
}

export const DeepSeekAccountSection: React.FC<DeepSeekAccountSectionProps> = ({
  className,
  selectedAccountId,
  onAccountSelect,
  showLoggedInAccounts = false,
}) => {
  const { t } = useTranslation();
  const [identifier, setIdentifier] = React.useState("");
  const [password, setPassword] = React.useState("");
  const [showForm, setShowForm] = React.useState(false);

  const {
    accounts,
    hasAnyAccount,
    error,
    isAddingAccount,
    isRemovingAccount,
    isSettingDefaultAccount,
    defaultAccountId,
    addAccount,
    removeAccount,
    setDefaultAccount,
  } = useDeepSeekAccount();

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

  const handleSubmit = async (event: React.FormEvent) => {
    event.preventDefault();
    const nextIdentifier = identifier.trim();
    const nextPassword = password.trim();
    if (!nextIdentifier) {
      return;
    }
    if (!nextPassword) {
      return;
    }
    const isEmail = nextIdentifier.includes("@");
    try {
      const account = await addAccount({
        email: isEmail ? nextIdentifier : null,
        mobile: isEmail ? null : nextIdentifier,
        password: nextPassword,
      });
      onAccountSelect?.(account.id);
      setIdentifier("");
      setPassword("");
      setShowForm(false);
    } catch {
      // Error is surfaced by the hook.
    }
  };

  return (
    <div className={`space-y-4 ${className || ""}`}>
      <div className="flex items-center justify-between">
        <Label>{t("deepseekAccount.authStatus", "DeepSeek 账号")}</Label>
        <Badge
          variant={hasAnyAccount ? "default" : "secondary"}
          className={hasAnyAccount ? "bg-green-500 hover:bg-green-600" : ""}
        >
          {hasAnyAccount
            ? t("deepseekAccount.accountCount", {
                count: accounts.length,
                defaultValue: `${accounts.length} 个账号`,
              })
            : t("deepseekAccount.notAuthenticated", "未登录")}
        </Badge>
      </div>

      {hasAnyAccount && showLoggedInAccounts && (
        <div className="space-y-2">
          <Label className="text-sm text-muted-foreground">
            {t("deepseekAccount.loggedInAccounts", "已登录账号")}
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
                    {account.login}
                  </span>
                  {defaultAccountId === account.id && (
                    <Badge variant="secondary" className="text-xs">
                      {t("deepseekAccount.defaultAccount", "默认")}
                    </Badge>
                  )}
                  {selectedAccountId === account.id && (
                    <Badge variant="outline" className="text-xs">
                      {t("deepseekAccount.selected", "已选中")}
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
                      {t("deepseekAccount.setAsDefault", "设为默认")}
                    </Button>
                  )}
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="h-7 w-7 text-muted-foreground hover:text-red-500"
                    onClick={(e) => handleRemoveAccount(account.id, e)}
                    disabled={isRemovingAccount}
                    title={t("deepseekAccount.removeAccount", "移除账号")}
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
            {t("deepseekAccount.selectAccount", "选择账号")}
          </Label>
          <Select
            value={selectedAccountId || "none"}
            onValueChange={handleAccountSelect}
          >
            <SelectTrigger>
              <SelectValue
                placeholder={t(
                  "deepseekAccount.selectAccountPlaceholder",
                  "选择一个 DeepSeek 账号",
                )}
              />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="none">
                <span className="text-muted-foreground">
                  {t("deepseekAccount.useDefaultAccount", "使用默认账号")}
                </span>
              </SelectItem>
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

      {showForm && (
        <form className="space-y-3" onSubmit={handleSubmit}>
          <div className="space-y-2">
            <Label htmlFor="deepseek-identifier">
              {t("deepseekAccount.identifier", "邮箱或手机号")}
            </Label>
            <Input
              id="deepseek-identifier"
              type="text"
              value={identifier}
              onChange={(event) => setIdentifier(event.currentTarget.value)}
              autoComplete="username"
              placeholder={t(
                "deepseekAccount.identifierPlaceholder",
                "请输入邮箱或手机号",
              )}
              required
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="deepseek-password">
              {t("deepseekAccount.password", "密码")}
            </Label>
            <Input
              id="deepseek-password"
              type="password"
              value={password}
              onChange={(event) => setPassword(event.currentTarget.value)}
              autoComplete="current-password"
              required
            />
          </div>
          {error && <p className="text-sm text-red-500">{error}</p>}
          <div className="flex gap-2">
            <Button
              type="submit"
              variant="outline"
              className="flex-1"
              disabled={isAddingAccount || !identifier.trim()}
            >
              {isAddingAccount ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : (
                <Plus className="mr-2 h-4 w-4" />
              )}
              {t("deepseekAccount.addAccount", "添加账号")}
            </Button>
            <Button
              type="button"
              variant="ghost"
              onClick={() => setShowForm(false)}
            >
              {t("common.cancel", "取消")}
            </Button>
          </div>
        </form>
      )}

      {!showForm && (
        <Button
          type="button"
          onClick={() => setShowForm(true)}
          className="w-full"
          variant="outline"
        >
          <Plus className="mr-2 h-4 w-4" />
          {hasAnyAccount
            ? t("deepseekAccount.addAnotherAccount", "添加其他账号")
            : t("deepseekAccount.addAccount", "添加账号")}
        </Button>
      )}
    </div>
  );
};
