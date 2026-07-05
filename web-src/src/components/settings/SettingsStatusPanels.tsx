import { Loader2, RotateCcw } from "lucide-react";

import { KeyValue } from "@/components/KeyValue";
import { StatusPill } from "@/components/StatusPill";
import {
  BackupManifest,
  RouterConfigView,
  RouterDiagnosticsResponse,
  RouterStatusResponse,
  TunnelRuntimeStatus,
} from "@/lib/api";
import { useI18n } from "@/lib/i18n";

export function BackupPolicySummary({ backups }: { backups: BackupManifest[] }) {
  const latest = [...backups].sort((left, right) => right.createdAtMs - left.createdAtMs)[0];
  const totalBytes = backups.reduce(
    (sum, backup) => sum + backup.files.reduce((fileSum, file) => fileSum + file.sizeBytes, 0),
    0,
  );
  const latestFiles = latest?.files.map((file) => file.fileName).sort().join(", ") || "-";
  return (
    <div className="settings-policy-grid">
      <KeyValue label="latest" value={formatTime(latest?.createdAtMs)} />
      <KeyValue label="snapshots" value={backups.length} />
      <KeyValue label="total size" value={formatBytes(totalBytes)} />
      <KeyValue label="retention" value="24 periodic" />
      <KeyValue label="latest files" value={latestFiles} />
      <KeyValue label="restore safety" value="pre-restore snapshot" />
    </div>
  );
}

export function DiagnosticsSummary({ diagnostics }: { diagnostics?: RouterDiagnosticsResponse }) {
  if (!diagnostics) return null;
  const tunnelErrors = diagnostics.tunnels.filter((tunnel) => tunnel.lastError).length;
  const activeTunnels = diagnostics.tunnels.filter((tunnel) => tunnel.status === "connected" || tunnel.status === "running").length;
  const shareErrors = diagnostics.shareSync.filter((share) => share.routerLastSyncError).length;
  const disabledShares = diagnostics.shareSync.filter((share) => !share.enabled).length;
  return (
    <div className="settings-policy-grid">
      <KeyValue label="registered" value={diagnostics.registered ? "yes" : "no"} />
      <KeyValue label="pending logs" value={diagnostics.pendingRequestLogSync} />
      <KeyValue label="active tunnels" value={`${activeTunnels}/${diagnostics.tunnels.length}`} />
      <KeyValue label="tunnel errors" value={tunnelErrors} />
      <KeyValue label="share errors" value={shareErrors} />
      <KeyValue label="disabled shares" value={disabledShares} />
    </div>
  );
}

export function RouterFacts({ router, status }: { router?: RouterConfigView; status?: RouterStatusResponse }) {
  return (
    <div className="provider-card-meta">
      <KeyValue label="installation" value={router?.installationId || "-"} />
      <KeyValue label="control secret" value={router?.controlSecretPresent ? "present" : "-"} />
      <KeyValue label="registered at" value={formatTime(router?.lastRegisteredAtMs)} />
      <KeyValue label="heartbeat" value={formatTime(status?.lastHeartbeatMs)} />
      <KeyValue label="last error" value={router?.lastRegisterError || status?.lastError || "-"} />
      <KeyValue label="public key" value={router?.publicKey ? `${router.publicKey.slice(0, 18)}...` : "-"} />
    </div>
  );
}

export function TunnelStatus({ status }: { status?: TunnelRuntimeStatus | null }) {
  return (
    <div className="provider-card-meta">
      <KeyValue label="status" value={status?.status || "-"} />
      <KeyValue label="url" value={status?.tunnelUrl || "-"} />
      <KeyValue label="lease" value={status?.leaseId || "-"} />
      <KeyValue label="expires" value={status?.leaseExpiresAt || "-"} />
      <KeyValue label="connected" value={formatTime(status?.connectedAtMs)} />
      <KeyValue label="error" value={status?.lastError || "-"} />
    </div>
  );
}

export function BackupSnapshotGrid({
  backups,
  busy,
  onRestore,
}: {
  backups: BackupManifest[];
  busy: string | null;
  onRestore: (backup: BackupManifest) => void;
}) {
  const { tx } = useI18n();
  return (
    <div className="backup-card-list">
      {backups.length ? (
        backups.map((backup) => {
          const size = backup.files.reduce((sum, file) => sum + file.sizeBytes, 0);
          const restoring = busy === `backup-restore:${backup.id}`;
          return (
            <article className="backup-card" key={backup.id}>
              <header>
                <div>
                  <strong title={backup.id}>{backup.id}</strong>
                  <span>{formatTime(backup.createdAtMs)}</span>
                </div>
                <button
                  className="icon-button"
                  type="button"
                  title={tx("Restore backup")}
                  aria-label={tx("Restore backup")}
                  disabled={restoring}
                  onClick={() => onRestore(backup)}
                >
                  {restoring ? <Loader2 size={15} /> : <RotateCcw size={15} />}
                </button>
              </header>
              <div className="settings-policy-grid">
                <KeyValue label="reason" value={backup.reason || "-"} />
                <KeyValue label="files" value={backup.files.length} />
                <KeyValue label="size" value={formatBytes(size)} />
                <KeyValue label="stored files" value={backup.files.map((file) => file.fileName).sort().join(", ") || "-"} />
              </div>
            </article>
          );
        })
      ) : (
        <div className="provider-empty">{tx("No backups")}</div>
      )}
    </div>
  );
}

export function Diagnostics({ diagnostics }: { diagnostics?: RouterDiagnosticsResponse }) {
  const { tx } = useI18n();
  if (!diagnostics) return <div className="provider-empty">{tx("No diagnostics")}</div>;
  return (
    <div className="diagnostics-grid">
      <div className="diagnostics-card-grid">
        {diagnostics.tunnels.length ? (
          diagnostics.tunnels.map((tunnel) => (
            <article className="diagnostics-card" key={tunnel.key}>
              <header>
                <div>
                  <strong>{tunnel.key}</strong>
                  <span>{tunnel.kind}</span>
                </div>
                <StatusPill tone={diagnosticTone(tunnel.status, tunnel.lastError)}>{tx(tunnel.status || "unknown")}</StatusPill>
              </header>
              <div className="settings-policy-grid">
                <KeyValue label="url" value={tunnel.tunnelUrl || "-"} />
                <KeyValue label="subdomain" value={tunnel.subdomain || "-"} />
                <KeyValue label="lease" value={tunnel.leaseId || "-"} />
                <KeyValue label="connected" value={formatTime(tunnel.connectedAtMs)} />
                <KeyValue label="updated" value={formatTime(tunnel.updatedAtMs)} />
                <KeyValue label="error" value={tunnel.lastError || "-"} />
              </div>
            </article>
          ))
        ) : (
          <div className="provider-empty">{tx("No tunnels")}</div>
        )}
      </div>
      <div className="diagnostics-card-grid">
        {diagnostics.shareSync.length ? (
          diagnostics.shareSync.map((share) => {
            const status = share.enabled ? share.status : "disabled";
            return (
              <article className="diagnostics-card" key={share.shareId}>
                <header>
                  <div>
                    <strong>{share.shareName || share.shareId}</strong>
                    <span>{share.shareId}</span>
                  </div>
                  <StatusPill tone={diagnosticTone(status, share.routerLastSyncError)}>{tx(status)}</StatusPill>
                </header>
                <div className="settings-policy-grid">
                  <KeyValue label="synced" value={formatTime(share.routerLastSyncedAtMs)} />
                  <KeyValue label="url" value={share.routerUrl || "-"} />
                  <KeyValue label="enabled" value={share.enabled ? "yes" : "no"} />
                  <KeyValue label="error" value={share.routerLastSyncError || "-"} />
                </div>
              </article>
            );
          })
        ) : (
          <div className="provider-empty">{tx("No share sync diagnostics")}</div>
        )}
      </div>
    </div>
  );
}

function diagnosticTone(status?: string | null, error?: string | null): "success" | "warning" | "danger" {
  if (error) return "danger";
  const normalized = status?.trim().toLowerCase();
  if (!normalized || ["disabled", "stopped", "ended", "unknown"].includes(normalized)) return "warning";
  if (["error", "failed", "expired", "exhausted"].includes(normalized)) return "danger";
  return "success";
}

function formatTime(value?: number | null): string {
  if (value == null || value <= 0) return "-";
  return new Date(value).toLocaleString();
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
