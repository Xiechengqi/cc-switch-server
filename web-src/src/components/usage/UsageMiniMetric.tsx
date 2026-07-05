import type { ReactNode } from "react";

import { useI18n } from "@/lib/i18n";

export function UsageMiniMetric({ label, value, detail }: { label: string; value: ReactNode; detail: ReactNode }) {
  const { tx } = useI18n();
  return (
    <div className="usage-mini-metric">
      <span>{tx(label)}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
    </div>
  );
}
