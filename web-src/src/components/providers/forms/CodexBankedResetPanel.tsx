import React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  ChevronDown,
  Gift,
  Loader2,
  Mail,
  RefreshCw,
  RotateCcw,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { codexBankedResetApi, subscriptionApi } from "@/lib/api";
import type { CodexBankedResetCredit } from "@/lib/api";

const MAX_EMAILS = 5;
const EMAIL_PATTERN = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

interface CodexBankedResetPanelProps {
  accountId?: string | null;
}

export const CodexBankedResetPanel: React.FC<CodexBankedResetPanelProps> = ({
  accountId,
}) => {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const [selectedCreditId, setSelectedCreditId] = React.useState<string>("");
  const [emailsInput, setEmailsInput] = React.useState("");
  const [consentConfirmed, setConsentConfirmed] = React.useState(false);
  const [rulesOpen, setRulesOpen] = React.useState(false);

  const queryKey = React.useMemo(
    () => ["codex_banked_reset", "status", accountId ?? "default"],
    [accountId],
  );

  const statusQuery = useQuery({
    queryKey,
    queryFn: () => codexBankedResetApi.getCodexBankedResetStatus(accountId),
    staleTime: 60_000,
    retry: false,
  });

  const availableCredits = React.useMemo(() => {
    return (statusQuery.data?.credits ?? []).filter((credit) => {
      const status = credit.status?.toLowerCase();
      return status === "available";
    });
  }, [statusQuery.data?.credits]);

  React.useEffect(() => {
    if (availableCredits.length === 0) {
      setSelectedCreditId("");
      return;
    }
    if (!availableCredits.some((credit) => credit.id === selectedCreditId)) {
      setSelectedCreditId(availableCredits[0].id);
    }
  }, [availableCredits, selectedCreditId]);

  const inviteMutation = useMutation({
    mutationFn: (emails: string[]) =>
      codexBankedResetApi.sendCodexBankedResetInvite(accountId, emails),
    onSuccess: (result) => {
      const failed = result.failedEmails.filter(Boolean);
      if (failed.length > 0) {
        toast.error(
          t("codexBankedReset.invitePartialFailed", {
            emails: failed.join(", "),
          }),
        );
        return;
      }
      setEmailsInput("");
      toast.success(
        result.message ||
          t("codexBankedReset.inviteSuccess", {
            defaultValue: "Invite sent",
          }),
      );
    },
    onError: (error) => {
      toast.error(error instanceof Error ? error.message : String(error));
    },
  });

  const consumeMutation = useMutation({
    mutationFn: (creditId: string) =>
      codexBankedResetApi.consumeCodexBankedReset(accountId, creditId),
    onSuccess: async (result) => {
      const success = result.code === "reset" || !result.code;
      toast[success ? "success" : "error"](consumeMessage(result.code));
      await queryClient.invalidateQueries({ queryKey });
      if (success) {
        await subscriptionApi.refreshOauthQuota(
          "codex_oauth",
          accountId ?? null,
        );
        await queryClient.invalidateQueries({
          queryKey: ["codex_oauth", "quota", accountId ?? "default"],
        });
      }
    },
    onError: (error) => {
      toast.error(error instanceof Error ? error.message : String(error));
    },
  });

  const parseEmails = React.useCallback(() => {
    const unique = [
      ...new Map(
        emailsInput
          .split(/[,\s;]+/)
          .map((email) => email.trim())
          .filter(Boolean)
          .map((email) => [email.toLowerCase(), email] as const),
      ).values(),
    ];

    if (unique.length === 0) {
      throw new Error(t("codexBankedReset.emailsRequired"));
    }
    if (unique.length > MAX_EMAILS) {
      throw new Error(t("codexBankedReset.emailLimit", { max: MAX_EMAILS }));
    }
    const invalid = unique.find((email) => !EMAIL_PATTERN.test(email));
    if (invalid) {
      throw new Error(t("codexBankedReset.invalidEmail", { email: invalid }));
    }
    return unique;
  }, [emailsInput, t]);

  const handleInvite = () => {
    try {
      if ((statusQuery.data?.requiresConsent ?? true) && !consentConfirmed) {
        throw new Error(t("codexBankedReset.consentRequired"));
      }
      inviteMutation.mutate(parseEmails());
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    }
  };

  const handleConsume = () => {
    if (!selectedCreditId) return;
    const confirmed = window.confirm(t("codexBankedReset.consumeConfirm"));
    if (!confirmed) return;
    consumeMutation.mutate(selectedCreditId);
  };

  const selectedCredit = availableCredits.find(
    (credit) => credit.id === selectedCreditId,
  );
  const selectedCreditIndex = availableCredits.findIndex(
    (credit) => credit.id === selectedCreditId,
  );
  const selectedCreditTimeSummary = selectedCredit
    ? creditTimeSummary(selectedCredit, t, i18n.language)
    : null;
  const availableCount =
    statusQuery.data?.availableCount ?? availableCredits.length;

  const consumeMessage = (code?: string | null) => {
    if (code === "nothing_to_reset")
      return t("codexBankedReset.nothingToReset");
    if (code === "already_redeemed")
      return t("codexBankedReset.alreadyRedeemed");
    if (code === "no_credit") return t("codexBankedReset.noCredit");
    return t("codexBankedReset.consumeSuccess");
  };

  return (
    <section className="space-y-3 rounded-md border border-border-default bg-muted/20 p-3">
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 items-start gap-2">
          <div className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-md bg-emerald-500 text-white">
            <Gift className="h-4 w-4" />
          </div>
          <div className="min-w-0">
            <Label className="text-sm font-medium">
              {t("codexBankedReset.title")}
            </Label>
            <p className="mt-1 text-xs text-muted-foreground">
              {t("codexBankedReset.description")}
            </p>
          </div>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-8 w-8 shrink-0"
          onClick={() => statusQuery.refetch()}
          disabled={statusQuery.isFetching}
          title={t("common.refresh")}
        >
          <RefreshCw
            className={`h-4 w-4 ${statusQuery.isFetching ? "animate-spin" : ""}`}
          />
        </Button>
      </div>

      {statusQuery.isLoading ? (
        <div className="flex items-center justify-center gap-2 py-5 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          {t("common.loading")}
        </div>
      ) : statusQuery.error ? (
        <div className="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
          {statusQuery.error instanceof Error
            ? statusQuery.error.message
            : String(statusQuery.error)}
        </div>
      ) : (
        <>
          {statusQuery.data?.inviteEligibilityError && (
            <div className="rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-700 dark:text-amber-300">
              {t("codexBankedReset.inviteEligibilityUnavailable")}
            </div>
          )}

          <div className="grid grid-cols-1 gap-3 md:grid-cols-[minmax(0,0.9fr)_minmax(0,1.1fr)]">
            <div className="space-y-3 rounded-md border border-border-default bg-background p-3">
              <div className="flex items-center justify-between gap-3">
                <div>
                  <div className="text-xs text-muted-foreground">
                    {t("codexBankedReset.available")}
                  </div>
                  <div className="mt-1 flex items-end gap-1">
                    <span className="text-3xl font-semibold leading-none">
                      {availableCount}
                    </span>
                    <span className="text-xs text-muted-foreground">
                      {t("codexBankedReset.availableUnit")}
                    </span>
                  </div>
                </div>
                <RotateCcw className="h-5 w-5 text-emerald-600" />
              </div>

              {availableCredits.length > 0 ? (
                <div className="space-y-2">
                  <Label className="text-xs text-muted-foreground">
                    {t("codexBankedReset.selectedCredit")}
                  </Label>
                  <Select
                    value={selectedCreditId}
                    onValueChange={setSelectedCreditId}
                  >
                    <SelectTrigger>
                      <SelectValue
                        placeholder={t("codexBankedReset.selectCredit")}
                      >
                        {selectedCredit
                          ? creditLabel(selectedCredit, selectedCreditIndex, t)
                          : undefined}
                      </SelectValue>
                    </SelectTrigger>
                    <SelectContent
                      position="item-aligned"
                      className="w-[22rem] max-w-[calc(100vw-2rem)]"
                    >
                      {availableCredits.map((credit, index) => {
                        const summary = creditTimeSummary(
                          credit,
                          t,
                          i18n.language,
                        );
                        return (
                          <SelectItem
                            key={credit.id}
                            value={credit.id}
                            className="py-2"
                          >
                            <div className="flex min-w-0 flex-col gap-0.5">
                              <span className="truncate">
                                {creditLabel(credit, index, t)}
                              </span>
                              {summary && (
                                <span className="truncate text-xs text-muted-foreground">
                                  {summary}
                                </span>
                              )}
                            </div>
                          </SelectItem>
                        );
                      })}
                    </SelectContent>
                  </Select>
                  {selectedCredit && (
                    <div className="rounded-md bg-muted px-3 py-2 text-xs text-muted-foreground">
                      <p>
                        {selectedCredit.description ||
                          t("codexBankedReset.creditFallbackDescription")}
                      </p>
                      {selectedCreditTimeSummary && (
                        <p className="mt-1 font-mono">
                          {selectedCreditTimeSummary}
                        </p>
                      )}
                    </div>
                  )}
                </div>
              ) : (
                <p className="rounded-md border border-dashed border-border-default px-3 py-4 text-sm text-muted-foreground">
                  {t("codexBankedReset.noCredits")}
                </p>
              )}

              <Button
                type="button"
                className="w-full"
                onClick={handleConsume}
                disabled={!selectedCreditId || consumeMutation.isPending}
              >
                {consumeMutation.isPending ? (
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                ) : (
                  <RotateCcw className="mr-2 h-4 w-4" />
                )}
                {t("codexBankedReset.useReset")}
              </Button>
            </div>

            <div className="space-y-3 rounded-md border border-border-default bg-background p-3">
              <div className="space-y-2">
                <Label htmlFor="codex-banked-reset-emails" className="text-xs">
                  {t("codexBankedReset.inviteEmails")}
                </Label>
                <Textarea
                  id="codex-banked-reset-emails"
                  value={emailsInput}
                  onChange={(event) => setEmailsInput(event.target.value)}
                  rows={5}
                  className="font-mono text-xs"
                  placeholder={t("codexBankedReset.emailPlaceholder")}
                />
                <p className="text-xs text-muted-foreground">
                  {t("codexBankedReset.emailHint", { max: MAX_EMAILS })}
                </p>
              </div>

              <label className="flex items-start gap-2 rounded-md border border-border-default bg-muted/20 p-2 text-xs text-muted-foreground">
                <Checkbox
                  checked={consentConfirmed}
                  onCheckedChange={(checked) =>
                    setConsentConfirmed(checked === true)
                  }
                  className="mt-0.5"
                />
                <span>{t("codexBankedReset.consent")}</span>
              </label>

              <Button
                type="button"
                variant="outline"
                className="w-full"
                onClick={handleInvite}
                disabled={inviteMutation.isPending}
              >
                {inviteMutation.isPending ? (
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                ) : (
                  <Mail className="mr-2 h-4 w-4" />
                )}
                {t("codexBankedReset.sendInvite")}
              </Button>
            </div>
          </div>

          <button
            type="button"
            className="flex w-full items-center justify-between rounded-md border border-border-default bg-background px-3 py-2 text-left text-xs text-muted-foreground hover:bg-muted/40"
            onClick={() => setRulesOpen((open) => !open)}
          >
            <span>{t("codexBankedReset.rules")}</span>
            <ChevronDown
              className={`h-4 w-4 transition-transform ${rulesOpen ? "rotate-180" : ""}`}
            />
          </button>
          {rulesOpen && (
            <div className="rounded-md bg-background p-3 text-xs text-muted-foreground">
              {(statusQuery.data?.eligibilityRules ?? []).length > 0 ? (
                <ul className="list-disc space-y-1 pl-5">
                  {statusQuery.data?.eligibilityRules.map((rule) => (
                    <li key={rule}>{rule}</li>
                  ))}
                </ul>
              ) : (
                <p>{t("codexBankedReset.rulesEmpty")}</p>
              )}
            </div>
          )}
        </>
      )}
    </section>
  );
};

function creditLabel(
  credit: CodexBankedResetCredit,
  index: number,
  t: ReturnType<typeof useTranslation>["t"],
) {
  const safeIndex = index >= 0 ? index : 0;
  return `${credit.title || t("codexBankedReset.creditFallbackTitle")} #${
    safeIndex + 1
  }`;
}

function creditTimeSummary(
  credit: CodexBankedResetCredit,
  t: ReturnType<typeof useTranslation>["t"],
  locale: string,
) {
  const grantedAt = formatCreditDate(credit.grantedAt, locale);
  const expiresAt = formatCreditDate(credit.expiresAt, locale);
  const parts: string[] = [];
  if (grantedAt) {
    parts.push(`${t("codexBankedReset.creditGrantedAt")} ${grantedAt}`);
  }
  if (expiresAt) {
    parts.push(`${t("codexBankedReset.creditExpiresAt")} ${expiresAt}`);
  }
  return parts.length > 0 ? parts.join(" · ") : null;
}

function formatCreditDate(value: string | null | undefined, locale: string) {
  if (!value) return null;
  const text = String(value).trim();
  if (!text) return null;
  const numeric = Number(text);
  const date = Number.isFinite(numeric)
    ? new Date(numeric > 10_000_000_000 ? numeric : numeric * 1000)
    : new Date(text);
  if (Number.isNaN(date.getTime())) return null;
  return new Intl.DateTimeFormat(locale, {
    year: "2-digit",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  }).format(date);
}

export default CodexBankedResetPanel;
