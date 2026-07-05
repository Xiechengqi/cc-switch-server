import { ProviderHealth } from "@/lib/api";
import { useI18n } from "@/lib/i18n";

export function ProviderHealthIndicator({ health }: { health?: ProviderHealth }) {
  const { tx } = useI18n();
  const status = providerHealthStatus(health);
  const latency = health?.avgLatencyMs == null ? null : `${Math.round(health.avgLatencyMs)}ms`;
  return (
    <div className={`provider-health-indicator ${status}`}>
      <span className="provider-health-dot" />
      <span>
        {tx(status)}
        {latency ? ` (${latency})` : ""}
      </span>
    </div>
  );
}

function providerHealthStatus(health?: ProviderHealth): "operational" | "degraded" | "failed" {
  if (!health) return "degraded";
  if (!health.healthy) return "failed";
  if ((health.failures || 0) > 0 || (health.successRate != null && health.successRate < 0.95)) {
    return "degraded";
  }
  return "operational";
}
