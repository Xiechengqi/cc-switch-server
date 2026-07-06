import { useEffect, useRef, useState } from "react";
import { Mail, Server } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { TunnelConfig } from "@/lib/api";
import {
  useEmailAuthChangeOwnerEmailMutation,
  useEmailAuthRequestOwnerChangeCodeMutation,
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

interface ShareOwnerChangeEmailDialogProps {
  open: boolean;
  tunnelConfig: TunnelConfig;
  tunnelConfigSaving: boolean;
  currentEmail: string | null;
  onOpenChange: (open: boolean) => void;
  onSaveTunnelConfig: (config: TunnelConfig) => Promise<void> | void;
}

type Step = "router" | "email" | "code";

export function ShareOwnerChangeEmailDialog({
  open,
  tunnelConfig,
  tunnelConfigSaving,
  currentEmail,
  onOpenChange,
  onSaveTunnelConfig,
}: ShareOwnerChangeEmailDialogProps) {
  const { t } = useTranslation();
  const requestCodeMutation = useEmailAuthRequestOwnerChangeCodeMutation();
  const changeOwnerMutation = useEmailAuthChangeOwnerEmailMutation();
  const [step, setStep] = useState<Step>("router");
  const [routerDomain, setRouterDomain] = useState(tunnelConfig.domain);
  const [routerDomainError, setRouterDomainError] = useState<string | null>(
    null,
  );
  const [newEmail, setNewEmail] = useState("");
  const [code, setCode] = useState("");
  const [codeSentTo, setCodeSentTo] = useState("");
  const wasOpenRef = useRef(false);

  useEffect(() => {
    if (open && !wasOpenRef.current) {
      setStep("router");
      setRouterDomain(tunnelConfig.domain);
      setRouterDomainError(null);
      setNewEmail("");
      setCode("");
      setCodeSentTo("");
    }
    wasOpenRef.current = open;
  }, [open, tunnelConfig.domain]);

  const normalizedCurrentEmail = currentEmail?.trim().toLowerCase() ?? "";
  const normalizedNewEmail = newEmail.trim().toLowerCase();
  const canSendCode =
    Boolean(normalizedCurrentEmail) &&
    Boolean(routerDomain.trim()) &&
    Boolean(normalizedNewEmail) &&
    normalizedNewEmail !== normalizedCurrentEmail;
  const canSubmitChange =
    canSendCode && Boolean(code.trim()) && codeSentTo === normalizedNewEmail;

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
    if (!canSendCode) return;
    try {
      await requestCodeMutation.mutateAsync({
        routerDomain: normalizeShareRouterDomain(routerDomain),
        currentEmail: normalizedCurrentEmail,
        newEmail: normalizedNewEmail,
      });
      setCode("");
      setCodeSentTo(normalizedNewEmail);
      setStep("code");
    } catch {
      return;
    }
  };

  const handleChangeOwner = async () => {
    if (!canSubmitChange) return;
    try {
      await changeOwnerMutation.mutateAsync({
        routerDomain: normalizeShareRouterDomain(routerDomain),
        currentEmail: normalizedCurrentEmail,
        newEmail: normalizedNewEmail,
        code: code.trim(),
      });
      onOpenChange(false);
    } catch {
      return;
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>
            {t("share.ownerChange.title", {
              defaultValue: "Change Owner Email",
            })}
          </DialogTitle>
          <DialogDescription>
            {t("share.ownerChange.description", {
              defaultValue:
                "Choose a router, enter the new owner email, then verify the code sent to that email.",
            })}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-5 px-6 py-5">
          <div className="flex items-center gap-2 text-sm text-muted-foreground">
            <span
              className={step === "router" ? "font-medium text-foreground" : ""}
            >
              1.{" "}
              {t("share.ownerChange.routerStep", {
                defaultValue: "Router",
              })}
            </span>
            <span>/</span>
            <span
              className={step === "email" ? "font-medium text-foreground" : ""}
            >
              2.{" "}
              {t("share.ownerChange.emailStep", {
                defaultValue: "New Owner",
              })}
            </span>
            <span>/</span>
            <span
              className={step === "code" ? "font-medium text-foreground" : ""}
            >
              3.{" "}
              {t("share.ownerChange.codeStep", {
                defaultValue: "Code",
              })}
            </span>
          </div>

          {step === "router" ? (
            <div className="space-y-3">
              <div className="flex items-start gap-3 rounded-lg border border-border/60 bg-muted/30 p-3">
                <Server className="mt-0.5 h-4 w-4 text-muted-foreground" />
                <div className="space-y-1 text-sm">
                  <div>
                    <span className="text-muted-foreground">
                      {t("share.ownerChange.currentOwner", {
                        defaultValue: "Current owner",
                      })}
                      :{" "}
                    </span>
                    <span className="font-medium">
                      {normalizedCurrentEmail || "-"}
                    </span>
                  </div>
                </div>
              </div>
              <div className="space-y-2">
                <Label htmlFor="share-owner-change-router">
                  {t("share.tunnel.region")}
                </Label>
                <ShareRouterSelector
                  value={routerDomain}
                  onChange={(value) => {
                    setRouterDomain(value);
                    setRouterDomainError(null);
                  }}
                  selectId="share-owner-change-router"
                  customInputId="share-owner-change-router-custom"
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
                {t("share.ownerChange.selectedRouter", {
                  defaultValue: "Selected router: {{router}}",
                  router: routerDomain,
                })}
              </div>
              <div className="space-y-2">
                <Label htmlFor="share-owner-new-email">
                  {t("share.ownerChange.newEmailLabel", {
                    defaultValue: "New owner email",
                  })}
                </Label>
                <Input
                  id="share-owner-new-email"
                  type="email"
                  value={newEmail}
                  onChange={(event) => setNewEmail(event.currentTarget.value)}
                  placeholder="name@example.com"
                  disabled={step === "code"}
                  autoComplete="off"
                />
              </div>
              {normalizedNewEmail === normalizedCurrentEmail ? (
                <div className="text-xs text-amber-600 dark:text-amber-400">
                  {t("share.ownerChange.sameEmailHint", {
                    defaultValue:
                      "The new owner email must be different from the current owner.",
                  })}
                </div>
              ) : null}
              {step === "code" ? (
                <div className="space-y-3">
                  <div className="rounded-lg border border-border/60 bg-muted/30 p-3 text-sm text-muted-foreground">
                    {t("share.ownerChange.codeSentToNewOwner", {
                      defaultValue:
                        "Verification code sent to new owner: {{email}}",
                      email: codeSentTo || normalizedNewEmail,
                    })}
                  </div>
                  <div className="space-y-2">
                    <Label htmlFor="share-owner-change-code">
                      {t("settings.authCenter.emailCodeLabel", {
                        defaultValue: "Verification Code",
                      })}
                    </Label>
                    <Input
                      id="share-owner-change-code"
                      inputMode="numeric"
                      value={code}
                      onChange={(event) => setCode(event.currentTarget.value)}
                      placeholder="123456"
                      autoComplete="one-time-code"
                    />
                  </div>
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
                setStep(step === "code" ? "email" : "router");
                setCode("");
                if (step === "code") {
                  setCodeSentTo("");
                }
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
              disabled={!canSendCode || requestCodeMutation.isPending}
            >
              <Mail className="h-4 w-4" />
              {t("settings.authCenter.sendEmailCode", {
                defaultValue: "发送验证码",
              })}
            </Button>
          ) : (
            <Button
              type="button"
              onClick={() => void handleChangeOwner()}
              disabled={!canSubmitChange || changeOwnerMutation.isPending}
            >
              {t("share.ownerChange.submit", {
                defaultValue: "Change Owner",
              })}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
