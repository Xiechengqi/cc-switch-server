import React from "react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { formatRelativeTime } from "@/components/SubscriptionQuotaFooter";
import { ProviderQuotaMetaRow } from "@/components/providers/ProviderQuotaMetaRow";
import type {
  CodingPlanQuotaSnapshot,
  CodingPlanQuotaState,
  CodingPlanQuotaWindow,
  ProviderResource,
} from "@/lib/api/providers";
import {
  useCodingPlanQuota,
  useRefreshCodingPlanQuota,
} from "@/lib/query/codingPlanQuota";
import { cn } from "@/lib/utils";
import { extractErrorMessage } from "@/utils/errorUtils";

interface CodingPlanQuotaFooterProps {
  resource: ProviderResource;
  inline?: boolean;
}

const WINDOW_LABEL_KEYS: Record<CodingPlanQuotaWindow["kind"], string> = {
  five_hour: "provider.codingPlanQuota.fiveHour",
  weekly: "provider.codingPlanQuota.weekly",
  monthly: "provider.codingPlanQuota.monthly",
};

const STATE_LABEL_KEYS: Record<CodingPlanQuotaState, string> = {
  supported: "provider.codingPlanQuota.supported",
  stale: "provider.codingPlanQuota.stale",
  unknown: "provider.codingPlanQuota.unknown",
  unavailable: "provider.codingPlanQuota.unavailable",
};

const STATE_CLASSES: Record<CodingPlanQuotaState, string> = {
  supported: "text-green-600 dark:text-green-400",
  stale: "text-amber-600 dark:text-amber-400",
  unknown: "text-amber-600 dark:text-amber-400",
  unavailable: "text-muted-foreground",
};

function compactNumber(value: number): string {
  return new Intl.NumberFormat("en-US", {
    maximumFractionDigits: value % 1 === 0 ? 0 : 2,
    useGrouping: false,
  }).format(value);
}

function resetCountdown(
  resetsAtMs: number | undefined,
  nowMs: number,
): string | null {
  if (!Number.isFinite(resetsAtMs) || resetsAtMs === undefined) return null;
  const remainingMinutes = Math.floor((resetsAtMs - nowMs) / 60_000);
  if (remainingMinutes <= 0) return null;
  if (remainingMinutes < 60) return `${remainingMinutes}m`;
  const hours = Math.floor(remainingMinutes / 60);
  if (hours < 24) return `${hours}h${remainingMinutes % 60}m`;
  return `${Math.floor(hours / 24)}d${hours % 24}h`;
}

export function formatCodingPlanQuotaWindow(
  window: CodingPlanQuotaWindow,
  t: TFunction,
  nowMs = Date.now(),
): string {
  const baseLabel = t(WINDOW_LABEL_KEYS[window.kind]);
  const scope = window.scope?.trim().replace(/_/g, " ");
  const label = scope ? `${baseLabel} (${scope})` : baseLabel;
  const amount =
    typeof window.used === "number" &&
    Number.isFinite(window.used) &&
    typeof window.limit === "number" &&
    Number.isFinite(window.limit)
      ? `${compactNumber(window.used)}/${compactNumber(window.limit)}${
          window.unit?.trim() ? ` ${window.unit.trim()}` : ""
        }`
      : null;
  const utilization = `${Math.round(window.utilization)}%`;
  const countdown = resetCountdown(window.resetsAtMs, nowMs);
  return [
    label,
    amount,
    utilization,
    countdown
      ? t("provider.codingPlanQuota.resetsIn", { time: countdown })
      : null,
  ]
    .filter(Boolean)
    .join(" ");
}

export function formatCodingPlanQuotaSummary(
  snapshot: CodingPlanQuotaSnapshot,
  t: TFunction,
  nowMs = Date.now(),
): string {
  const { quota } = snapshot;
  if (quota.state === "unknown" || quota.state === "unavailable") {
    return quota.reason?.trim() || t(STATE_LABEL_KEYS[quota.state]);
  }
  const windows = quota.windows.map((window) =>
    formatCodingPlanQuotaWindow(window, t, nowMs),
  );
  return [
    quota.plan?.trim(),
    ...windows,
    windows.length === 0 ? t("provider.codingPlanQuota.quotaAvailable") : null,
  ]
    .filter(Boolean)
    .join(" · ");
}

export function CodingPlanQuotaFooter({
  resource,
  inline = false,
}: CodingPlanQuotaFooterProps) {
  const { t } = useTranslation();
  const hasContract = Boolean(resource.runtime?.codingPlan);
  const query = useCodingPlanQuota(resource, hasContract);
  const refresh = useRefreshCodingPlanQuota(resource);
  const [now, setNow] = React.useState(Date.now());
  const snapshot = query.data;

  React.useEffect(() => {
    if (
      !snapshot?.quota.observedAtMs &&
      !snapshot?.quota.windows.some((item) => item.resetsAtMs)
    ) {
      return;
    }
    const interval = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(interval);
  }, [snapshot]);

  if (!hasContract) return null;

  const loading = query.isFetching || refresh.isPending;
  const state = snapshot?.quota.state ?? "unknown";
  const stateLabel = query.isPending
    ? t("provider.codingPlanQuota.loading")
    : query.isError
      ? t("provider.codingPlanQuota.unknown")
      : t(STATE_LABEL_KEYS[state]);
  const timeLabel = snapshot?.quota.observedAtMs
    ? formatRelativeTime(snapshot.quota.observedAtMs, now, t)
    : t("provider.quotaNeverUpdated", { defaultValue: "Never updated" });
  const summary = snapshot
    ? formatCodingPlanQuotaSummary(snapshot, t, now)
    : query.isError
      ? t("provider.codingPlanQuota.queryFailed")
      : t("provider.codingPlanQuota.loading");
  const detail = [
    t(`provider.codingPlanQuota.source.${snapshot?.source ?? "contract"}`),
    snapshot?.quota.reason,
  ]
    .filter(Boolean)
    .join(" · ");

  const handleRefresh = () => {
    void refresh.mutateAsync().catch((error) => {
      toast.error(
        extractErrorMessage(error) || t("provider.codingPlanQuota.queryFailed"),
      );
    });
  };

  return (
    <div
      className={cn(
        "flex min-w-0 flex-col items-end gap-1",
        inline ? "max-w-full text-xs" : "w-full border-t border-border pt-3",
      )}
      title={detail || undefined}
      data-coding-plan-quota-state={state}
    >
      <ProviderQuotaMetaRow
        timeLabel={timeLabel}
        loading={loading}
        onRefresh={(event) => {
          event.stopPropagation();
          handleRefresh();
        }}
        refreshTitle={t("provider.codingPlanQuota.refresh")}
        leading={
          <span
            className={cn("text-[10px] font-semibold", STATE_CLASSES[state])}
          >
            {stateLabel}
          </span>
        }
      />
      <div className="min-w-0 max-w-full text-right text-[10px] font-medium text-foreground break-words">
        {summary}
      </div>
    </div>
  );
}

export default CodingPlanQuotaFooter;
