import { Eye, Filter } from "lucide-react";

import { inferIconForText } from "@/config/iconInference";
import type { UsageLog } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { LoadingBlock } from "@/components/LoadingBlock";
import { ProviderIcon } from "@/components/ProviderIcon";
import { StatusPill } from "@/components/StatusPill";
import { UsageMiniMetric } from "@/components/usage/UsageMiniMetric";
import { formatInt, formatLatency, formatTime, formatUsd, freshInputTokens, modelRoute, sourceText } from "@/components/usage/usageDisplay";

export function UsageLogsPanel({
  logs,
  loading,
  onDetail,
}: {
  logs: UsageLog[];
  loading: boolean;
  onDetail: (log: UsageLog) => void;
}) {
  const { tx } = useI18n();
  if (loading) return <LoadingBlock label="Loading request logs" />;
  return (
    <section className="usage-panel-card usage-activity-panel">
      <div className="section-heading">
        <div className="section-title-row compact-title">
          <Filter size={17} />
          <h2>{tx("Request Logs")}</h2>
        </div>
        <span>{tx("{{count}} entries", { count: logs.length })}</span>
      </div>
      {logs.length ? (
        <div className="usage-log-list">
          {logs.map((log) => (
            <UsageLogCard key={log.requestId} log={log} onDetail={onDetail} />
          ))}
        </div>
      ) : (
        <div className="provider-empty">
          <Filter size={22} />
          <span>{tx("No request logs")}</span>
        </div>
      )}
    </section>
  );
}

function UsageLogCard({ log, onDetail }: { log: UsageLog; onDetail: (log: UsageLog) => void }) {
  const { tx } = useI18n();
  const icon = inferIconForText(log.providerType, log.providerName, log.providerId, log.app);
  const ok = log.statusCode >= 200 && log.statusCode < 300;
  const cache = (log.cacheReadTokens || 0) + (log.cacheCreationTokens || 0);
  const totalTokens = log.totalTokens ?? freshInputTokens(log) + (log.outputTokens || 0) + cache;
  const tokenDetail = tx("in {{input}} / out {{output}}", {
    input: formatInt(freshInputTokens(log)),
    output: formatInt(log.outputTokens),
  });
  return (
    <article className="usage-log-card">
      <header>
        <div className="usage-log-title">
          <span className="provider-icon-frame">
            <ProviderIcon
              icon={icon.icon}
              color={icon.iconColor}
              name={log.providerName || log.providerId || log.app}
              size={22}
            />
          </span>
          <div>
            <strong title={log.providerId}>{log.providerName || log.providerId}</strong>
            <span title={modelRoute(log)}>{modelRoute(log)}</span>
          </div>
        </div>
        <div className="usage-log-status">
          <StatusPill tone={ok ? "success" : "danger"}>{log.statusCode}</StatusPill>
          <small>{formatTime(log.createdAtMs)}</small>
        </div>
      </header>
      <div className="usage-log-metrics">
        <UsageMiniMetric label="tokens" value={formatInt(totalTokens)} detail={tokenDetail} />
        <UsageMiniMetric label="cost" value={log.totalCostUsd == null ? "-" : formatUsd(log.totalCostUsd, 5)} detail={log.pricingModel || "-"} />
        <UsageMiniMetric label="latency" value={formatLatency(log)} detail={tx(log.isStreaming ? "streaming" : "non-stream")} />
        <UsageMiniMetric label="source" value={sourceText(log) || "-"} detail={log.sessionId || log.requestAgent || "-"} />
      </div>
      <footer>
        <div className="usage-log-tags">
          <span>{tx(log.app)}</span>
          {log.isHealthCheck && <span>{tx("health")}</span>}
          {log.shareName || log.shareId ? <span>{log.shareName || log.shareId}</span> : null}
          {log.userEmail && <span>{log.userEmail}</span>}
          {log.streamStatus && <span>{tx(log.streamStatus)}</span>}
        </div>
        <button className="secondary-button compact-action" type="button" onClick={() => onDetail(log)}>
          <Eye size={15} />
          <span>{tx("Detail")}</span>
        </button>
      </footer>
    </article>
  );
}
