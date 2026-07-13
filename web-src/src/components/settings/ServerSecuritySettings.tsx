import { FormEvent, useState } from "react";
import { Loader2, Shield } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { isRemoteWebMode } from "@/lib/api/auth";
import { changeServerPassword } from "@/lib/server-legacy-api";
import {
  clearRouterSessionTokens,
  SERVER_AUTH_EXPIRED_EVENT,
} from "@/lib/routerAuth";
import { readCachedPassword, writeCachedPassword, writeToken } from "@/lib/runtime";

export function ServerSecuritySettings() {
  const { t } = useTranslation();
  const [currentPassword, setCurrentPassword] = useState(
    () => readCachedPassword() ?? "",
  );
  const [newPassword, setNewPassword] = useState("");
  const [busy, setBusy] = useState(false);

  async function handleChangePassword(event: FormEvent) {
    event.preventDefault();

    const trimmedCurrent = currentPassword.trim();
    const trimmedNew = newPassword.trim();
    if (!trimmedCurrent) {
      toast.error(
        t("settings.serverSecurity.currentPasswordRequired", {
          defaultValue: "请输入当前密码",
        }),
      );
      return;
    }
    if (trimmedNew.length < 8) {
      toast.error(
        t("settings.serverSecurity.passwordMinLength", {
          defaultValue: "新密码至少 8 位",
        }),
      );
      return;
    }

    setBusy(true);
    try {
      await changeServerPassword({
        currentPassword: trimmedCurrent,
        newPassword: trimmedNew,
      });
      setCurrentPassword("");
      setNewPassword("");
      writeToken(null);
      if (isRemoteWebMode()) {
        clearRouterSessionTokens();
      }
      writeCachedPassword(trimmedNew);
      toast.success(
        t("settings.serverSecurity.passwordChangedSignOut", {
          defaultValue: "密码已修改，请使用新密码重新登录",
        }),
      );
      window.dispatchEvent(new CustomEvent(SERVER_AUTH_EXPIRED_EVENT));
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="space-y-4">
      <form
        className="flex flex-col gap-4 rounded-xl border border-border bg-card/50 p-4 transition-colors hover:bg-muted/50 sm:flex-row sm:items-end sm:justify-between"
        onSubmit={handleChangePassword}
      >
        <input
          type="text"
          name="username"
          autoComplete="username"
          defaultValue="admin"
          tabIndex={-1}
          aria-hidden="true"
          className="sr-only"
          readOnly
        />
        <div className="flex min-w-0 items-start gap-3">
          <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-background ring-1 ring-border">
            <Shield className="h-4 w-4 text-amber-500" />
          </div>
          <div className="space-y-1">
            <p className="text-sm font-medium leading-none">
              {t("settings.serverSecurity.changePasswordTitle", {
                defaultValue: "密码修改",
              })}
            </p>
            <p className="text-xs text-muted-foreground">
              {t("settings.serverSecurity.changePasswordDescription", {
                defaultValue: "修改管理员登录密码",
              })}
            </p>
          </div>
        </div>

        <div className="flex w-full shrink-0 flex-col gap-2 sm:w-auto sm:flex-row sm:items-center">
          <Input
            id="server-current-password"
            type="password"
            autoComplete="current-password"
            className="h-9 w-full sm:w-44 placeholder:text-muted-foreground/50"
            placeholder={t("settings.serverSecurity.currentPassword", {
              defaultValue: "当前密码",
            })}
            value={currentPassword}
            onChange={(event) => setCurrentPassword(event.target.value)}
          />
          <Input
            id="server-new-password"
            type="password"
            autoComplete="new-password"
            className="h-9 w-full sm:w-44 sm:w-52 placeholder:text-muted-foreground/50"
            placeholder={t("settings.serverSecurity.newPassword", {
              defaultValue: "新密码",
            })}
            value={newPassword}
            onChange={(event) => setNewPassword(event.target.value)}
          />
          <Button
            type="submit"
            size="sm"
            className="h-9 shrink-0"
            disabled={
              busy || !currentPassword.trim() || !newPassword.trim()
            }
          >
            {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
            {t("common.save", { defaultValue: "保存" })}
          </Button>
        </div>
      </form>
    </section>
  );
}
