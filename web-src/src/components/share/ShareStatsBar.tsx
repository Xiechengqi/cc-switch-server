import type { ReactNode } from "react";

import type { ShareRecord } from "@/lib/api";
import { useI18n } from "@/lib/i18n";

export function ShareStatsBar({ shares }: { shares: ShareRecord[] }) {
  const { t } = useI18n();
  return (
    <div className="share-stats-bar">
      <ShareStat label={t("server.shares.active")} value={shares.filter((share) => share.status === "active").length} />
      <ShareStat label={t("server.shares.paused")} value={shares.filter((share) => share.status === "paused").length} />
      <ShareStat label={t("server.shares.forSale")} value={shares.filter((share) => share.forSale).length} />
      <ShareStat label={t("server.shares.requests")} value={shares.reduce((sum, share) => sum + (share.requestsCount || 0), 0)} />
    </div>
  );
}

function ShareStat({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="share-stat">
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}
