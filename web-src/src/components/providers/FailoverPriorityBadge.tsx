import { useI18n } from "@/lib/i18n";

export function FailoverPriorityBadge({ priority }: { priority: number }) {
  const { tx } = useI18n();
  const label = priority <= 1 ? tx("primary") : tx("fallback {{rank}}", { rank: priority });
  return (
    <span className={priority <= 1 ? "failover-priority-badge primary" : "failover-priority-badge"}>
      {label}
    </span>
  );
}
