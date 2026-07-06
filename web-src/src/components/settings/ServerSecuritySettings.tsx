import { FormEvent, useState } from "react";
import { KeyRound, Loader2, LogOut, Shield } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { changeServerPassword } from "@/lib/server-legacy-api";
import { writeCachedPassword, writeToken } from "@/lib/runtime";

interface ServerSecuritySettingsProps {
  onSignOut?: (options?: { clearPasswordCache?: boolean }) => void;
}

export function ServerSecuritySettings({ onSignOut }: ServerSecuritySettingsProps) {
  const { t } = useTranslation();
  const [newPassword, setNewPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleChangePassword(event: FormEvent) {
    event.preventDefault();
    setError(null);

    const trimmed = newPassword.trim();
    if (trimmed.length < 8) {
      setError(
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
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="rounded-xl border border-border/60 bg-card/60 p-6 space-y-6">
      <header className="space-y-1">
        <div className="flex items-center gap-2">
          <Shield className="h-4 w-4 text-primary" />
          <h3 className="text-sm font-medium">
            {t("settings.serverSecurity.changePasswordTitle", {
              defaultValue: "密码修改",
            })}
          </h3>
        </div>
        <p className="text-xs text-muted-foreground">
          {t("settings.serverSecurity.changePasswordDescription", {
            defaultValue: "修改管理员登录密码。",
          })}
        </p>
      </header>

      <form className="space-y-4" onSubmit={handleChangePassword}>
          <div className="space-y-2">
            <Label htmlFor="server-new-password">
              {t("settings.serverSecurity.newPassword", {
                defaultValue: "新密码",
              })}
            </Label>
            <Input
              id="server-new-password"
              type="text"
              autoComplete="off"
              value={newPassword}
              onChange={(event) => setNewPassword(event.target.value)}
            />
          </div>
          {error ? <p className="text-sm text-destructive">{error}</p> : null}
          <Button type="submit" disabled={busy || !newPassword.trim()}>
            {busy ? (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            ) : (
              <KeyRound className="mr-2 h-4 w-4" />
            )}
            {t("common.save", { defaultValue: "保存" })}
          </Button>
        </form>

      {onSignOut ? (
        <div className="flex justify-end border-t border-border/60 pt-4">
          <Button
            type="button"
            className="bg-red-600 text-white hover:bg-red-700"
            onClick={() => onSignOut()}
          >
            <LogOut className="mr-2 h-4 w-4" />
            {t("settings.serverSecurity.signOut", {
              defaultValue: "登出",
            })}
          </Button>
        </div>
      ) : null}
    </section>
  );
}
