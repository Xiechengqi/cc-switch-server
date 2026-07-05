import { AlertTriangle } from "lucide-react";

import { inferIconForText } from "@/config/iconInference";
import type { ProviderLimitStatus } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { KeyValue } from "@/components/KeyValue";
import { LoadingBlock } from "@/components/LoadingBlock";
import { ProviderIcon } from "@/components/ProviderIcon";
import { StatusPill } from "@/components/StatusPill";
import { formatInt, formatTime, formatUsd, limitPercent } from "@/components/usage/usageDisplay";

export function ProviderLimitsGrid({ limits, loading }: { limits: ProviderLimitStatus[]; loading: boolean }) {
  const { tx } = useI18n();
  if (loading) return <LoadingBlock label="Loading provider limits" />;
  return (
    <section className="usage-panel-card">
      <div className="section-heading">
        <div className="section-title-row compact-title">
          <AlertTriangle size={17} />
          <h2>{tx("Provider Limits")}</h2>
        </div>
        <span>{tx("{{count}} providers", { count: limits.length })}</span>
      </div>
      {limits.length ? (
        <div className="usage-limit-grid">
          {limits.map((limit) => (
            <ProviderLimitCard key={`${limit.app}:${limit.providerId}`} limit={limit} />
          ))}
        </div>
      ) : (
        <div className="provider-empty">
          <AlertTriangle size={22} />
          <span>{tx("No provider limits")}</span>
        </div>
      )}
    </section>
  );
}

function ProviderLimitCard({ limit }: { limit: ProviderLimitStatus }) {
  const { tx } = useI18n();
  const icon = inferIconForText(limit.providerType, limit.providerName, limit.providerId, limit.app);
  const stateTone = limit.blocked ? "danger" : limit.warnings.length ? "warning" : "success";
  return (
    <article className="usage-limit-card">
      <header>
        <div className="usage-log-title">
          <span className="provider-icon-frame">
            <ProviderIcon icon={icon.icon} color={icon.iconColor} name={limit.providerName || limit.providerId} size={22} />
          </span>
          <div>
            <strong title={limit.providerId}>{limit.providerName || limit.providerId}</strong>
            <span>{`${limit.providerType} / ${limit.app}`}</span>
          </div>
        </div>
        <StatusPill tone={stateTone}>{tx(limit.blocked ? "blocked" : limit.warnings.length ? "warning" : "ok")}</StatusPill>
      </header>
      <div className="usage-limit-meter-stack">
        <LimitMeter label="daily" usage={formatUsd(limit.dailyUsageUsd, 4)} limit={limit.dailyLimitUsd == null ? "-" : formatUsd(limit.dailyLimitUsd, 2)} percent={limitPercent(limit.dailyUsageUsd, limit.dailyLimitUsd)} exceeded={limit.dailyExceeded} />
        <LimitMeter label="monthly" usage={formatUsd(limit.monthlyUsageUsd, 4)} limit={limit.monthlyLimitUsd == null ? "-" : formatUsd(limit.monthlyLimitUsd, 2)} percent={limitPercent(limit.monthlyUsageUsd, limit.monthlyLimitUsd)} exceeded={limit.monthlyExceeded} />
        <LimitMeter label="quota" usage={limit.accountQuotaPercent == null ? "-" : `${limit.accountQuotaPercent.toFixed(1)}%`} limit={limit.quotaDispatchLimitPercent == null ? "-" : `${limit.quotaDispatchLimitPercent.toFixed(1)}%`} percent={limit.accountQuotaPercent ?? 0} exceeded={limit.quotaDispatchExceeded} />
      </div>
      <div className="usage-limit-meta">
        <KeyValue label="account" value={limit.accountEmail || limit.accountId || "-"} />
        <KeyValue label="shares" value={formatInt(limit.shares.length)} />
        <KeyValue label="quota refreshed" value={formatTime(limit.accountQuotaRefreshedAt)} />
      </div>
      {(limit.warnings.length || limit.accountLastRefreshError) && (
        <div className="usage-log-tags warning-tags">
          {limit.warnings.map((warning) => (
            <span key={warning}>{warning}</span>
          ))}
          {limit.accountLastRefreshError && <span>{limit.accountLastRefreshError}</span>}
        </div>
      )}
      {limit.shares.length > 0 && (
        <div className="usage-share-strip">
          {limit.shares.slice(0, 4).map((share) => (
            <span key={share.shareId} title={share.warnings.join(", ")}>
              {share.shareName || share.shareId}
            </span>
          ))}
          {limit.shares.length > 4 && <span>{tx("+{{count}} more", { count: limit.shares.length - 4 })}</span>}
        </div>
      )}
    </article>
  );
}

function LimitMeter({
  label,
  usage,
  limit,
  percent,
  exceeded,
}: {
  label: string;
  usage: string;
  limit: string;
  percent: number;
  exceeded: boolean;
}) {
  const { tx } = useI18n();
  const width = Math.max(0, Math.min(100, percent));
  return (
    <div className={exceeded ? "usage-limit-meter exceeded" : "usage-limit-meter"}>
      <div>
        <span>{tx(label)}</span>
        <strong>{usage}</strong>
        <small>{tx("limit")}: {limit}</small>
      </div>
      <div className="usage-rank-meter" aria-label={tx(label)}>
        <span style={{ width: `${width}%` }} />
      </div>
    </div>
  );
}
