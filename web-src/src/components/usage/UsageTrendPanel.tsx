import { useMemo } from "react";
import { BarChart3 } from "lucide-react";

import type { UsageTrendPoint } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { LoadingBlock } from "@/components/LoadingBlock";
import { compactNumber, compactTime, formatInt, formatTime, formatUsd } from "@/components/usage/usageDisplay";

export function UsageTrendPanel({
  trends,
  loading,
  onSelectRange,
}: {
  trends: UsageTrendPoint[];
  loading: boolean;
  onSelectRange: (point: UsageTrendPoint) => void;
}) {
  const { tx } = useI18n();
  const chartData = useMemo(
    () =>
      trends.map((point) => ({
        key: `${point.startMs}:${point.endMs}`,
        label: compactTime(point.startMs),
        startMs: point.startMs,
        endMs: point.endMs,
        requests: point.rollup.requests,
        tokens: point.rollup.totalTokens,
        cost: Number(point.rollup.totalCostUsd.toFixed(6)),
        successRate: point.rollup.requests ? (point.rollup.successes / point.rollup.requests) * 100 : 0,
      })),
    [trends],
  );
  const maxTokens = Math.max(1, ...chartData.map((point) => point.tokens));
  const maxRequests = Math.max(1, ...chartData.map((point) => point.requests));
  const maxCost = Math.max(0.000001, ...chartData.map((point) => point.cost));
  return (
    <section className="usage-trend-panel">
      <div className="section-heading">
        <div className="section-title-row compact-title">
          <BarChart3 size={17} />
          <h2>{tx("Trend")}</h2>
        </div>
        <span>{tx("{{count}} buckets", { count: trends.length })}</span>
      </div>
      {loading ? (
        <LoadingBlock label="Loading usage trend" />
      ) : trends.length ? (
        <div className="usage-chart-card">
          <div className="usage-chart-legend" aria-hidden="true">
            <span className="tokens">{tx("Tokens")}</span>
            <span className="requests">{tx("Requests")}</span>
            <span className="cost">{tx("Cost")}</span>
          </div>
          <svg className="usage-svg-chart" viewBox="0 0 760 260" role="img" aria-label={tx("Usage trend chart")}>
            {[40, 85, 130, 175, 220].map((y) => (
              <line key={y} className="usage-chart-grid-line" x1="42" x2="742" y1={y} y2={y} />
            ))}
            <text className="usage-chart-axis-label" x="42" y="24">{compactNumber(maxTokens)}</text>
            <text className="usage-chart-axis-label" x="708" y="24">{formatUsd(maxCost, 3)}</text>
            {chartData.map((point, index) => {
              const slot = 700 / Math.max(1, chartData.length);
              const groupX = 46 + index * slot;
              const barWidth = Math.max(3, Math.min(14, slot / 5));
              const tokenHeight = Math.max(2, (point.tokens / maxTokens) * 178);
              const requestHeight = Math.max(2, (point.requests / maxRequests) * 178);
              const costHeight = Math.max(2, (point.cost / maxCost) * 178);
              const labelEvery = Math.max(1, Math.ceil(chartData.length / 8));
              const title = `${formatTime(point.startMs)} - ${formatInt(point.tokens)} ${tx("tokens")} - ${formatUsd(point.cost, 4)}`;
              return (
                <g
                  key={point.key}
                  className="usage-chart-group"
                  role="button"
                  tabIndex={0}
                  aria-label={tx("Filter {{time}}", { time: formatTime(point.startMs) })}
                  onClick={() => {
                    const trend = trends.find((item) => `${item.startMs}:${item.endMs}` === point.key);
                    if (trend) onSelectRange(trend);
                  }}
                  onKeyDown={(event) => {
                    if (event.key !== "Enter" && event.key !== " ") return;
                    event.preventDefault();
                    const trend = trends.find((item) => `${item.startMs}:${item.endMs}` === point.key);
                    if (trend) onSelectRange(trend);
                  }}
                >
                  <title>{title}</title>
                  <rect className="usage-chart-hover-target" x={groupX - 4} y="30" width={Math.max(10, slot)} height="210" rx="6" />
                  <rect className="usage-chart-bar tokens" x={groupX} y={220 - tokenHeight} width={barWidth} height={tokenHeight} rx="3" />
                  <rect className="usage-chart-bar requests" x={groupX + barWidth + 2} y={220 - requestHeight} width={barWidth} height={requestHeight} rx="3" />
                  <rect className="usage-chart-bar cost" x={groupX + (barWidth + 2) * 2} y={220 - costHeight} width={barWidth} height={costHeight} rx="3" />
                  {index % labelEvery === 0 && (
                    <text className="usage-chart-tick-label" x={groupX} y="244">
                      {point.label}
                    </text>
                  )}
                </g>
              );
            })}
          </svg>
        </div>
      ) : (
        <div className="provider-empty">
          <BarChart3 size={22} />
          <span>{tx("No usage trend data")}</span>
        </div>
      )}
    </section>
  );
}
