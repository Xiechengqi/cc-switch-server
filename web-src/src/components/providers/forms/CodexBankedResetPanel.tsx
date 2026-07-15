import React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  AlertTriangle,
  CheckCircle2,
  Clock3,
  Gift,
  Loader2,
  RefreshCw,
  RotateCcw,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { codexBankedResetApi } from "@/lib/api";
import type { CodexBankedResetCredit, CodexBankedResetStatus } from "@/lib/api";

interface CodexBankedResetPanelProps {
  accountId?: string | null;
  workspaceId?: string | null;
}

type TimestampValue = string | number | null | undefined;
type DetailsState = "fresh" | "stale" | "unavailable" | "unknown";

function resetQueryKey(
  accountId: string | null | undefined,
  workspaceId: string | null | undefined,
) {
  return [
    "codex_banked_reset",
    "status",
    accountId ?? "default-account",
    workspaceId ?? "default-workspace",
  ] as const;
}

export const CodexBankedResetPanel: React.FC<CodexBankedResetPanelProps> = ({
  accountId,
  workspaceId,
}) => {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const queryKey = React.useMemo(
    () => resetQueryKey(accountId, workspaceId),
    [accountId, workspaceId],
  );

  const statusQuery = useQuery({
    queryKey,
    queryFn: async () => {
      const status = await codexBankedResetApi.getCodexBankedResetStatus(
        accountId,
        false,
      );
      if (workspaceId && status.workspaceId !== workspaceId) {
        throw new Error(t("codexBankedReset.workspaceChanged"));
      }
      return status;
    },
    staleTime: 60_000,
    retry: false,
  });

  const refreshMutation = useMutation({
    mutationFn: async (target: {
      accountId: string | null | undefined;
      workspaceId: string | null | undefined;
    }) => {
      const status = await codexBankedResetApi.getCodexBankedResetStatus(
        target.accountId,
        true,
      );
      if (target.workspaceId && status.workspaceId !== target.workspaceId) {
        throw new Error(t("codexBankedReset.workspaceChanged"));
      }
      return status;
    },
    onSuccess: (status, target) => {
      queryClient.setQueryData(
        resetQueryKey(
          target.accountId,
          status.workspaceId ?? target.workspaceId,
        ),
        status,
      );
    },
  });

  const status = statusQuery.data;
  const sortedCredits = React.useMemo(
    () => sortCodexBankedResetCredits(status?.credits ?? []),
    [status?.credits],
  );
  const availableCount = displayAvailableCount(status, sortedCredits);
  const detailsState = getDetailsState(status);
  const hasSplitFreshnessMetadata =
    status != null &&
    ("countFetchedAt" in status || "detailsFetchedAt" in status);
  const lastFetchedAt = hasSplitFreshnessMetadata
    ? latestTimestamp(status?.countFetchedAt, status?.detailsFetchedAt)
    : latestTimestamp(status?.queriedAt);
  const nextExpiresAt =
    status?.nextExpiresAt ??
    sortedCredits.find(
      (credit) => normalizeCreditStatus(credit.status) === "available",
    )?.expiresAt;
  const refreshTargetsCurrentAccount =
    (refreshMutation.variables?.accountId ?? null) === (accountId ?? null) &&
    (refreshMutation.variables?.workspaceId ?? null) === (workspaceId ?? null);
  const isRefreshing =
    statusQuery.isFetching ||
    (refreshMutation.isPending && refreshTargetsCurrentAccount);
  const refreshError =
    (refreshTargetsCurrentAccount ? refreshMutation.error : null) ??
    (status ? statusQuery.error : null);

  return (
    <section className="space-y-3 rounded-md border border-border-default bg-muted/20 p-3">
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 items-start gap-2">
          <div className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-md bg-emerald-500 text-white">
            <Gift className="h-4 w-4" />
          </div>
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <Label className="text-sm font-medium">
                {t("codexBankedReset.title")}
              </Label>
              <span className="rounded-full bg-muted px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
                {t("codexBankedReset.readOnly")}
              </span>
            </div>
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
          onClick={() => refreshMutation.mutate({ accountId, workspaceId })}
          disabled={isRefreshing}
          title={t("common.refresh")}
          aria-label={t("common.refresh")}
        >
          <RefreshCw
            className={`h-4 w-4 ${isRefreshing ? "animate-spin" : ""}`}
          />
        </Button>
      </div>

      {statusQuery.isLoading ? (
        <div className="flex items-center justify-center gap-2 py-5 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          {t("common.loading")}
        </div>
      ) : !status && statusQuery.error ? (
        <ErrorNotice error={statusQuery.error} />
      ) : status ? (
        <div className="space-y-3">
          {status.enabled === false && (
            <Notice tone="warning">{t("codexBankedReset.disabled")}</Notice>
          )}

          {refreshError && (
            <Notice tone="error">
              {t("codexBankedReset.refreshFailed")}:{" "}
              {errorMessage(refreshError)}
            </Notice>
          )}

          {status.detailsError && (
            <Notice tone="warning">
              {t("codexBankedReset.detailsError")}: {status.detailsError}
            </Notice>
          )}

          <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
            <div className="rounded-md border border-border-default bg-background p-3">
              <div className="flex items-center justify-between gap-3">
                <div>
                  <div className="text-xs text-muted-foreground">
                    {t("codexBankedReset.available")}
                  </div>
                  <div className="mt-1 flex items-end gap-1">
                    <span className="text-3xl font-semibold leading-none">
                      {availableCount ?? "—"}
                    </span>
                    <span className="text-xs text-muted-foreground">
                      {t("codexBankedReset.availableUnit")}
                    </span>
                  </div>
                  {availableCount == null && (
                    <p className="mt-2 text-xs text-amber-700 dark:text-amber-300">
                      {t("codexBankedReset.unknownCount")}
                    </p>
                  )}
                </div>
                <RotateCcw className="h-5 w-5 text-emerald-600" />
              </div>
            </div>

            <div className="rounded-md border border-border-default bg-background p-3">
              <div className="flex items-center justify-between gap-3">
                <div>
                  <div className="text-xs text-muted-foreground">
                    {t("codexBankedReset.detailsTitle")}
                  </div>
                  <div className="mt-1 flex items-center gap-2 text-sm font-medium">
                    <DetailsStateIcon state={detailsState} />
                    {t(`codexBankedReset.details${capitalize(detailsState)}`)}
                  </div>
                </div>
                <div className="text-right text-xs text-muted-foreground">
                  <div>{t("codexBankedReset.lastFetchedAt")}</div>
                  <div className="mt-1 font-mono text-foreground">
                    {formatDate(lastFetchedAt, i18n.language) ?? "—"}
                  </div>
                </div>
              </div>
            </div>
          </div>

          <dl className="grid grid-cols-1 gap-x-4 gap-y-2 rounded-md border border-border-default bg-background p-3 text-xs sm:grid-cols-2">
            <MetadataItem
              label={t("codexBankedReset.workspace")}
              value={status.workspaceId ?? workspaceId ?? null}
            />
            <MetadataItem
              label={t("codexBankedReset.countSource")}
              value={formatSource(status.countSource ?? status.source)}
            />
            <MetadataItem
              label={t("codexBankedReset.detailsSource")}
              value={formatSource(status.detailsSource ?? status.source)}
            />
            <MetadataItem
              label={t("codexBankedReset.countFetchedAt")}
              value={formatDate(status.countFetchedAt, i18n.language)}
            />
            <MetadataItem
              label={t("codexBankedReset.detailsFetchedAt")}
              value={formatDate(status.detailsFetchedAt, i18n.language)}
            />
            <MetadataItem
              label={t("codexBankedReset.nextExpiresAt")}
              value={formatDate(nextExpiresAt, i18n.language)}
            />
          </dl>

          <div className="space-y-2">
            <div className="flex items-center justify-between gap-2">
              <Label className="text-xs text-muted-foreground">
                {t("codexBankedReset.creditsTitle")}
              </Label>
              <span className="text-xs tabular-nums text-muted-foreground">
                {sortedCredits.length}
              </span>
            </div>

            {sortedCredits.length > 0 ? (
              <div className="space-y-2">
                {sortedCredits.map((credit, index) => {
                  const normalizedStatus = normalizeCreditStatus(credit.status);
                  return (
                    <article
                      key={credit.id || `${normalizedStatus}-${index}`}
                      className="rounded-md border border-border-default bg-background p-3"
                    >
                      <div className="flex flex-wrap items-start justify-between gap-2">
                        <div className="min-w-0 text-sm font-medium">
                          {credit.title ||
                            `${t("codexBankedReset.creditFallbackTitle")} #${index + 1}`}
                        </div>
                        <CreditStatus status={normalizedStatus} />
                      </div>
                      <dl className="mt-3 grid grid-cols-1 gap-2 text-xs sm:grid-cols-2">
                        <CreditTime
                          label={t("codexBankedReset.creditGrantedAt")}
                          value={credit.grantedAt}
                          locale={i18n.language}
                        />
                        <CreditTime
                          label={t("codexBankedReset.creditExpiresAt")}
                          value={credit.expiresAt}
                          locale={i18n.language}
                        />
                      </dl>
                    </article>
                  );
                })}
              </div>
            ) : (
              <p className="rounded-md border border-dashed border-border-default bg-background px-3 py-4 text-sm text-muted-foreground">
                {availableCount == null
                  ? t("codexBankedReset.allUnavailableHint")
                  : availableCount > 0
                    ? t("codexBankedReset.detailsUnavailableHint")
                    : t("codexBankedReset.noCredits")}
              </p>
            )}
          </div>
        </div>
      ) : null}
    </section>
  );
};

function Notice({
  children,
  tone,
}: {
  children: React.ReactNode;
  tone: "warning" | "error";
}) {
  const classes =
    tone === "error"
      ? "border-destructive/30 bg-destructive/5 text-destructive"
      : "border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300";
  return (
    <div
      className={`break-words rounded-md border px-3 py-2 text-xs ${classes}`}
    >
      {children}
    </div>
  );
}

function ErrorNotice({ error }: { error: unknown }) {
  return (
    <div className="break-words rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
      {errorMessage(error)}
    </div>
  );
}

function MetadataItem({
  label,
  value,
}: {
  label: string;
  value: string | null;
}) {
  return (
    <div className="flex min-w-0 items-baseline justify-between gap-3">
      <dt className="shrink-0 text-muted-foreground">{label}</dt>
      <dd
        className="min-w-0 truncate font-mono text-foreground"
        title={value ?? "—"}
      >
        {value ?? "—"}
      </dd>
    </div>
  );
}

function CreditTime({
  label,
  value,
  locale,
}: {
  label: string;
  value: TimestampValue;
  locale: string;
}) {
  return (
    <div className="flex min-w-0 items-start gap-2 rounded bg-muted/50 px-2 py-1.5">
      <Clock3 className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
      <div className="min-w-0">
        <dt className="text-muted-foreground">{label}</dt>
        <dd className="mt-0.5 break-words font-mono text-foreground">
          {formatDate(value, locale) ?? "—"}
        </dd>
      </div>
    </div>
  );
}

function DetailsStateIcon({ state }: { state: DetailsState }) {
  if (state === "fresh") {
    return <CheckCircle2 className="h-4 w-4 text-emerald-600" />;
  }
  if (state === "stale" || state === "unavailable") {
    return <AlertTriangle className="h-4 w-4 text-amber-600" />;
  }
  return <Clock3 className="h-4 w-4 text-muted-foreground" />;
}

function CreditStatus({ status }: { status: NormalizedCreditStatus }) {
  const { t } = useTranslation();
  const styles: Record<NormalizedCreditStatus, string> = {
    available: "bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
    redeeming: "bg-blue-500/10 text-blue-700 dark:text-blue-300",
    redeemed: "bg-muted text-muted-foreground",
    unknown: "bg-amber-500/10 text-amber-700 dark:text-amber-300",
  };
  return (
    <span
      className={`shrink-0 rounded-full px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide ${styles[status]}`}
    >
      {t(`codexBankedReset.status${capitalize(status)}`)}
    </span>
  );
}

type NormalizedCreditStatus =
  "available" | "redeeming" | "redeemed" | "unknown";

function normalizeCreditStatus(
  status: string | null | undefined,
): NormalizedCreditStatus {
  const normalized = status?.trim().toLowerCase();
  if (
    normalized === "available" ||
    normalized === "redeeming" ||
    normalized === "redeemed"
  ) {
    return normalized;
  }
  return "unknown";
}

export function sortCodexBankedResetCredits(
  credits: CodexBankedResetCredit[],
): CodexBankedResetCredit[] {
  const statusRank: Record<NormalizedCreditStatus, number> = {
    available: 0,
    redeeming: 1,
    redeemed: 2,
    unknown: 3,
  };

  return [...credits].sort((left, right) => {
    const statusDifference =
      statusRank[normalizeCreditStatus(left.status)] -
      statusRank[normalizeCreditStatus(right.status)];
    if (statusDifference !== 0) return statusDifference;

    const leftExpiry = timestampMs(left.expiresAt) ?? Number.POSITIVE_INFINITY;
    const rightExpiry =
      timestampMs(right.expiresAt) ?? Number.POSITIVE_INFINITY;
    if (leftExpiry !== rightExpiry) return leftExpiry - rightExpiry;

    return left.id.localeCompare(right.id);
  });
}

function displayAvailableCount(
  status: CodexBankedResetStatus | undefined,
  credits: CodexBankedResetCredit[],
): number | null {
  if (!status) return null;
  const explicitCount = finiteNonNegativeInteger(status.availableCount);
  if (explicitCount != null) return explicitCount;

  if (status.countSource?.toLowerCase().includes("derived")) {
    return credits.filter(
      (credit) => normalizeCreditStatus(credit.status) === "available",
    ).length;
  }
  return null;
}

function finiteNonNegativeInteger(value: unknown): number | null {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) {
    return null;
  }
  return Math.trunc(value);
}

function getDetailsState(
  status: CodexBankedResetStatus | undefined,
): DetailsState {
  if (status?.detailsStale === true) return "stale";
  if (status?.detailsAvailable === false) return "unavailable";
  if (status?.detailsAvailable === true) return "fresh";
  return "unknown";
}

function capitalize<T extends string>(value: T): Capitalize<T> {
  return `${value.charAt(0).toUpperCase()}${value.slice(1)}` as Capitalize<T>;
}

function formatSource(value: string | null | undefined): string | null {
  const source = value?.trim();
  if (!source) return null;
  return source.replace(/[_-]+/g, " ");
}

function latestTimestamp(...values: TimestampValue[]): number | null {
  const timestamps = values
    .map(timestampMs)
    .filter((value): value is number => value != null);
  return timestamps.length > 0 ? Math.max(...timestamps) : null;
}

function timestampMs(value: TimestampValue): number | null {
  if (value == null) return null;
  const text = String(value).trim();
  if (!text) return null;

  const numeric = Number(text);
  if (Number.isFinite(numeric)) {
    const milliseconds =
      Math.abs(numeric) > 10_000_000_000 ? numeric : numeric * 1000;
    return Number.isFinite(milliseconds) ? milliseconds : null;
  }

  const parsed = Date.parse(text);
  return Number.isNaN(parsed) ? null : parsed;
}

function formatDate(value: TimestampValue, locale: string): string | null {
  const milliseconds = timestampMs(value);
  if (milliseconds == null) return null;
  return new Intl.DateTimeFormat(locale, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(new Date(milliseconds));
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export default CodexBankedResetPanel;
