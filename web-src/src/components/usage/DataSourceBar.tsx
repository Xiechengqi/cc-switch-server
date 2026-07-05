import { Database, FileText, Loader2 } from "lucide-react";
import { ReactNode } from "react";

import { useI18n } from "@/lib/i18n";

export interface UsageDataSourceSummary {
  dataSource: string;
  requests: number;
  successes: number;
  failures: number;
  totalTokens: number;
  totalCostUsd: number;
  healthChecks: number;
}

export function emptyDataSourceSummary(dataSource: string): UsageDataSourceSummary {
  return {
    dataSource,
    requests: 0,
    successes: 0,
    failures: 0,
    totalTokens: 0,
    totalCostUsd: 0,
    healthChecks: 0,
  };
}

export function DataSourceBar({
  sources,
  loading,
  activeSource,
  onSelect,
}: {
  sources: UsageDataSourceSummary[];
  loading: boolean;
  activeSource: string;
  onSelect: (dataSource: string) => void;
}) {
  const { tx } = useI18n();
  if (loading) {
    return (
      <section className="usage-data-source-panel">
        <div className="usage-data-source-label">
          <Database size={15} />
          <span>{tx("Data Sources")}</span>
        </div>
        <div className="provider-empty inline-empty">
          <Loader2 size={18} />
          <span>{tx("Loading sources")}</span>
        </div>
      </section>
    );
  }

  if (!sources.length) return null;

  const total = sources.reduce<UsageDataSourceSummary>(
    (next, source) => ({
      dataSource: "all",
      requests: next.requests + source.requests,
      successes: next.successes + source.successes,
      failures: next.failures + source.failures,
      totalTokens: next.totalTokens + source.totalTokens,
      totalCostUsd: next.totalCostUsd + source.totalCostUsd,
      healthChecks: next.healthChecks + source.healthChecks,
    }),
    emptyDataSourceSummary("all"),
  );

  return (
    <section className="usage-data-source-panel" aria-label={tx("Loaded usage sources")}>
      <div className="usage-data-source-label">
        <Database size={15} />
        <span>{tx("Data Sources")}</span>
      </div>
      <div className="usage-data-source-list">
        <DataSourceChip
          source={total}
          active={!activeSource}
          label="All"
          onClick={() => onSelect("")}
        />
        {sources.map((source) => (
          <DataSourceChip
            key={source.dataSource}
            source={source}
            active={source.dataSource === activeSource}
            label={dataSourceLabel(source.dataSource)}
            onClick={() => onSelect(source.dataSource)}
          />
        ))}
      </div>
    </section>
  );
}

function DataSourceChip({
  source,
  active,
  label,
  onClick,
}: {
  source: UsageDataSourceSummary;
  active: boolean;
  label: string;
  onClick: () => void;
}) {
  const { tx } = useI18n();
  const failureRate = source.requests > 0 ? (source.failures / source.requests) * 100 : 0;
  return (
    <button className={active ? "usage-data-source-chip active" : "usage-data-source-chip"} type="button" onClick={onClick}>
      {dataSourceIcon(source.dataSource)}
      <span>
        <strong>{tx(label)}</strong>
        <small>{formatInt(source.requests)} req</small>
      </span>
      <span>
        <strong>{formatInt(source.totalTokens)}</strong>
        <small>{formatUsd(source.totalCostUsd, 4)}</small>
      </span>
      <span>
        <strong>{failureRate.toFixed(1)}%</strong>
        <small>{source.healthChecks ? `${formatInt(source.healthChecks)} ${tx("health")}` : tx("fail")}</small>
      </span>
    </button>
  );
}

function dataSourceIcon(dataSource: string): ReactNode {
  if (dataSource === "all") return <Database size={15} />;
  if (
    dataSource === "session_log" ||
    dataSource === "codex_session" ||
    dataSource === "gemini_session" ||
    dataSource === "opencode_session" ||
    dataSource.includes("session")
  ) {
    return <FileText size={15} />;
  }
  return <Database size={15} />;
}

function dataSourceLabel(dataSource: string): string {
  if (dataSource === "proxy") return "Proxy";
  if (dataSource === "session_log") return "Session logs";
  if (dataSource === "codex_db") return "Codex DB";
  if (dataSource === "codex_session") return "Codex session";
  if (dataSource === "gemini_session") return "Gemini session";
  if (dataSource === "opencode_session") return "OpenCode session";
  return dataSource.replace(/[_-]+/g, " ") || "unknown";
}

function formatInt(value?: number | null): string {
  if (value == null) return "-";
  return new Intl.NumberFormat(undefined, { maximumFractionDigits: 0 }).format(value);
}

function formatUsd(value: number, digits: number): string {
  if (!Number.isFinite(value)) return "-";
  return `$${value.toFixed(digits)}`;
}
