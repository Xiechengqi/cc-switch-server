import { useEffect, useRef, useState } from "react";
import { Server, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { TunnelConfig } from "@/lib/api";
import { useEmailAuthChangeOwnerEmailMutation } from "@/lib/query/emailAuth";
import { normalizeShareRouterDomain } from "@/utils/shareRouter";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
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
  onChanged?: () => Promise<void> | void;
}

type Step = "router" | "email";

export function ShareOwnerChangeEmailDialog({
  open,
  tunnelConfig,
  tunnelConfigSaving,
  currentEmail,
  onOpenChange,
  onSaveTunnelConfig,
  onChanged,
}: ShareOwnerChangeEmailDialogProps) {
  const { t } = useTranslation();
  const changeOwnerMutation = useEmailAuthChangeOwnerEmailMutation();
  const [step, setStep] = useState<Step>("router");
  const [routerDomain, setRouterDomain] = useState(tunnelConfig.domain);
  const [routerDomainError, setRouterDomainError] = useState<string | null>(
    null,
  );
  const [newEmail, setNewEmail] = useState("");
  const wasOpenRef = useRef(false);

  useEffect(() => {
    if (open && !wasOpenRef.current) {
      setStep("router");
      setRouterDomain(tunnelConfig.domain);
      setRouterDomainError(null);
      setNewEmail("");
    }
    wasOpenRef.current = open;
  }, [open, tunnelConfig.domain]);

  const normalizedCurrentEmail = currentEmail?.trim().toLowerCase() ?? "";
  const normalizedNewEmail = newEmail.trim().toLowerCase();
  const canSubmitChange =
    Boolean(normalizedCurrentEmail) &&
    Boolean(routerDomain.trim()) &&
    Boolean(normalizedNewEmail) &&
    normalizedNewEmail !== normalizedCurrentEmail;

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

  const handleChangeOwner = async () => {
    if (!canSubmitChange) return;
    try {
      await changeOwnerMutation.mutateAsync({
        routerDomain: normalizeShareRouterDomain(routerDomain),
        currentEmail: normalizedCurrentEmail,
        newEmail: normalizedNewEmail,
      });
      await onChanged?.();
      onOpenChange(false);
    } catch {
      return;
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader className="relative pr-12">
          <DialogTitle>
            {t("share.ownerChange.title", {
              defaultValue: "Change Owner Email",
            })}
          </DialogTitle>
          <DialogDescription>
            {t("share.ownerChange.description", {
              defaultValue:
                "Choose a router, enter the new owner email, then confirm. Server installation credentials authorize this change without email verification.",
            })}
          </DialogDescription>
          <DialogClose
            className="absolute right-0 top-0 rounded-full p-1.5 hover:bg-muted transition-colors focus:outline-none focus:ring-2 focus:ring-primary focus:ring-offset-2"
            aria-label={t("common.close", { defaultValue: "Close" })}
          >
            <X className="h-4 w-4 text-muted-foreground" />
          </DialogClose>
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
            </div>
          ) : (
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
                  autoComplete="off"
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      event.preventDefault();
                      void handleChangeOwner();
                    }
                  }}
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
              <p className="text-xs text-muted-foreground">
                {t("share.ownerChange.confirmMessage", {
                  defaultValue:
                    "Change the server owner to the email below? Local shares will rebind to the new owner.",
                })}
              </p>
            </div>
          )}
        </div>

        <DialogFooter className="gap-2">
          <Button
            type="button"
            variant="outline"
            onClick={() => onOpenChange(false)}
          >
            {t("common.close", { defaultValue: "Close" })}
          </Button>
          {step === "email" ? (
            <Button
              type="button"
              variant="outline"
              onClick={() => setStep("router")}
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
