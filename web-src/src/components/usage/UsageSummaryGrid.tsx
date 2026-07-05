import type { UsageRollup } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { formatInt, formatUsd } from "@/components/usage/usageDisplay";

export function UsageSummaryGrid({ summary, loading }: { summary: UsageRollup; loading: boolean }) {
  const successRate = summary.requests > 0 ? (summary.successes / summary.requests) * 100 : 0;
  const failureRate = summary.requests > 0 ? (summary.failures / summary.requests) * 100 : 0;
  const cacheRate =
    summary.inputTokens + summary.cacheReadTokens + summary.cacheCreationTokens > 0
      ? (summary.cacheReadTokens /
          (summary.inputTokens + summary.cacheReadTokens + summary.cacheCreationTokens)) *
        100
      : 0;
  return (
    <div className="usage-summary-grid">
      <UsageMetricCard
        label="requests"
        value={loading ? "..." : formatInt(summary.requests)}
        detail={loading ? "..." : `${formatInt(summary.successes)} ok / ${formatInt(summary.failures)} failed`}
        progress={successRate}
      />
      <UsageMetricCard
        label="success"
        value={loading ? "..." : `${successRate.toFixed(1)}%`}
        detail={loading ? "..." : `${failureRate.toFixed(1)}% fail`}
        progress={successRate}
      />
      <UsageMetricCard
        label="tokens"
        value={loading ? "..." : formatInt(summary.totalTokens)}
        detail={loading ? "..." : `${formatInt(summary.inputTokens)} in / ${formatInt(summary.outputTokens)} out`}
        progress={100}
      />
      <UsageMetricCard
        label="cache hit"
        value={loading ? "..." : `${cacheRate.toFixed(1)}%`}
        detail={loading ? "..." : `${formatInt(summary.cacheReadTokens)} read`}
        progress={cacheRate}
      />
      <UsageMetricCard
        label="cost"
        value={loading ? "..." : formatUsd(summary.totalCostUsd, 4)}
        detail={loading ? "..." : `${formatUsd(summary.requests ? summary.totalCostUsd / summary.requests : 0, 6)} / req`}
        progress={100}
      />
    </div>
  );
}

function UsageMetricCard({
  label,
  value,
  detail,
  progress,
}: {
  label: string;
  value: string;
  detail: string;
  progress: number;
}) {
  const { tx } = useI18n();
  return (
    <div className="usage-metric-card">
      <span>{tx(label)}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
      <div className="usage-metric-progress" aria-hidden="true">
        <span style={{ width: `${Math.max(0, Math.min(100, progress))}%` }} />
      </div>
    </div>
  );
}
