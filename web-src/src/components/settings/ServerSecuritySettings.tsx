import { FormEvent, useState } from "react";
import { Loader2, Shield } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { isRemoteWebMode } from "@/lib/api/auth";
import { changeServerPassword } from "@/lib/server-legacy-api";
import {
  clearRouterSessionTokens,
  SERVER_AUTH_EXPIRED_EVENT,
} from "@/lib/routerAuth";
import { writeToken } from "@/lib/runtime";
import { SecretInput } from "@/server/ui/SecretInput";

/**
 * 管理员密码修改。
 *
 * 设计如此：**不要求输入旧密码**，只填新密码即可直接改。凭据是当前这个管理员
 * 会话本身（后端 `/web-api/auth/password/set` 用 `require_web_admin_session`
 * 校验），因此邮箱验证码 / API Token / Router SSO 等没有本地明文密码的登录方式
 * 也能改密码。改完后端会清空所有会话，前端随即清 token 并要求重新登录。
 *
 * 详见 AGENTS.md「管理员密码修改」一节，不要再加回「当前密码」「确认新密码」。
 */
export function ServerSecuritySettings() {
  const { t } = useTranslation();
  const [newPassword, setNewPassword] = useState("");
  const [busy, setBusy] = useState(false);

  async function handleChangePassword(event: FormEvent) {
    event.preventDefault();

    const trimmedNew = newPassword.trim();
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
      await changeServerPassword(trimmedNew);
      setNewPassword("");
      writeToken(null);
      if (isRemoteWebMode()) {
        clearRouterSessionTokens();
      }
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
        className="flex items-center justify-between gap-4 rounded-xl border border-border bg-card/50 p-4 transition-colors hover:bg-muted/50"
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
        <div className="flex min-w-0 items-center gap-3">
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

        <div className="flex shrink-0 items-center gap-2">
          <SecretInput
            id="server-new-password"
            type="password"
            autoComplete="new-password"
            aria-label={t("settings.serverSecurity.newPassword", {
              defaultValue: "新密码",
            })}
            placeholder={t("settings.serverSecurity.newPassword", {
              defaultValue: "新密码",
            })}
            className="h-9 w-44 placeholder:text-muted-foreground/50 sm:w-52"
            value={newPassword}
            onChange={(event) => setNewPassword(event.target.value)}
          />
          <Button
            type="submit"
            size="sm"
            className="h-9 shrink-0"
            disabled={busy || !newPassword.trim()}
          >
            {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
            {t("common.save", { defaultValue: "保存" })}
          </Button>
        </div>
      </form>
    </section>
  );
}
