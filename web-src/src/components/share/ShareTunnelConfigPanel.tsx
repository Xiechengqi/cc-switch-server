import { Cable, FileJson, Loader2, Route, Store } from "lucide-react";
import type { ReactNode } from "react";

import { StatusPill } from "@/components/StatusPill";
import type { PublicShareMarket, ShareRecord } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { formatTime, shareName } from "@/components/share/shareDisplay";

export function ShareTunnelConfigPanel({
  shares,
  markets,
  marketsLoaded,
  busyId,
  onSnapshot,
  onRestore,
  onPullEdits,
  onLoadMarkets,
}: {
  shares: ShareRecord[];
  markets: PublicShareMarket[];
  marketsLoaded: boolean;
  busyId: string | null;
  onSnapshot: () => void;
  onRestore: () => void;
  onPullEdits: () => void;
  onLoadMarkets: () => void;
}) {
  const { tx } = useI18n();
  const tunneled = shares.filter((share) => share.tunnelSubdomain || share.routerUrl);
  const syncErrors = shares.filter((share) => share.routerLastSyncError);
  const marketShares = shares.filter((share) => share.acl?.publicMarketEmail);
  const pendingGrants = shares.filter((share) => share.marketGrant?.status === "pending");
  const visibleRoutes = [...tunneled]
    .sort((left, right) => (right.routerLastSyncedAtMs || 0) - (left.routerLastSyncedAtMs || 0))
    .slice(0, 4);
  return (
    <section className="share-tunnel-panel">
      <header className="share-tunnel-header">
        <div className="section-title-row compact-title">
          <Cable size={16} />
          <div>
            <h2>{tx("Tunnel Config")}</h2>
            <span>{tx("Router routes, tunnel recovery, and market authorization status")}</span>
          </div>
        </div>
        <div className="share-tunnel-actions">
          <button className="secondary-button compact" type="button" onClick={onSnapshot} disabled={busyId === "snapshot"}>
            {busyId === "snapshot" ? <Loader2 size={14} /> : <FileJson size={14} />}
            <span>{tx("Runtime")}</span>
          </button>
          <button className="secondary-button compact" type="button" onClick={onRestore} disabled={busyId === "restore"}>
            {busyId === "restore" ? <Loader2 size={14} /> : <Cable size={14} />}
            <span>{tx("Restore tunnels")}</span>
          </button>
          <button className="secondary-button compact" type="button" onClick={onPullEdits} disabled={busyId === "edits"}>
            {busyId === "edits" ? <Loader2 size={14} /> : <Route size={14} />}
            <span>{tx("Pull edits")}</span>
          </button>
          <button className="secondary-button compact" type="button" onClick={onLoadMarkets} disabled={busyId === "markets"}>
            {busyId === "markets" ? <Loader2 size={14} /> : <Store size={14} />}
            <span>{tx("Markets")}</span>
          </button>
        </div>
      </header>
      <div className="share-tunnel-summary">
        <ShareTunnelMetric label="tunnel routes" value={`${tunneled.length}/${shares.length}`} tone={syncErrors.length ? "warning" : "success"} />
        <ShareTunnelMetric label="sync errors" value={syncErrors.length} tone={syncErrors.length ? "danger" : "success"} />
        <ShareTunnelMetric label="market access" value={marketsLoaded ? `${marketShares.length}/${markets.length || "-"}` : tx("not loaded")} tone={marketsLoaded ? "success" : "warning"} />
        <ShareTunnelMetric label="pending grants" value={pendingGrants.length} tone={pendingGrants.length ? "warning" : "success"} />
      </div>
      {visibleRoutes.length ? (
        <div className="share-tunnel-route-list">
          {visibleRoutes.map((share) => (
            <ShareTunnelRoute key={share.id} share={share} />
          ))}
        </div>
      ) : (
        <div className="share-tunnel-empty">
          <Route size={16} />
          <span>{tx("No tunnel routes yet")}</span>
        </div>
      )}
    </section>
  );
}

function ShareTunnelMetric({
  label,
  value,
  tone,
}: {
  label: string;
  value: ReactNode;
  tone: "success" | "warning" | "danger";
}) {
  const { tx } = useI18n();
  return (
    <div className="share-tunnel-metric">
      <span>{tx(label)}</span>
      <StatusPill tone={tone}>{value}</StatusPill>
    </div>
  );
}

function ShareTunnelRoute({ share }: { share: ShareRecord }) {
  const { tx } = useI18n();
  const syncTone = share.routerLastSyncError ? "danger" : share.routerLastSyncedAtMs ? "success" : "warning";
  return (
    <div className="share-tunnel-route">
      <div>
        <strong>{shareName(share)}</strong>
        <span>{share.routerUrl || share.tunnelSubdomain || "-"}</span>
      </div>
      <div>
        <StatusPill tone={share.status === "active" ? "success" : share.status === "paused" ? "warning" : "danger"}>
          {share.status}
        </StatusPill>
        <StatusPill tone={syncTone}>
          {share.routerLastSyncError ? tx("sync error") : formatTime(share.routerLastSyncedAtMs)}
        </StatusPill>
      </div>
    </div>
  );
}
