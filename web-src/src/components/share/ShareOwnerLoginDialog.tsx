import { useEffect, useRef, useState } from "react";
import { Mail, Server } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { TunnelConfig } from "@/lib/api";
import {
  useEmailAuthRequestCodeMutation,
  useEmailAuthVerifyCodeMutation,
} from "@/lib/query";
import { normalizeShareRouterDomain } from "@/utils/shareRouter";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ShareRouterSelector } from "./ShareRouterSelector";

interface ShareOwnerLoginDialogProps {
  open: boolean;
  tunnelConfig: TunnelConfig;
  tunnelConfigSaving: boolean;
  currentEmail?: string | null;
  lockedOwnerEmail?: string | null;
  onOpenChange: (open: boolean) => void;
  onSaveTunnelConfig: (config: TunnelConfig) => Promise<void> | void;
  onVerified?: () => Promise<void> | void;
}

type Step = "router" | "email" | "code";

export function ShareOwnerLoginDialog({
  open,
  tunnelConfig,
  tunnelConfigSaving,
  currentEmail,
  lockedOwnerEmail,
  onOpenChange,
  onSaveTunnelConfig,
  onVerified,
}: ShareOwnerLoginDialogProps) {
  const { t } = useTranslation();
  const requestCodeMutation = useEmailAuthRequestCodeMutation();
  const verifyCodeMutation = useEmailAuthVerifyCodeMutation();
  const [step, setStep] = useState<Step>("router");
  const [routerDomain, setRouterDomain] = useState(tunnelConfig.domain);
  const [routerDomainError, setRouterDomainError] = useState<string | null>(
    null,
  );
  const [email, setEmail] = useState(currentEmail ?? "");
  const [code, setCode] = useState("");
  const wasOpenRef = useRef(false);

  useEffect(() => {
    if (open && !wasOpenRef.current) {
      setStep("router");
      setRouterDomain(tunnelConfig.domain);
      setRouterDomainError(null);
      setEmail(lockedOwnerEmail ?? currentEmail ?? "");
      setCode("");
    }
    wasOpenRef.current = open;
  }, [currentEmail, lockedOwnerEmail, open, tunnelConfig.domain]);

  const normalizedEmail = email.trim().toLowerCase();
  const normalizedLockedOwnerEmail = lockedOwnerEmail?.trim().toLowerCase();
  const effectiveEmail = normalizedLockedOwnerEmail || normalizedEmail;
  const emailBlockedByOwner =
    Boolean(normalizedEmail) &&
    Boolean(normalizedLockedOwnerEmail) &&
    normalizedEmail !== normalizedLockedOwnerEmail;

  const handleContinue = async () => {
    let domain: string;
    try {
      domain = normalizeShareRouterDomain(routerDomain);
      setRouterDomainError(null);
    } catch (error) {
      const key =
        error instanceof Error
          ? error.message
          : "share.validation.invalidRouterDomain";
      setRouterDomainError(
        t(key, {
          defaultValue: "Router domain is invalid",
        }),
      );
      return;
    }
    try {
      if (domain !== tunnelConfig.domain) {
        await onSaveTunnelConfig({ domain });
      }
      setRouterDomain(domain);
      setStep("email");
    } catch {
      return;
    }
  };

  const handleSendCode = async () => {
    if (!effectiveEmail || emailBlockedByOwner) return;
    try {
      await requestCodeMutation.mutateAsync({
        routerDomain: normalizeShareRouterDomain(routerDomain),
        email: effectiveEmail,
      });
      setCode("");
      setStep("code");
    } catch {
      return;
    }
  };

  const handleVerify = async () => {
    try {
      await verifyCodeMutation.mutateAsync({
        routerDomain: normalizeShareRouterDomain(routerDomain),
        email: effectiveEmail,
        code: code.trim(),
      });
      onOpenChange(false);
      await onVerified?.();
    } catch {
      return;
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>
            {t("share.ownerLogin.title", {
              defaultValue: "Share Owner Login",
            })}
          </DialogTitle>
          <DialogDescription>
            {t("share.ownerLogin.description", {
              defaultValue:
                "Choose a router, enter the owner email, then verify the code sent by that router.",
            })}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-5 px-6 py-5">
          <div className="flex items-center gap-2 text-sm text-muted-foreground">
            <span
              className={step === "router" ? "font-medium text-foreground" : ""}
            >
              1. {t("share.ownerLogin.routerStep", { defaultValue: "Router" })}
            </span>
            <span>/</span>
            <span
              className={step === "email" ? "font-medium text-foreground" : ""}
            >
              2. {t("share.ownerLogin.emailStep", { defaultValue: "Email" })}
            </span>
            <span>/</span>
            <span
              className={step === "code" ? "font-medium text-foreground" : ""}
            >
              3. {t("share.ownerLogin.codeStep", { defaultValue: "Code" })}
            </span>
          </div>

          {step === "router" ? (
            <div className="space-y-3">
              <div className="flex items-start gap-3 rounded-lg border border-border/60 bg-muted/30 p-3">
                <Server className="mt-0.5 h-4 w-4 text-muted-foreground" />
                <div className="text-sm text-muted-foreground">
                  {t("share.ownerLogin.routerHint", {
                    defaultValue:
                      "Choose the share router first. The verification code will be sent by this router.",
                  })}
                </div>
              </div>
              <div className="space-y-2">
                <Label htmlFor="share-owner-router">
                  {t("share.tunnel.region")}
                </Label>
                <ShareRouterSelector
                  value={routerDomain}
                  onChange={(value) => {
                    setRouterDomain(value);
                    setRouterDomainError(null);
                  }}
                  selectId="share-owner-router"
                  customInputId="share-owner-router-custom"
                  disabled={tunnelConfigSaving}
                  error={routerDomainError}
                />
              </div>
              <div className="text-xs text-muted-foreground">
                {routerDomain || "-"}
              </div>
            </div>
          ) : null}

          {step !== "router" ? (
            <div className="space-y-3">
              <div className="rounded-lg border border-border/60 bg-muted/30 p-3 text-sm text-muted-foreground">
                {t("share.ownerLogin.selectedRouter", {
                  defaultValue: "Selected router: {{router}}",
                  router: routerDomain,
                })}
              </div>
              <div className="space-y-2">
                <Label htmlFor="share-owner-email">
                  {t("settings.authCenter.emailLabel", {
                    defaultValue: "Email",
                  })}
                </Label>
                <Input
                  id="share-owner-email"
                  type="email"
                  value={email}
                  onChange={(event) => setEmail(event.currentTarget.value)}
                  placeholder="name@example.com"
                  disabled={
                    step === "code" || Boolean(normalizedLockedOwnerEmail)
                  }
                />
              </div>
              {step === "code" ? (
                <div className="space-y-2">
                  <Label htmlFor="share-owner-code">
                    {t("settings.authCenter.emailCodeLabel", {
                      defaultValue: "Verification Code",
                    })}
                  </Label>
                  <Input
                    id="share-owner-code"
                    inputMode="numeric"
                    value={code}
                    onChange={(event) => setCode(event.currentTarget.value)}
                    placeholder="123456"
                  />
                </div>
              ) : null}
              {emailBlockedByOwner ? (
                <div className="text-xs text-amber-600 dark:text-amber-400">
                  {t("share.ownerLogin.lockedOwnerHint", {
                    defaultValue:
                      "Current device is bound to {{email}}. Use Change Owner Email to change it.",
                    email: normalizedLockedOwnerEmail,
                  })}
                </div>
              ) : null}
            </div>
          ) : null}
        </div>

        <DialogFooter className="gap-2">
          {step !== "router" ? (
            <Button
              type="button"
              variant="outline"
              onClick={() => {
                setStep("router");
                setCode("");
              }}
            >
              {t("common.back", { defaultValue: "Back" })}
            </Button>
          ) : null}
          {step === "router" ? (
            <Button
              type="button"
              onClick={() => void handleContinue()}
              disabled={!routerDomain.trim() || tunnelConfigSaving}
            >
              {t("common.continue", { defaultValue: "Continue" })}
            </Button>
          ) : step === "email" ? (
            <Button
              type="button"
              onClick={() => void handleSendCode()}
              disabled={
                !effectiveEmail ||
                emailBlockedByOwner ||
                requestCodeMutation.isPending
              }
            >
              <Mail className="h-4 w-4" />
              {t("settings.authCenter.sendEmailCode", {
                defaultValue: "发送验证码",
              })}
            </Button>
          ) : (
            <Button
              type="button"
              onClick={() => void handleVerify()}
              disabled={!code.trim() || verifyCodeMutation.isPending}
            >
              {t("settings.authCenter.verifyEmailCode", {
                defaultValue: "验证并登录",
              })}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
