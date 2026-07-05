import { CSS } from "@dnd-kit/utilities";
import { useSortable } from "@dnd-kit/sortable";
import {
  BarChart3,
  CheckCircle2,
  Copy,
  FlaskConical,
  GripVertical,
  Link2,
  Loader2,
  Minus,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  ServerCog,
  Trash2,
} from "lucide-react";
import {
  CSSProperties,
  HTMLAttributes,
  ReactNode,
  useState,
} from "react";

import {
  AccountManagerCapability,
  AccountRecord,
  ProviderBreaker,
  ProviderHealth,
  ProviderLimitStatus,
  ProviderMatrixEntry,
  StoredProvider,
} from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { FailoverPriorityBadge } from "@/components/providers/FailoverPriorityBadge";
import { IconAction } from "@/components/IconAction";
import { KeyValue } from "@/components/KeyValue";
import { ProviderHealthIndicator } from "@/components/providers/ProviderHealthIndicator";
import { ProviderIcon } from "@/components/ProviderIcon";
import { StatusPill } from "@/components/StatusPill";
import {
  accountSummary,
  apiFormatFromProvider,
  baseUrlFromProvider,
  limitLine,
  modelFromProvider,
} from "@/components/providers/providerDisplay";
import { storedProviderIcon } from "@/lib/provider-icons";

type DragHandleProps = HTMLAttributes<HTMLButtonElement> & {
  ref?: (node: HTMLButtonElement | null) => void;
};
type TranslateFn = (
  key: string,
  vars?: Record<string, string | number | boolean | null | undefined>,
) => string;
type TxFn = (message: string, values?: Record<string, string | number | boolean | null | undefined>) => string;
type ProviderQuotaTier = NonNullable<NonNullable<AccountRecord["quota"]>["tiers"]>[number];

export function SortableProviderCard(props: ProviderCardProps) {
  const { attributes, listeners, setActivatorNodeRef, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: props.provider.provider.id });
  const style: CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
  };
  const dragHandleProps: DragHandleProps = {
    ...attributes,
    ...listeners,
    ref: setActivatorNodeRef,
  };
  return (
    <ProviderCard
      {...props}
      dragHandleProps={dragHandleProps}
      nodeRef={setNodeRef}
      style={style}
      dragging={isDragging}
    />
  );
}

interface ProviderCardProps {
  provider: StoredProvider;
  priority: number;
  failoverEnabled: boolean;
  failoverPriority: number | null;
  inFailoverQueue: boolean;
  entry?: ProviderMatrixEntry;
  health?: ProviderHealth;
  account?: AccountRecord;
  capability?: AccountManagerCapability;
  limit?: ProviderLimitStatus;
  breaker?: ProviderBreaker;
  current: boolean;
  result?: string;
  busyId: string | null;
  onEdit: () => void;
  onAction: (action: "test" | "network" | "stream" | "models" | "switch" | "duplicate" | "resetFailover" | "delete") => void;
  onToggleFailover: (enabled: boolean) => void;
  onOpenUsage?: () => void;
}

function ProviderCard({
  provider,
  priority,
  failoverEnabled,
  failoverPriority,
  inFailoverQueue,
  entry,
  health,
  account,
  capability,
  limit,
  breaker,
  current,
  result,
  busyId,
  onEdit,
  onAction,
  onToggleFailover,
  onOpenUsage,
  dragHandleProps,
  nodeRef,
  style,
  dragging,
}: ProviderCardProps & {
  dragHandleProps?: DragHandleProps;
  nodeRef?: (node: HTMLElement | null) => void;
  style?: CSSProperties;
  dragging?: boolean;
}) {
  const { tx } = useI18n();
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  const model = modelFromProvider(provider.provider);
  const baseUrl = baseUrlFromProvider(provider.provider, provider.app);
  const providerIcon = storedProviderIcon(provider);
  const accountId = provider.provider.meta?.authBinding?.accountId;
  const accountValue = account
    ? accountSummary(account)
    : accountId || tx("direct config");
  const busyPrefix = `${provider.app}:${provider.provider.id}:`;
  const healthSummary = providerHealthSummary(health, tx);
  const breakerOpen = failoverEnabled && breaker != null && breaker.state !== "closed";
  return (
    <>
    <article
      ref={nodeRef}
      className={[current ? "provider-card current" : "provider-card", dragging ? "dragging" : ""]
        .filter(Boolean)
        .join(" ")}
      style={style}
    >
      <header className="provider-card-header">
        <div className="provider-card-title-row">
          <button
            {...dragHandleProps}
            className="provider-drag-handle"
            type="button"
            aria-label={tx("Drag provider")}
            title={tx("Drag provider")}
          >
            <GripVertical size={16} />
          </button>
          <div className="provider-icon-frame">
            <ProviderIcon
              icon={providerIcon.icon}
              name={provider.provider.name}
              color={providerIcon.color}
              size={22}
            />
          </div>
          <div className="provider-title-stack">
            <div className="provider-name-row">
              <h3>{provider.provider.name}</h3>
              <FailoverPriorityBadge priority={priority} />
              {failoverEnabled && inFailoverQueue && failoverPriority != null && (
                <StatusPill tone="success">{tx("failover P{{priority}}", { priority: failoverPriority })}</StatusPill>
              )}
              {breakerOpen && (
                <StatusPill tone={breaker.state === "open" ? "danger" : "warning"}>
                  {tx("breaker {{state}}", { state: breaker.state })}
                </StatusPill>
              )}
              {current && <StatusPill tone="success">{tx("current")}</StatusPill>}
              {account?.subscriptionLevel && (
                <StatusPill tone="success">{account.subscriptionLevel}</StatusPill>
              )}
            </div>
            <p>{entry?.label || provider.providerTypeId}</p>
          </div>
        </div>
        <div className="provider-card-right">
          <div className="provider-health-stack">
            <ProviderHealthIndicator health={health} />
            <span>{healthSummary}</span>
          </div>
          <button
            className="icon-button provider-card-refresh"
            type="button"
            onClick={() => onAction("test")}
            disabled={busyId === `${busyPrefix}test`}
            aria-label={tx("Refresh provider health")}
            title={tx("Refresh provider health")}
          >
            {busyId === `${busyPrefix}test` ? <Loader2 size={14} /> : <RefreshCw size={14} />}
          </button>
        </div>
      </header>
      {baseUrl && (
        <a className="provider-url-row" href={baseUrl} target="_blank" rel="noreferrer">
          <Link2 size={14} />
          <span>{baseUrl}</span>
        </a>
      )}
      <div className="provider-card-meta compact">
        <KeyValue label="model" value={model || "-"} />
        <KeyValue label="api format" value={apiFormatFromProvider(provider.provider) || "-"} />
        <KeyValue label="account" value={accountValue} />
        <KeyValue label="last status" value={health?.lastStatusCode || "-"} />
      </div>
      {entry && <ProviderReadinessPanel entry={entry} capability={capability} />}
      {account && <ProviderAccountFooter account={account} />}
      {limit && <ProviderLimitFooter limit={limit} />}
      <div className="provider-card-result">
        {result || health?.reason || tx("{{count}} recent requests", { count: health?.requests ?? 0 })}
      </div>
      <div className="provider-actions">
        <IconAction title="Edit" onClick={onEdit}>
          <Pencil size={15} />
        </IconAction>
        <IconAction
          title="Duplicate"
          onClick={() => onAction("duplicate")}
          busy={busyId === `${busyPrefix}duplicate`}
        >
          <Copy size={15} />
        </IconAction>
        <IconAction
          title="Config test"
          onClick={() => onAction("test")}
          busy={busyId === `${busyPrefix}test`}
        >
          <CheckCircle2 size={15} />
        </IconAction>
        <IconAction
          title="Network test"
          onClick={() => onAction("network")}
          busy={busyId === `${busyPrefix}network`}
        >
          <FlaskConical size={15} />
        </IconAction>
        <IconAction
          title="Stream test"
          onClick={() => onAction("stream")}
          busy={busyId === `${busyPrefix}stream`}
        >
          <RefreshCw size={15} />
        </IconAction>
        <IconAction
          title="Fetch models"
          onClick={() => onAction("models")}
          busy={busyId === `${busyPrefix}models`}
        >
          <ServerCog size={15} />
        </IconAction>
        {onOpenUsage && (
          <IconAction title="Usage and limits" onClick={onOpenUsage}>
            <BarChart3 size={15} />
          </IconAction>
        )}
        {failoverEnabled && (
          <IconAction
            title={inFailoverQueue ? "Remove from failover queue" : "Add to failover queue"}
            onClick={() => onToggleFailover(!inFailoverQueue)}
            busy={busyId === `${busyPrefix}failover`}
          >
            {inFailoverQueue ? <Minus size={15} /> : <Plus size={15} />}
          </IconAction>
        )}
        {breakerOpen && (
          <IconAction
            title="Reset failover breaker"
            onClick={() => onAction("resetFailover")}
            busy={busyId === `${busyPrefix}resetFailover`}
          >
            <RefreshCw size={15} />
          </IconAction>
        )}
        <button
          className={current ? "secondary-button compact current-action" : "primary-button compact"}
          type="button"
          onClick={() => onAction("switch")}
          disabled={current || busyId === `${busyPrefix}switch`}
          title={tx(current ? "current" : "switch")}
        >
          {current ? <CheckCircle2 size={15} /> : <Play size={15} />}
          <span>{tx(current ? "current" : "switch")}</span>
        </button>
        <IconAction
          title="Delete"
          disabledTitle="Current provider cannot be deleted"
          onClick={() => setDeleteConfirmOpen(true)}
          busy={busyId === `${busyPrefix}delete`}
          disabled={current}
          danger
        >
          <Trash2 size={15} />
        </IconAction>
      </div>
    </article>
      <ConfirmDialog
        isOpen={deleteConfirmOpen}
        title={tx("Delete provider")}
        message={tx("Delete provider {{name}}?", { name: provider.provider.name })}
        confirmText={tx("Delete")}
        onConfirm={() => {
          setDeleteConfirmOpen(false);
          onAction("delete");
        }}
        onCancel={() => setDeleteConfirmOpen(false)}
      />
    </>
  );
}

function providerHealthSummary(
  health: ProviderHealth | undefined,
  tx: TranslateFn,
): string {
  const requests = health?.requests ?? 0;
  if (!health?.lastRequestAtMs) {
    return tx("{{count}} recent requests", { count: requests });
  }
  return tx("{{time}} · {{count}} requests", {
    time: relativeRequestTime(health.lastRequestAtMs, tx),
    count: requests,
  });
}

function relativeRequestTime(
  value: number,
  tx: TranslateFn,
): string {
  const millis = value < 10_000_000_000 ? value * 1000 : value;
  const diff = Date.now() - millis;
  if (!Number.isFinite(diff) || diff < 0) return formatRequestTime(millis);
  if (diff < 60_000) return tx("just now");
  if (diff < 3_600_000) return tx("{{count}}m ago", { count: Math.max(1, Math.round(diff / 60_000)) });
  if (diff < 86_400_000) return tx("{{count}}h ago", { count: Math.max(1, Math.round(diff / 3_600_000)) });
  if (diff < 604_800_000) return tx("{{count}}d ago", { count: Math.max(1, Math.round(diff / 86_400_000)) });
  return formatRequestTime(millis);
}

function formatRequestTime(millis: number): string {
  const date = new Date(millis);
  if (Number.isNaN(date.getTime())) return "-";
  return new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" }).format(date);
}

function ProviderReadinessPanel({
  entry,
  capability,
}: {
  entry: ProviderMatrixEntry;
  capability?: AccountManagerCapability;
}) {
  const { tx } = useI18n();
  return (
    <details className="provider-readiness-panel">
      <summary>
        <span>{tx("Adapter readiness")}</span>
        <div className="provider-readiness-header">
          <StatusPill tone={entry.uiVisible ? "success" : "warning"}>
            {entry.visibility === "diagnostic_only" ? tx("diagnostic") : tx("creatable")}
          </StatusPill>
          <span>{entry.credentialMode}</span>
        </div>
      </summary>
      <div className="provider-readiness-body">
        <div className="provider-readiness-grid">
          <ReadinessFlag label="direct" enabled={entry.directConfigSupported} />
          <ReadinessFlag label="account" enabled={entry.accountSupported} />
          <ReadinessFlag label="managed" enabled={entry.managedAccountRecommended} />
          <ReadinessFlag label="refresh" enabled={capability?.supportsRefresh} />
          <ReadinessFlag label="quota" enabled={capability?.supportsQuota} />
          <ReadinessFlag label="plan" enabled={capability?.supportsRefreshPlan} />
        </div>
        <div className="provider-readiness-note">
          {capability?.serverNativeStage || capability?.status || "direct-config"}
          {entry.note ? ` · ${entry.note}` : ""}
        </div>
      </div>
    </details>
  );
}

function ReadinessFlag({ label, enabled }: { label: string; enabled?: boolean }) {
  const { tx } = useI18n();
  return (
    <span className={enabled ? "readiness-flag active" : "readiness-flag"}>
      {tx(label)}
    </span>
  );
}

function ProviderAccountFooter({ account }: { account: AccountRecord }) {
  const { tx } = useI18n();
  const quotaPercent = accountQuotaPercent(account);
  const tiers = account.quota?.tiers || [];
  const expiryLabel = providerExpiryLabel(account.expiresAt, tx);
  const refreshedLabel = account.quotaRefreshedAt == null
    ? null
    : tx("refreshed {{time}}", { time: formatRelativePast(account.quotaRefreshedAt, tx) });
  const nextRefreshLabel = providerCountdownLabel(account.quotaNextRefreshAt, tx, "refresh");
  return (
    <div className="provider-account-footer">
      <div className="provider-account-line">
        <span>{account.email || account.id}</span>
        <span>{account.subscriptionLevel || tx("account")}</span>
        <span>{quotaPercent == null ? tx("quota -") : `${quotaPercent.toFixed(1)}%`}</span>
        {expiryLabel && <span title={formatDateTime(account.expiresAt)}>{expiryLabel}</span>}
        {refreshedLabel && <span title={formatDateTime(account.quotaRefreshedAt)}>{refreshedLabel}</span>}
        {nextRefreshLabel && <span title={formatDateTime(account.quotaNextRefreshAt)}>{nextRefreshLabel}</span>}
      </div>
      {quotaPercent != null && (
        <div className="provider-quota-meter" aria-label={tx("quota")}>
          <span style={{ width: `${clampPercent(quotaPercent)}%` }} />
        </div>
      )}
      {tiers.length > 0 && (
        <div className="provider-quota-tiers">
          {tiers.slice(0, 3).map((tier) => (
            <div className="provider-quota-tier" key={tier.name}>
              <div>
                <strong>{tier.name}</strong>
                <span>{tierLine(tier, tx)}</span>
              </div>
              <div className="provider-quota-tier-meter">
                <span style={{ width: `${clampPercent(tier.utilization ?? 0)}%` }} />
              </div>
            </div>
          ))}
        </div>
      )}
      {account.lastRefreshError && <strong>{account.lastRefreshError}</strong>}
    </div>
  );
}


function accountQuotaPercent(account: AccountRecord): number | null {
  if (account.quotaPercent != null) return account.quotaPercent;
  const utilization = account.quota?.tiers?.find((tier) => tier.utilization != null)?.utilization;
  return utilization == null ? null : utilization;
}

function clampPercent(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(100, value));
}

function tierLine(tier: ProviderQuotaTier, tx: TxFn): string {
  const usage = tier.used != null && tier.limit != null
    ? `${formatCompactNumber(tier.used)}/${formatCompactNumber(tier.limit)}`
    : tier.utilization == null
      ? "-"
      : `${tier.utilization.toFixed(1)}%`;
  const unit = tier.unit ? ` ${tier.unit}` : "";
  const reset = tier.resetsAt == null ? "" : ` · ${providerCountdownLabel(tier.resetsAt, tx, "resets") || formatTime(tier.resetsAt)}`;
  return `${usage}${unit}${reset}`;
}

function providerExpiryLabel(value: number | null | undefined, tx: TxFn): string | null {
  const millis = normalizeTimestamp(value);
  if (millis == null) return tx("expires -");
  const delta = millis - Date.now();
  if (delta < 0) return tx("expired {{time}} ago", { time: formatDuration(Math.abs(delta), tx) });
  return tx("expires in {{time}}", { time: formatDuration(delta, tx) });
}

function providerCountdownLabel(value: number | null | undefined, tx: TxFn, label: string): string | null {
  const millis = normalizeTimestamp(value);
  if (millis == null) return null;
  const delta = millis - Date.now();
  if (delta < 0) return tx("{{label}} {{time}} ago", { label, time: formatDuration(Math.abs(delta), tx) });
  return tx("{{label}} in {{time}}", { label, time: formatDuration(delta, tx) });
}

function formatRelativePast(value: number | null | undefined, tx: TxFn): string {
  const millis = normalizeTimestamp(value);
  if (millis == null) return "-";
  const delta = Date.now() - millis;
  if (delta < 0) return tx("in {{time}}", { time: formatDuration(Math.abs(delta), tx) });
  return tx("{{time}} ago", { time: formatDuration(delta, tx) });
}

function formatDuration(millis: number, tx: TxFn): string {
  const seconds = Math.max(0, Math.round(millis / 1000));
  const minutes = Math.floor(seconds / 60);
  const hours = Math.floor(minutes / 60);
  const days = Math.floor(hours / 24);
  if (days > 0) return tx("{{count}}d", { count: days });
  if (hours > 0) return tx("{{count}}h", { count: hours });
  if (minutes > 0) return tx("{{count}}m", { count: minutes });
  return tx("{{count}}s", { count: seconds });
}

function normalizeTimestamp(value: number | null | undefined): number | null {
  if (value == null || !Number.isFinite(value)) return null;
  return value < 10_000_000_000 ? value * 1000 : value;
}

function formatDateTime(value: number | null | undefined): string {
  const millis = normalizeTimestamp(value);
  if (millis == null) return "-";
  const date = new Date(millis);
  if (Number.isNaN(date.getTime())) return "-";
  return date.toISOString();
}

function formatCompactNumber(value: number): string {
  if (!Number.isFinite(value)) return "-";
  if (Math.abs(value) >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}m`;
  if (Math.abs(value) >= 1_000) return `${(value / 1_000).toFixed(1)}k`;
  return Number.isInteger(value) ? String(value) : value.toFixed(1);
}

function ProviderLimitFooter({ limit }: { limit: ProviderLimitStatus }) {
  const shareWarnings = limit.shares.filter((share) => share.blocked || share.warnings.length);
  const warnings = [...limit.warnings, ...shareWarnings.flatMap((share) => share.warnings.map((warning) => `${share.shareName}: ${warning}`))];
  return (
    <div className="provider-limit-footer">
      <div className="provider-limit-grid">
        <LimitMetric
          label="daily"
          value={limitLine(limit.dailyUsageUsd, limit.dailyLimitUsd)}
          tone={limit.dailyExceeded ? "danger" : "success"}
        />
        <LimitMetric
          label="monthly"
          value={limitLine(limit.monthlyUsageUsd, limit.monthlyLimitUsd)}
          tone={limit.monthlyExceeded ? "danger" : "success"}
        />
        <LimitMetric
          label="quota"
          value={limit.accountQuotaPercent == null ? "-" : `${limit.accountQuotaPercent.toFixed(1)}%`}
          tone={limit.quotaDispatchExceeded ? "danger" : "success"}
        />
        <LimitMetric
          label="shares"
          value={`${limit.shares.filter((share) => share.blocked).length}/${limit.shares.length} blocked`}
          tone={shareWarnings.length ? "warning" : "success"}
        />
      </div>
      {(limit.accountEmail || limit.accountLastRefreshError || limit.quotaDispatchLimitPercent != null) && (
        <div className="provider-limit-line">
          <span>{limit.accountEmail || "account -"}</span>
          <span>{limit.quotaDispatchLimitPercent == null ? "dispatch -" : `dispatch ${limit.quotaDispatchLimitPercent.toFixed(1)}%`}</span>
          <span>{limit.accountQuotaRefreshedAt == null ? "quota refresh -" : formatTime(limit.accountQuotaRefreshedAt)}</span>
          {limit.accountLastRefreshError && <strong>{limit.accountLastRefreshError}</strong>}
        </div>
      )}
      {warnings.length > 0 && (
        <div className="provider-warning-list">
          {warnings.slice(0, 4).map((warning, index) => (
            <span key={`${warning}:${index}`}>{warning}</span>
          ))}
        </div>
      )}
    </div>
  );
}

function LimitMetric({
  label,
  value,
  tone,
}: {
  label: string;
  value: string;
  tone: "success" | "warning" | "danger";
}) {
  const { tx } = useI18n();
  return (
    <div className="limit-metric">
      <span>{tx(label)}</span>
      <StatusPill tone={tone}>{value}</StatusPill>
    </div>
  );
}



function formatTime(value?: number | null): string {
  if (!value) return "expires -";
  const millis = value < 10_000_000_000 ? value * 1000 : value;
  const date = new Date(millis);
  if (Number.isNaN(date.getTime())) return "expires -";
  return new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" }).format(date);
}
