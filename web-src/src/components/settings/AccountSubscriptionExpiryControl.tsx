import { useEffect, useMemo, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { CalendarClock, Loader2, Save, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { formatExpireDistance } from "@/components/SubscriptionQuotaFooter";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
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
  type SubscriptionExpiryCadence,
  type SubscriptionExpiryRule,
  type SubscriptionExpiryRuleDraft,
} from "@/lib/api";
import { cn } from "@/lib/utils";
import { extractErrorMessage } from "@/utils/errorUtils";

interface AccountSubscriptionExpiryControlProps {
  account: ManagedAuthAccount;
}

interface RuleFormState {
  cadence: SubscriptionExpiryCadence | null;
  month: number;
  day: number;
  timeZone: string;
}

const MONTHS = Array.from({ length: 12 }, (_, index) => index + 1);
const MONTHLY_DAYS = Array.from({ length: 31 }, (_, index) => index + 1);

function browserTimeZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
}

function daysInAnnualMonth(month: number): number[] {
  const count = new Date(Date.UTC(2000, month, 0)).getUTCDate();
  return Array.from({ length: count }, (_, index) => index + 1);
}

function legacyDateParts(value: string | null): { month: number; day: number } {
  const date = value ? new Date(value) : new Date();
  if (!Number.isFinite(date.getTime())) {
    const now = new Date();
    return { month: now.getMonth() + 1, day: now.getDate() };
  }
  return { month: date.getMonth() + 1, day: date.getDate() };
}

function formStateFromExpiry(
  rule: SubscriptionExpiryRule | null,
  legacyExpiresAt: string | null,
): RuleFormState {
  if (rule) {
    return {
      cadence: rule.cadence,
      month: rule.month ?? new Date().getMonth() + 1,
      day: rule.day,
      timeZone: rule.timeZone,
    };
  }
  const legacy = legacyDateParts(legacyExpiresAt);
  return {
    cadence: legacyExpiresAt ? null : "monthly",
    month: legacy.month,
    day: legacy.day,
    timeZone: browserTimeZone(),
  };
}

function draftFromForm(form: RuleFormState): SubscriptionExpiryRuleDraft | null {
  if (!form.cadence) return null;
  return {
    cadence: form.cadence,
    month: form.cadence === "yearly" ? form.month : null,
    day: form.day,
    timeZone: form.timeZone,
  };
}

function persistedDraft(
  rule: SubscriptionExpiryRule | null,
): SubscriptionExpiryRuleDraft | null {
  if (!rule) return null;
  return {
    cadence: rule.cadence,
    month: rule.cadence === "monthly" ? null : rule.month,
    day: rule.day,
    timeZone: rule.timeZone,
  };
}

function sameRule(
  first: SubscriptionExpiryRuleDraft | null,
  second: SubscriptionExpiryRuleDraft | null,
): boolean {
  return JSON.stringify(first) === JSON.stringify(second);
}

export function AccountSubscriptionExpiryControl({
  account,
}: AccountSubscriptionExpiryControlProps) {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const expiry = account.subscriptionExpiry;
  const [form, setForm] = useState(() =>
    formStateFromExpiry(expiry.rule, expiry.legacyManualExpiresAt),
  );
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    setForm(formStateFromExpiry(expiry.rule, expiry.legacyManualExpiresAt));
  }, [
    expiry.legacyManualExpiresAt,
    expiry.rule?.cadence,
    expiry.rule?.day,
    expiry.rule?.month,
    expiry.rule?.timeZone,
    expiry.rule?.updatedAtMs,
  ]);

  useEffect(() => {
    if (!expiry.effectiveExpiresAt && !expiry.ruleNextExpiresAt) return;
    setNow(Date.now());
    const interval = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(interval);
  }, [expiry.effectiveExpiresAt, expiry.ruleNextExpiresAt]);

  useEffect(() => {
    if (!expiry.rule || !expiry.ruleNextExpiresAt) {
      return;
    }
    const ruleNextAt = new Date(expiry.ruleNextExpiresAt).getTime();
    if (Number.isFinite(ruleNextAt) && now > ruleNextAt) {
      void queryClient.invalidateQueries({
        queryKey: ["managed-auth-status", account.provider],
      });
    }
  }, [
    account.provider,
    expiry.rule,
    expiry.ruleNextExpiresAt,
    now,
    queryClient,
  ]);

  const formatDate = (value: string | null, timeZone?: string): string | null => {
    if (!value) return null;
    const date = new Date(value);
    if (!Number.isFinite(date.getTime())) return null;
    try {
      return new Intl.DateTimeFormat(i18n.resolvedLanguage ?? i18n.language, {
        dateStyle: "medium",
        ...(timeZone ? { timeZone } : {}),
      }).format(date);
    } catch {
      return new Intl.DateTimeFormat(
        i18n.resolvedLanguage ?? i18n.language,
        { dateStyle: "medium" },
      ).format(date);
    }
  };

  const effectiveLabel = formatDate(
    expiry.effectiveExpiresAt,
    expiry.source === "recurring_rule" ? expiry.rule?.timeZone : undefined,
  );
  const ruleNextLabel = formatDate(
    expiry.ruleNextExpiresAt,
    expiry.rule?.timeZone,
  );
  const distance = expiry.effectiveExpiresAt
    ? formatExpireDistance(expiry.effectiveExpiresAt, now, (key, options) =>
        t(key, options),
      )
    : null;
  const supportsManual =
    expiry.capability === "manual_required" ||
    expiry.capability === "automatic_or_manual";
  const draft = draftFromForm(form);
  const hasDraft = !sameRule(draft, persistedDraft(expiry.rule));
  const yearlyDays = useMemo(() => daysInAnnualMonth(form.month), [form.month]);

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
    mutationFn: (rule: SubscriptionExpiryRuleDraft | null) =>
      authApi.authSetSubscriptionExpiryRule(
        account.provider,
        account.id,
        rule,
      ),
    onSuccess: async (updatedAccount, rule) => {
      const updatedExpiry = updatedAccount.subscriptionExpiry;
      setForm(
        formStateFromExpiry(
          updatedExpiry.rule,
          updatedExpiry.legacyManualExpiresAt,
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
        rule
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

  const sourceLabel =
    expiry.source === "automatic"
      ? t("settings.authCenter.subscriptionExpiry.automatic")
      : expiry.source === "manual"
        ? t("settings.authCenter.subscriptionExpiry.legacyManual")
        : expiry.source === "recurring_rule"
          ? t("settings.authCenter.subscriptionExpiry.manual")
          : null;
  const isSaving = mutation.isPending && mutation.variables !== null;
  const isClearing = mutation.isPending && mutation.variables === null;
  const hasPersistedManual = Boolean(
    expiry.rule || expiry.legacyManualExpiresAt,
  );

  return (
    <div className="mt-2 w-full space-y-2 border-t border-border/50 pt-2">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs">
        <CalendarClock className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        <span className="font-medium">
          {t("settings.authCenter.subscriptionExpiry.title")}
        </span>
        <span className="text-muted-foreground">
          {effectiveLabel ??
            t(
              supportsManual
                ? "settings.authCenter.subscriptionExpiry.notSet"
                : "settings.authCenter.subscriptionExpiry.notAvailable",
            )}
        </span>
        {distance && <span className="text-muted-foreground">{distance}</span>}
        {sourceLabel && (
          <span className="text-muted-foreground">{sourceLabel}</span>
        )}
      </div>

      {expiry.source === "automatic" && expiry.rule && (
        <div className="text-xs text-muted-foreground">
          {t("settings.authCenter.subscriptionExpiry.fallbackRule", {
            rule:
              expiry.rule.cadence === "monthly"
                ? t("settings.authCenter.subscriptionExpiry.monthlyDay", {
                    day: expiry.rule.day,
                  })
                : t("settings.authCenter.subscriptionExpiry.yearlyDate", {
                    month: expiry.rule.month,
                    day: expiry.rule.day,
                  }),
            date: ruleNextLabel,
          })}
        </div>
      )}

      {expiry.legacyManualExpiresAt && !expiry.rule && (
        <div className="text-xs text-amber-600 dark:text-amber-400">
          {t("settings.authCenter.subscriptionExpiry.legacyNeedsCadence")}
        </div>
      )}

      {supportsManual && (
        <div className="flex flex-wrap items-center gap-2">
          <div
            role="group"
            aria-label={t("settings.authCenter.subscriptionExpiry.cadence")}
            className="inline-flex h-9 shrink-0 items-center rounded-md border border-border-default bg-muted/40 p-0.5"
          >
            {(["monthly", "yearly"] as const).map((cadence) => (
              <button
                key={cadence}
                type="button"
                className={cn(
                  "h-8 min-w-[4.5rem] rounded px-3 text-xs font-medium transition-colors",
                  form.cadence === cadence
                    ? "bg-background text-foreground shadow-sm"
                    : "text-muted-foreground hover:text-foreground",
                )}
                onClick={() =>
                  setForm((current) => {
                    const maxDay =
                      cadence === "yearly"
                        ? daysInAnnualMonth(current.month).length
                        : 31;
                    return {
                      ...current,
                      cadence,
                      day: Math.min(current.day, maxDay),
                    };
                  })
                }
                disabled={mutation.isPending}
                aria-pressed={form.cadence === cadence}
              >
                {t(`settings.authCenter.subscriptionExpiry.${cadence}`)}
              </button>
            ))}
          </div>

          {form.cadence === "yearly" && (
            <>
              <Label className="sr-only">
                {t("settings.authCenter.subscriptionExpiry.month")}
              </Label>
              <Select
                value={String(form.month)}
                onValueChange={(value) =>
                  setForm((current) => {
                    const month = Number(value);
                    return {
                      ...current,
                      month,
                      day: Math.min(
                        current.day,
                        daysInAnnualMonth(month).length,
                      ),
                    };
                  })
                }
                disabled={mutation.isPending}
              >
                <SelectTrigger
                  className="h-9 w-[6.5rem] text-xs"
                  aria-label={t(
                    "settings.authCenter.subscriptionExpiry.month",
                  )}
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {MONTHS.map((month) => (
                    <SelectItem key={month} value={String(month)}>
                      {t("settings.authCenter.subscriptionExpiry.monthValue", {
                        month,
                      })}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </>
          )}

          {form.cadence && (
            <>
              <Label className="sr-only">
                {t("settings.authCenter.subscriptionExpiry.day")}
              </Label>
              <Select
                value={String(form.day)}
                onValueChange={(value) =>
                  setForm((current) => ({
                    ...current,
                    day: Number(value),
                  }))
                }
                disabled={mutation.isPending}
              >
                <SelectTrigger
                  className="h-9 w-[6.5rem] text-xs"
                  aria-label={t(
                    "settings.authCenter.subscriptionExpiry.day",
                  )}
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {(form.cadence === "yearly" ? yearlyDays : MONTHLY_DAYS).map(
                    (day) => (
                      <SelectItem key={day} value={String(day)}>
                        {t("settings.authCenter.subscriptionExpiry.dayValue", {
                          day,
                        })}
                      </SelectItem>
                    ),
                  )}
                </SelectContent>
              </Select>
            </>
          )}

          <span className="max-w-full truncate font-mono text-[11px] text-muted-foreground">
            {form.timeZone}
          </span>

          <TooltipProvider delayDuration={300}>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  type="button"
                  size="icon"
                  variant="outline"
                  className="h-9 w-9"
                  onClick={() => draft && mutation.mutate(draft)}
                  disabled={mutation.isPending || !hasDraft || !draft}
                  aria-label={t(
                    "settings.authCenter.subscriptionExpiry.save",
                  )}
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
                    if (hasPersistedManual) {
                      mutation.mutate(null);
                    } else {
                      setForm(formStateFromExpiry(null, null));
                    }
                  }}
                  disabled={
                    mutation.isPending || (!hasPersistedManual && !hasDraft)
                  }
                  aria-label={t(
                    "settings.authCenter.subscriptionExpiry.clear",
                  )}
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
