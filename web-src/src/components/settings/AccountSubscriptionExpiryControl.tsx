import { useEffect, useId, useMemo, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { CalendarClock, Loader2, Save, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { formatExpireDistance } from "@/components/SubscriptionQuotaFooter";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  authApi,
  type ManagedAuthAccount,
  type ManagedAuthStatus,
} from "@/lib/api";
import { extractErrorMessage } from "@/utils/errorUtils";

interface AccountSubscriptionExpiryControlProps {
  account: ManagedAuthAccount;
  context?: "subscription" | "next_payment";
}

function toLocalDateTimeInput(value: string | null): string {
  if (!value) return "";
  const timestamp = new Date(value).getTime();
  if (!Number.isFinite(timestamp)) return "";
  const localTimestamp =
    timestamp - new Date(timestamp).getTimezoneOffset() * 60_000;
  return new Date(localTimestamp).toISOString().slice(0, 16);
}

function toUtcIso(value: string): string | null {
  if (!value) return null;
  const date = new Date(value);
  return Number.isFinite(date.getTime()) ? date.toISOString() : null;
}

export function AccountSubscriptionExpiryControl({
  account,
  context = "subscription",
}: AccountSubscriptionExpiryControlProps) {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const inputId = useId();
  const expiry = account.subscriptionExpiry;
  const [manualInput, setManualInput] = useState(() =>
    toLocalDateTimeInput(expiry.manualExpiresAt),
  );
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    setManualInput(toLocalDateTimeInput(expiry.manualExpiresAt));
  }, [expiry.manualExpiresAt]);

  useEffect(() => {
    if (!expiry.effectiveExpiresAt) return;
    setNow(Date.now());
    const interval = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(interval);
  }, [expiry.effectiveExpiresAt]);

  const effectiveLabel = useMemo(() => {
    if (!expiry.effectiveExpiresAt) return null;
    const date = new Date(expiry.effectiveExpiresAt);
    if (!Number.isFinite(date.getTime())) return null;
    return new Intl.DateTimeFormat(i18n.resolvedLanguage ?? i18n.language, {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(date);
  }, [expiry.effectiveExpiresAt, i18n.language, i18n.resolvedLanguage]);

  const invalidateRelatedQueries = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["managed-auth-status"] }),
      queryClient.invalidateQueries({ queryKey: ["subscription"] }),
      queryClient.invalidateQueries({ queryKey: [account.provider, "quota"] }),
      queryClient.invalidateQueries({ queryKey: ["providers"] }),
      queryClient.invalidateQueries({ queryKey: ["share"] }),
    ]);
  };

  const mutation = useMutation({
    mutationFn: (expiresAt: string | null) =>
      authApi.authSetManualSubscriptionExpiry(
        account.provider,
        account.id,
        expiresAt,
      ),
    onSuccess: async (updatedAccount, expiresAt) => {
      setManualInput(
        toLocalDateTimeInput(
          updatedAccount.subscriptionExpiry.manualExpiresAt ?? expiresAt,
        ),
      );
      queryClient.setQueryData<ManagedAuthStatus>(
        ["managed-auth-status", account.provider],
        (current) =>
          current
            ? {
                ...current,
                accounts: current.accounts.map((candidate) =>
                  candidate.id === updatedAccount.id
                    ? updatedAccount
                    : candidate,
                ),
              }
            : current,
      );
      await invalidateRelatedQueries();
      toast.success(
        expiresAt
          ? t("settings.authCenter.subscriptionExpiry.saved")
          : t("settings.authCenter.subscriptionExpiry.cleared"),
      );
    },
    onError: (error) => {
      toast.error(
        t("settings.authCenter.subscriptionExpiry.saveFailed", {
          error:
            extractErrorMessage(error) ||
            t("settings.authCenter.subscriptionExpiry.unknownError"),
        }),
      );
    },
  });

  if (
    expiry.capability === "research_pending" ||
    expiry.capability === "not_applicable"
  ) {
    return null;
  }

  const supportsManual =
    expiry.capability === "manual_required" ||
    expiry.capability === "automatic_or_manual";
  const title = t(
    context === "next_payment"
      ? "settings.authCenter.subscriptionExpiry.nextPaymentTitle"
      : "settings.authCenter.subscriptionExpiry.title",
  );
  const persistedInput = toLocalDateTimeInput(expiry.manualExpiresAt);
  const utcValue = toUtcIso(manualInput);
  const hasDraft = manualInput !== persistedInput;
  const distance = expiry.effectiveExpiresAt
    ? formatExpireDistance(expiry.effectiveExpiresAt, now, (key, options) =>
        t(key, options),
      )
    : null;
  const isSaving = mutation.isPending && mutation.variables !== null;
  const isClearing = mutation.isPending && mutation.variables === null;

  return (
    <div className="mt-2 w-full space-y-2 border-t border-border/50 pt-2">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs">
        <CalendarClock className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        <span className="font-medium">{title}</span>
        <span className="text-muted-foreground">
          {effectiveLabel ??
            t(
              supportsManual
                ? "settings.authCenter.subscriptionExpiry.notSet"
                : "settings.authCenter.subscriptionExpiry.notAvailable",
            )}
        </span>
        {distance && <span className="text-muted-foreground">{distance}</span>}
        <span className="text-muted-foreground">
          {expiry.source === "automatic"
            ? t("settings.authCenter.subscriptionExpiry.automatic")
            : supportsManual
              ? t("settings.authCenter.subscriptionExpiry.manual")
              : t("settings.authCenter.subscriptionExpiry.automatic")}
        </span>
      </div>

      {supportsManual && (
        <div className="flex flex-wrap items-center gap-2">
          <Label htmlFor={inputId} className="sr-only">
            {title}
          </Label>
          <Input
            id={inputId}
            type="datetime-local"
            value={manualInput}
            onChange={(event) => setManualInput(event.currentTarget.value)}
            disabled={mutation.isPending}
            className="min-w-[12rem] flex-1 sm:max-w-[17rem]"
          />
          <TooltipProvider delayDuration={300}>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  type="button"
                  size="icon"
                  variant="outline"
                  className="h-9 w-9"
                  onClick={() => utcValue && mutation.mutate(utcValue)}
                  disabled={mutation.isPending || !hasDraft || !utcValue}
                  aria-label={t("settings.authCenter.subscriptionExpiry.save")}
                >
                  {isSaving ? (
                    <Loader2 className="h-4 w-4 animate-spin" />
                  ) : (
                    <Save className="h-4 w-4" />
                  )}
                </Button>
              </TooltipTrigger>
              <TooltipContent>
                {t("settings.authCenter.subscriptionExpiry.save")}
              </TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  type="button"
                  size="icon"
                  variant="ghost"
                  className="h-9 w-9 text-muted-foreground hover:text-destructive"
                  onClick={() => {
                    if (expiry.manualExpiresAt) {
                      mutation.mutate(null);
                    } else {
                      setManualInput("");
                    }
                  }}
                  disabled={
                    mutation.isPending ||
                    (!expiry.manualExpiresAt && !manualInput)
                  }
                  aria-label={t("settings.authCenter.subscriptionExpiry.clear")}
                >
                  {isClearing ? (
                    <Loader2 className="h-4 w-4 animate-spin" />
                  ) : (
                    <Trash2 className="h-4 w-4" />
                  )}
                </Button>
              </TooltipTrigger>
              <TooltipContent>
                {t("settings.authCenter.subscriptionExpiry.clear")}
              </TooltipContent>
            </Tooltip>
          </TooltipProvider>
        </div>
      )}
    </div>
  );
}
