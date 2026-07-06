import { useEffect, useState } from "react";
import { Mail } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  useEmailAuthRequestCodeMutation,
  useEmailAuthSessionMeQuery,
  useEmailAuthStatusQuery,
  useEmailAuthVerifyCodeMutation,
  useSettingsQuery,
} from "@/lib/query";
import { getTunnelConfigFromSettings } from "@/utils/shareUtils";

export function ShareEmailLoginCard() {
  const { t } = useTranslation();
  const { data: emailAuthStatus } = useEmailAuthStatusQuery();
  const { data: emailSession } = useEmailAuthSessionMeQuery();
  const { data: settings } = useSettingsQuery();
  const requestCodeMutation = useEmailAuthRequestCodeMutation();
  const verifyCodeMutation = useEmailAuthVerifyCodeMutation();
  const routerDomain = getTunnelConfigFromSettings(settings).domain;
  const [emailInput, setEmailInput] = useState("");
  const [codeInput, setCodeInput] = useState("");

  useEffect(() => {
    if (emailAuthStatus?.email) {
      setEmailInput(emailAuthStatus.email);
    }
  }, [emailAuthStatus?.email]);

  return (
    <section className="rounded-xl border border-border/60 bg-card/60 p-6">
      <div className="mb-4 flex items-center gap-3">
        <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-muted">
          <Mail className="h-5 w-5" />
        </div>
        <div className="min-w-0 flex-1">
          <h4 className="font-medium">
            {t("settings.authCenter.emailLoginTitle", {
              defaultValue: "Share Email Login",
            })}
          </h4>
          <p className="text-sm text-muted-foreground">
            {t("settings.authCenter.emailLoginDescription", {
              defaultValue:
                "用于确认当前设备的 share owner 邮箱；验证码登录成功后，创建和管理 share 时会自动绑定到分享节点。",
            })}
          </p>
        </div>
        <Badge
          variant={emailAuthStatus?.authenticated ? "default" : "secondary"}
        >
          {emailAuthStatus?.authenticated
            ? t("common.connected", { defaultValue: "已连接" })
            : t("common.notConnected", { defaultValue: "未连接" })}
        </Badge>
      </div>

      <div className="grid gap-4 md:grid-cols-2">
        <div className="space-y-2">
          <Label htmlFor="email-auth-email">
            {t("settings.authCenter.emailLabel", {
              defaultValue: "Email",
            })}
          </Label>
          <Input
            id="email-auth-email"
            type="email"
            value={emailInput}
            onChange={(event) => setEmailInput(event.currentTarget.value)}
            placeholder="name@example.com"
          />
        </div>
        <div className="space-y-2">
          <Label htmlFor="email-auth-code">
            {t("settings.authCenter.emailCodeLabel", {
              defaultValue: "Verification Code",
            })}
          </Label>
          <Input
            id="email-auth-code"
            inputMode="numeric"
            value={codeInput}
            onChange={(event) => setCodeInput(event.currentTarget.value)}
            placeholder="123456"
          />
        </div>
      </div>

      <div className="mt-4 flex flex-wrap gap-2">
        <Button
          type="button"
          variant="secondary"
          disabled={!emailInput.trim() || requestCodeMutation.isPending}
          onClick={() =>
            requestCodeMutation.mutate({
              routerDomain,
              email: emailInput,
            })
          }
        >
          {t("settings.authCenter.sendEmailCode", {
            defaultValue: "发送验证码",
          })}
        </Button>
        <Button
          type="button"
          disabled={
            !emailInput.trim() ||
            !codeInput.trim() ||
            verifyCodeMutation.isPending
          }
          onClick={() =>
            verifyCodeMutation.mutate({
              routerDomain,
              email: emailInput,
              code: codeInput,
            })
          }
        >
          {t("settings.authCenter.verifyEmailCode", {
            defaultValue: "验证并登录",
          })}
        </Button>
      </div>

      <div className="mt-4 space-y-1 text-sm text-muted-foreground">
        <div>
          {t("settings.authCenter.currentEmail", {
            defaultValue: "当前邮箱",
          })}
          : {emailAuthStatus?.email ?? "-"}
        </div>
        <div>
          {t("settings.authCenter.installationOwnerEmail", {
            defaultValue: "设备绑定邮箱",
          })}
          : {emailSession?.installationOwnerEmail ?? "-"}
        </div>
        <div>
          {t("settings.authCenter.emailLoginHint", {
            defaultValue:
              "同一设备创建 share 后会锁定 owner 邮箱，不允许切换或退出登录。",
          })}
        </div>
      </div>
    </section>
  );
}
