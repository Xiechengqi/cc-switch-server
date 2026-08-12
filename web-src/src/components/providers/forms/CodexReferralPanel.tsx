import React from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Gift, Loader2, MailPlus, RefreshCw, Send, Users } from "lucide-react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { EmailTagsInput } from "@/components/ui/tags-input";
import {
  codexReferralsApi,
  type CodexReferralEligibility,
  type CodexReferralGrant,
} from "@/lib/api";

interface CodexReferralPanelProps {
  providerId: string;
  expectedRevision: number;
}

function referralQueryKey(providerId: string, expectedRevision: number) {
  return ["codex-referrals", providerId, expectedRevision] as const;
}

function responseError(
  response: {
    ok: boolean;
    statusCode: number;
    upstreamMessage?: string | null;
    diagnostic?: string | null;
    challenged: boolean;
  },
  fallback: string,
): Error | null {
  if (response.ok) return null;
  const message =
    response.upstreamMessage ||
    response.diagnostic ||
    (response.challenged ? "Cloudflare challenge" : null) ||
    `${fallback} (${response.statusCode})`;
  return new Error(message);
}

function grantLabel(grant: CodexReferralGrant): string {
  const amount = grant.amount == null ? "" : String(grant.amount);
  return [amount, grant.grantType, grant.recipient].filter(Boolean).join(" ");
}

export function formatReferralDate(
  value: string | null | undefined,
  language: string,
): string | null {
  if (!value) return null;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return null;
  try {
    return new Intl.DateTimeFormat(language, {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(date);
  } catch {
    return null;
  }
}

export const CodexReferralPanel: React.FC<CodexReferralPanelProps> = ({
  providerId,
  expectedRevision,
}) => {
  const { t, i18n } = useTranslation();
  const [emails, setEmails] = React.useState<string[]>([]);
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  const target = React.useMemo(
    () => ({ providerId, expectedRevision }),
    [expectedRevision, providerId],
  );
  const queryKey = React.useMemo(
    () => referralQueryKey(providerId, expectedRevision),
    [expectedRevision, providerId],
  );

  const eligibilityQuery = useQuery({
    queryKey: [...queryKey, "eligibility"],
    queryFn: async () => {
      const response = await codexReferralsApi.getEligibility(target);
      const error = responseError(
        response,
        t("codexReferrals.loadEligibilityFailed"),
      );
      if (error) throw error;
      return response;
    },
    staleTime: 60_000,
    retry: false,
  });

  const trackingQuery = useQuery({
    queryKey: [...queryKey, "tracking"],
    queryFn: async () => {
      const response = await codexReferralsApi.getTracking(target);
      const error = responseError(
        response,
        t("codexReferrals.loadTrackingFailed"),
      );
      if (error) throw error;
      return response;
    },
    enabled: eligibilityQuery.isSuccess,
    staleTime: 60_000,
    retry: false,
  });

  const sendMutation = useMutation({
    mutationFn: async (recipients: string[]) => {
      const response = await codexReferralsApi.send(target, recipients);
      const error = responseError(response, t("codexReferrals.sendFailed"));
      if (error) throw error;
      return response;
    },
    onSuccess: async (response) => {
      const failed = new Set(
        response.failedEmails.map((email) => email.toLowerCase()),
      );
      setEmails((current) =>
        current.filter((email) => failed.has(email.toLowerCase())),
      );
      toast.success(
        t("codexReferrals.sendSuccess", { count: response.emails.length }),
      );
      await eligibilityQuery.refetch();
      await trackingQuery.refetch();
    },
    onError: (error) => {
      toast.error(error instanceof Error ? error.message : String(error));
    },
  });

  const refresh = async () => {
    const eligibility = await eligibilityQuery.refetch();
    if (eligibility.isSuccess) await trackingQuery.refetch();
  };
  const eligibility = eligibilityQuery.data;
  const sendCapacity = eligibility?.remainingSendCapacity;
  const canSend =
    eligibility?.shouldShow === true &&
    emails.length > 0 &&
    emails.length <= 10 &&
    (sendCapacity == null || sendCapacity >= emails.length);
  const loading =
    eligibilityQuery.isLoading ||
    (eligibilityQuery.isSuccess && trackingQuery.isLoading);
  const refreshing = eligibilityQuery.isFetching || trackingQuery.isFetching;

  return (
    <div className="space-y-4">
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 items-start gap-2">
          <div className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-md bg-emerald-600 text-white">
            <Gift className="h-4 w-4" />
          </div>
          <div className="min-w-0">
            <Label className="text-sm font-medium">
              {eligibility?.title || t("codexReferrals.title")}
            </Label>
            <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
              {eligibility?.description || t("codexReferrals.description")}
            </p>
          </div>
        </div>
        <Button
          type="button"
          size="icon"
          variant="ghost"
          className="h-8 w-8 shrink-0"
          disabled={refreshing}
          title={t("common.refresh")}
          aria-label={t("common.refresh")}
          onClick={() => void refresh()}
        >
          <RefreshCw
            className={`h-4 w-4 ${refreshing ? "animate-spin" : ""}`}
          />
        </Button>
      </div>

      {loading ? (
        <div className="flex items-center justify-center gap-2 py-5 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          {t("common.loading")}
        </div>
      ) : eligibilityQuery.error ? (
        <div className="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
          {eligibilityQuery.error instanceof Error
            ? eligibilityQuery.error.message
            : String(eligibilityQuery.error)}
        </div>
      ) : eligibility ? (
        <>
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="rounded-md border border-border-default bg-background p-3">
              <div className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
                <span>{t("codexReferrals.sendCapacity")}</span>
                <MailPlus className="h-4 w-4" />
              </div>
              <div className="mt-2 text-2xl font-semibold">
                {eligibility.remainingSendCapacity ?? t("common.notSet")}
              </div>
            </div>
            <div className="rounded-md border border-border-default bg-background p-3">
              <div className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
                <span>{t("codexReferrals.rewardCapacity")}</span>
                <Users className="h-4 w-4" />
              </div>
              <div className="mt-2 text-2xl font-semibold">
                {eligibility.remainingRewardCapacity ?? t("common.notSet")}
              </div>
            </div>
          </div>

          {eligibility.grants.length ? (
            <div className="flex flex-wrap gap-2">
              {eligibility.grants.map((grant, index) => (
                <Badge
                  key={grant.rewardId || `${grantLabel(grant)}-${index}`}
                  variant="secondary"
                >
                  {grantLabel(grant) || t("codexReferrals.reward")}
                </Badge>
              ))}
            </div>
          ) : null}

          {eligibility.rules.length || eligibility.timeFrameRules.length ? (
            <div className="space-y-1 text-xs leading-relaxed text-muted-foreground">
              {eligibility.rules.map((rule) => (
                <p key={rule}>{rule}</p>
              ))}
              {eligibility.timeFrameRules.map((rule, index) => (
                <p key={`${rule.ruleType || "rule"}-${index}`}>
                  {t("codexReferrals.timeFrameRule", {
                    sent: rule.invitesSent ?? 0,
                    total: rule.invitesTotal ?? "?",
                    period: rule.timeFrame ?? "",
                  })}
                </p>
              ))}
            </div>
          ) : null}

          {!eligibility.shouldShow ? (
            <div className="rounded-md border border-amber-500/30 bg-amber-500/5 p-3 text-sm text-amber-700 dark:text-amber-300">
              {eligibility.ineligibleReason ||
                eligibility.upstreamMessage ||
                t("codexReferrals.ineligible")}
            </div>
          ) : (
            <div className="space-y-2">
              <Label htmlFor={`codex-referrals-${providerId}`}>
                {t("codexReferrals.recipients")}
              </Label>
              <EmailTagsInput
                inputId={`codex-referrals-${providerId}`}
                value={emails}
                onChange={(next) => setEmails(next.slice(0, 10))}
                disabled={sendMutation.isPending}
                invalid={emails.length > 10}
                placeholder={t("codexReferrals.recipientPlaceholder")}
                hidePlaceholderOnFocus
              />
              <div className="flex items-center justify-between gap-3">
                <span className="text-xs text-muted-foreground">
                  {t("codexReferrals.recipientLimit", {
                    count: emails.length,
                  })}
                </span>
                <Button
                  type="button"
                  size="sm"
                  disabled={!canSend || sendMutation.isPending}
                  onClick={() => setConfirmOpen(true)}
                >
                  {sendMutation.isPending ? (
                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  ) : (
                    <Send className="mr-2 h-4 w-4" />
                  )}
                  {t("codexReferrals.send")}
                </Button>
              </div>
            </div>
          )}

          <div className="space-y-2">
            <div className="flex items-center justify-between gap-2">
              <Label>{t("codexReferrals.tracking")}</Label>
              <span className="text-xs text-muted-foreground">
                {t("codexReferrals.pastDays", { count: 90 })}
              </span>
            </div>
            {trackingQuery.error ? (
              <div className="text-sm text-destructive">
                {trackingQuery.error instanceof Error
                  ? trackingQuery.error.message
                  : String(trackingQuery.error)}
              </div>
            ) : trackingQuery.data?.items.length ? (
              <div className="divide-y divide-border-default rounded-md border border-border-default">
                {trackingQuery.data.items.map((item, index) => (
                  <div
                    key={
                      item.referralId || `${item.email || "invite"}-${index}`
                    }
                    className="flex items-start justify-between gap-3 px-3 py-2"
                  >
                    <div className="min-w-0">
                      <div className="truncate text-sm font-medium">
                        {item.email || t("codexReferrals.unknownRecipient")}
                      </div>
                      <div className="mt-0.5 text-xs text-muted-foreground">
                        {formatReferralDate(item.createdAt, i18n.language) ??
                          t("common.notSet")}
                      </div>
                    </div>
                    <Badge variant="outline">
                      {item.status || t("common.unknown")}
                    </Badge>
                  </div>
                ))}
              </div>
            ) : (
              <div className="rounded-md border border-dashed border-border-default px-3 py-4 text-center text-sm text-muted-foreground">
                {t("codexReferrals.noTracking")}
              </div>
            )}
          </div>
        </>
      ) : null}

      <ConfirmDialog
        isOpen={confirmOpen}
        title={t("codexReferrals.confirmTitle")}
        message={t("codexReferrals.confirmMessage", { count: emails.length })}
        highlight={emails.join("\n")}
        confirmText={t("codexReferrals.send")}
        variant="info"
        onConfirm={async () => {
          setConfirmOpen(false);
          await sendMutation.mutateAsync(emails);
        }}
        onCancel={() => setConfirmOpen(false)}
      />
    </div>
  );
};

export default CodexReferralPanel;
