import { FormEvent, useState } from "react";
import { Loader2, Shield } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { changeServerPassword } from "@/lib/server-legacy-api";
import { writeCachedPassword, writeToken } from "@/lib/runtime";

interface ServerSecuritySettingsProps {
  onSignOut?: (options?: { clearPasswordCache?: boolean }) => void;
}

export function ServerSecuritySettings({ onSignOut }: ServerSecuritySettingsProps) {
  const { t } = useTranslation();
  const [newPassword, setNewPassword] = useState("");
  const [busy, setBusy] = useState(false);

  async function handleChangePassword(event: FormEvent) {
    event.preventDefault();

    const trimmed = newPassword.trim();
    if (trimmed.length < 8) {
      toast.error(
        t("settings.serverSecurity.passwordMinLength", {
          defaultValue: "新密码至少 8 位",
        }),
      );
      return;
    }

    setBusy(true);
    try {
      await changeServerPassword(trimmed);
      setNewPassword("");
      writeCachedPassword(trimmed);
      toast.success(
        t("settings.serverSecurity.passwordChangedSignOut", {
          defaultValue: "密码已修改，请使用新密码重新登录",
        }),
      );
      if (onSignOut) {
        onSignOut({ clearPasswordCache: false });
      } else {
        writeToken(null);
        window.location.reload();
      }
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
          <Input
            id="server-new-password"
            type="password"
            autoComplete="new-password"
            className="h-9 w-44 sm:w-52 placeholder:text-muted-foreground/50"
            placeholder={t("settings.serverSecurity.newPassword")}
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
