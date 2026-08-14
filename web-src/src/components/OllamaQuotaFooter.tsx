import React from "react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { formatRelativeTime } from "@/components/SubscriptionQuotaFooter";
import { ProviderQuotaMetaRow } from "@/components/providers/ProviderQuotaMetaRow";
import type {
  OllamaCloudModelUsage,
  OllamaCloudSnapshot,
  OllamaCloudSnapshotStatus,
  OllamaCloudUsageWindow,
  ProviderResource,
} from "@/lib/api/providers";
import { useOllamaQuota, useRefreshOllamaQuota } from "@/lib/query/ollama";
import { cn } from "@/lib/utils";
import { extractErrorMessage } from "@/utils/errorUtils";

interface OllamaQuotaFooterProps {
  resource: ProviderResource;
  inline?: boolean;
}

const STATUS_CLASSES: Record<OllamaCloudSnapshotStatus, string> = {
  complete: "text-green-600 dark:text-green-400",
  partial: "text-amber-600 dark:text-amber-400",
  stale: "text-amber-600 dark:text-amber-400",
  error: "text-destructive",
  unconfigured: "text-muted-foreground",
};

function compactNumber(value: number): string {
  return new Intl.NumberFormat("en-US", {
    maximumFractionDigits: value % 1 === 0 ? 0 : 1,
    useGrouping: false,
  }).format(value);
}

function usageWindowLabel(
  window: OllamaCloudUsageWindow,
  t: TFunction,
): string {
  return `${t(`provider.ollama.${window.kind}`)} ${compactNumber(
    window.utilization,
  )}%`;
}

function accountLabel(snapshot: OllamaCloudSnapshot): string | null {
  const account = snapshot.account.data;
  if (!account) return null;
  const identities = [account.name, account.email].filter(
    (value, index, values): value is string =>
      Boolean(value) && values.indexOf(value) === index,
  );
  if (identities.length === 0) identities.push(account.id);
  return [account.plan, ...identities].filter(Boolean).join(" · ");
}

function firstSectionReason(snapshot: OllamaCloudSnapshot): string | null {
  return snapshot.account.reason || snapshot.usage.reason || null;
}

export function formatOllamaCloudSummary(
  snapshot: OllamaCloudSnapshot,
  t: TFunction,
): string {
  const identity = accountLabel(snapshot);
  const windows =
    snapshot.usage.data?.limits.map((window) => usageWindowLabel(window, t)) ??
    [];
  const cost = snapshot.usage.data?.activity?.cost;
  const usage = [
    ...windows,
    cost !== undefined ? t("provider.ollama.cost", { value: cost }) : undefined,
  ].filter(Boolean);
  const reason = firstSectionReason(snapshot);
  return [
    identity,
    ...usage,
    reason && (identity || usage.length > 0) ? reason : null,
    !identity && usage.length === 0 && reason
      ? reason
      : !identity && usage.length === 0
        ? t(`provider.ollama.${snapshot.status}`)
        : null,
  ]
    .filter(Boolean)
    .join(" · ");
}

function formatModelList(
  models: OllamaCloudModelUsage[],
  modelsTruncated: boolean,
  t: TFunction,
  visibleLimit = 3,
): string {
  const visible = models
    .slice(0, visibleLimit)
    .map((model) => `${model.name} ${model.requestCount}`);
  const hidden = Math.max(0, models.length - visible.length);
  if (hidden > 0 || modelsTruncated) {
    visible.push(
      t("provider.ollama.modelsMore", {
        count: hidden + (modelsTruncated ? 1 : 0),
      }),
    );
  }
  return visible.join(", ");
}

export function formatOllamaCloudModels(
  snapshot: OllamaCloudSnapshot,
  t: TFunction,
): string {
  return (
    snapshot.usage.data?.limits
      .map((window) => {
        const models = formatModelList(
          window.models,
          window.modelsTruncated,
          t,
        );
        return models
          ? `${t(`provider.ollama.${window.kind}`)}: ${models}`
          : null;
      })
      .filter(Boolean)
      .join(" · ") ?? ""
  );
}

function observedAt(snapshot?: OllamaCloudSnapshot): number | undefined {
  if (!snapshot) return undefined;
  const values = [
    snapshot.account.observedAtMs,
    snapshot.usage.observedAtMs,
  ].filter((value): value is number => typeof value === "number");
  return values.length > 0 ? Math.max(...values) : undefined;
}

const OllamaQuotaFooter: React.FC<OllamaQuotaFooterProps> = ({
  resource,
  inline = false,
}) => {
  const { t } = useTranslation();
  const query = useOllamaQuota(resource);
  const refresh = useRefreshOllamaQuota(resource);
  const [now, setNow] = React.useState(Date.now());
  const snapshot = query.data;
  const timestamp = observedAt(snapshot);

  React.useEffect(() => {
    if (!timestamp) return;
    const interval = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(interval);
  }, [timestamp]);

  const status = query.isError ? "error" : (snapshot?.status ?? "unconfigured");
  const loading = query.isFetching || refresh.isPending;
  const statusLabel = query.isPending
    ? t("provider.ollama.loading")
    : query.isError
      ? t("provider.ollama.error")
      : t(`provider.ollama.${status}`);
  const summary = snapshot
    ? formatOllamaCloudSummary(snapshot, t)
    : query.isError
      ? t("provider.ollama.queryFailed")
      : t("provider.ollama.loading");
  const models = snapshot ? formatOllamaCloudModels(snapshot, t) : "";
  const timeLabel = timestamp
    ? formatRelativeTime(timestamp, now, t)
    : t("provider.quotaNeverUpdated", { defaultValue: "Never updated" });
  const detail = snapshot
    ? [
        t(`provider.ollama.source.${snapshot.source}`),
        snapshot.account.reason,
        snapshot.usage.reason,
      ]
        .filter(Boolean)
        .join(" · ")
    : undefined;

  const handleRefresh = () => {
    void refresh.mutateAsync().catch((error) => {
      toast.error(
        extractErrorMessage(error) || t("provider.ollama.queryFailed"),
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
      data-ollama-cloud-status={query.isPending ? "loading" : status}
    >
      <ProviderQuotaMetaRow
        timeLabel={timeLabel}
        loading={loading}
        onRefresh={(event) => {
          event.stopPropagation();
          handleRefresh();
        }}
        refreshTitle={t("provider.ollama.refresh")}
        leading={
          <span
            className={cn("text-[10px] font-semibold", STATUS_CLASSES[status])}
          >
            {statusLabel}
          </span>
        }
      />
      <div className="min-w-0 max-w-full text-right text-[10px] font-medium text-foreground break-words">
        {summary}
      </div>
      {models && (
        <div
          className="min-w-0 max-w-full text-right text-[10px] text-muted-foreground break-words"
          title={models}
        >
          {models}
        </div>
      )}
    </div>
  );
};

export default OllamaQuotaFooter;
