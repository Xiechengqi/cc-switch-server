import { FileJson } from "lucide-react";

import { KeyValue } from "@/components/KeyValue";
import { ProviderIcon } from "@/components/ProviderIcon";
import { StatusPill } from "@/components/StatusPill";
import { appIcon } from "@/lib/provider-icons";
import type { ShareRecord, UsageLog } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { appLabel, formatDuration, formatTime, formatTokens, formatUsd, shareName } from "@/components/share/shareDisplay";

export function ShareRequestLogPanel({ logs, shares }: { logs: UsageLog[]; shares: ShareRecord[] }) {
  const { tx } = useI18n();
  const shareById = new Map(shares.map((share) => [share.id, share]));
  return (
    <section className="share-request-log-panel">
      <div className="section-heading">
        <div className="section-title-row compact-title">
          <FileJson size={16} />
          <div>
            <h2>{tx("Request Logs")}</h2>
            <span>{tx("{{count}} recent share requests", { count: logs.length })}</span>
          </div>
        </div>
      </div>
      {logs.length ? (
        <div className="share-request-log-list">
          {logs.slice(0, 80).map((log) => (
            <ShareRequestLogCard
              key={log.requestId}
              log={log}
              share={log.shareId ? shareById.get(log.shareId) : undefined}
            />
          ))}
        </div>
      ) : (
        <div className="provider-empty compact-empty">
          <FileJson size={20} />
          <span>{tx("No share request logs")}</span>
        </div>
      )}
    </section>
  );
}

function ShareRequestLogCard({ log, share }: { log: UsageLog; share?: ShareRecord }) {
  const { tx } = useI18n();
  const app = appIcon(log.app);
  const model = log.actualModel || log.requestedModel || log.model || "-";
  const ok = log.statusCode >= 200 && log.statusCode < 400;
  return (
    <article className="share-request-log-card">
      <header>
        <div className="share-request-title">
          <span className="provider-icon-frame small">
            <ProviderIcon icon={app.icon} color={app.color} name={appLabel(log.app)} size={18} />
          </span>
          <div>
            <strong title={log.shareId || undefined}>{log.shareName || (share ? shareName(share) : log.shareId || "-")}</strong>
            <span title={model}>{model}</span>
          </div>
        </div>
        <div className="share-request-status">
          <StatusPill tone={ok ? "success" : "danger"}>{log.statusCode || "-"}</StatusPill>
          <small>{formatTime(log.createdAtMs)}</small>
        </div>
      </header>
      <div className="share-request-metrics">
        <KeyValue label="app" value={appLabel(log.app)} />
        <KeyValue label="tokens" value={formatTokens(log.totalTokens)} />
        <KeyValue label="cost" value={formatUsd(log.totalCostUsd)} />
        <KeyValue label="latency" value={formatDuration(log.durationMs)} />
      </div>
      <div className="share-request-tags">
        <span>{log.userEmail || tx("anonymous")}</span>
        {log.dataSource && <span>{log.dataSource}</span>}
        {log.streamStatus && <span>{tx(log.streamStatus)}</span>}
        {log.isHealthCheck && <span>{tx("health")}</span>}
      </div>
    </article>
  );
}
