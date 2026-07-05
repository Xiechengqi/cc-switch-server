import {
  Cable,
  Copy,
  Edit3,
  FileJson,
  Link2,
  Pause,
  Play,
  RotateCcw,
  Share2,
  SlidersHorizontal,
  Store,
  Trash2,
  Users,
} from "lucide-react";
import { ReactNode, useState } from "react";

import { AppKind, PublicShareMarket, ShareConnectInfo, ShareRecord, StoredProvider } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { IconAction } from "@/components/IconAction";
import { KeyValue } from "@/components/KeyValue";
import { JsonPreview } from "@/components/JsonPreview";
import { ProviderIcon } from "@/components/ProviderIcon";
import { StatusPill } from "@/components/StatusPill";
import { appIcon, storedProviderIcon } from "@/lib/provider-icons";
import {
  appLabel,
  formatTime,
  shareBindings,
  shareName,
  shareUsage,
  shareUsageRatio,
} from "@/components/share/shareDisplay";

const apps: Array<{ id: AppKind; label: string; icon: string; color?: string }> = [
  { id: "claude", label: "Claude", icon: appIcon("claude").icon, color: appIcon("claude").color },
  { id: "codex", label: "Codex", icon: appIcon("codex").icon, color: appIcon("codex").color },
  { id: "gemini", label: "Gemini", icon: appIcon("gemini").icon, color: appIcon("gemini").color },
];

function providerKey(app: AppKind, providerId: string): string {
  return `${app}:${providerId}`;
}

export function ShareCard({
  share,
  providerByKey,
  markets,
  marketsLoaded,
  result,
  connectInfo,
  runtimePanel,
  busyId,
  onEdit,
  onAcl,
  onSubdomain,
  onBinding,
  onMarket,
  onAction,
}: {
  share: ShareRecord;
  providerByKey: Map<string, StoredProvider>;
  markets: PublicShareMarket[];
  marketsLoaded: boolean;
  result?: string;
  connectInfo?: ShareConnectInfo;
  runtimePanel: ReactNode;
  busyId: string | null;
  onEdit: () => void;
  onAcl: () => void;
  onSubdomain: () => void;
  onBinding: (app: AppKind) => void;
  onMarket: () => void;
  onAction: (
    action:
      | "pause"
      | "resume"
      | "startTunnel"
      | "stopTunnel"
      | "resetUsage"
      | "connectInfo"
      | "delete",
  ) => void;
}) {
  const { tx } = useI18n();
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  const [resetConfirmOpen, setResetConfirmOpen] = useState(false);
  const [copyStatus, setCopyStatus] = useState<{ tone: "success" | "warning"; message: string } | null>(null);
  const usage = shareUsage(share);
  const syncText = share.routerLastSyncError || formatTime(share.routerLastSyncedAtMs);
  const market = markets.find((item) => item.email === share.acl?.publicMarketEmail);
  const sharedEmailCount = share.acl?.sharedWithEmails?.length || 0;
  const shareActive = share.enabled && share.status === "active";
  const shareCanRestart = share.status === "paused" || share.status === "stopped";
  const pauseResumeAction = shareActive ? "pause" : "resume";
  const pauseResumeDisabled = !shareActive && !shareCanRestart;
  const startTunnelDisabled = shareActive || !shareCanRestart;
  const stopTunnelDisabled = !shareActive;

  async function copyConnectInfo(value: string, successMessage: string) {
    if (!navigator.clipboard?.writeText) {
      setCopyStatus({ tone: "warning", message: tx("Clipboard unavailable; copy the visible value manually.") });
      return;
    }
    try {
      await navigator.clipboard.writeText(value);
      setCopyStatus({ tone: "success", message: successMessage });
    } catch {
      setCopyStatus({ tone: "warning", message: tx("Copy failed; copy the visible value manually.") });
    }
  }

  return (
    <>
    <article className="share-card">
      <header className="share-card-header">
        <div className="share-card-title-row">
          <div className="provider-icon-frame share-icon-frame">
            <Share2 size={22} />
          </div>
          <div>
            <h3>{shareName(share)}</h3>
            <p>{share.ownerEmail || "owner -"}</p>
          </div>
        </div>
        <div className="share-card-right">
          {share.forSale && <StatusPill tone="success">{share.saleMarketKind || "sale"}</StatusPill>}
          <StatusPill tone={share.status === "active" ? "success" : share.status === "paused" ? "warning" : "danger"}>
            {share.status}
          </StatusPill>
        </div>
      </header>

      <div className="share-chip-row">
        {shareBindings(share).map((binding) => {
          const provider = providerByKey.get(providerKey(binding.app, binding.providerId));
          const icon = provider ? storedProviderIcon(provider) : appIcon(binding.app);
          return (
            <button key={binding.app} type="button" className="share-binding-chip" onClick={() => onBinding(binding.app)}>
              <ProviderIcon
                icon={icon.icon}
                name={provider?.provider.name || appLabel(binding.app)}
                color={icon.color}
                size={18}
              />
              <span>
                <small>{appLabel(binding.app)}</small>
                <strong>{provider?.provider.name || binding.providerId}</strong>
              </span>
            </button>
          );
        })}
        {apps
          .filter((app) => !shareBindings(share).some((binding) => binding.app === app.id))
          .map((app) => (
            <button key={app.id} type="button" className="share-binding-chip muted" onClick={() => onBinding(app.id)}>
              <ProviderIcon icon={appIcon(app.id).icon} name={app.label} color={appIcon(app.id).color} size={18} />
              <span>
                <small>{app.label}</small>
                <strong>unbound</strong>
              </span>
            </button>
          ))}
      </div>

      <div className="provider-card-meta">
        <KeyValue label="subdomain" value={share.tunnelSubdomain || "-"} />
        <KeyValue label="route" value={share.routerUrl || "-"} />
        <KeyValue label="tokens" value={usage} />
        <KeyValue label="parallel" value={share.parallelLimit ?? "unlimited"} />
        <KeyValue label="expires" value={formatTime(share.expiresAt)} />
        <KeyValue label="router sync" value={syncText} />
      </div>

      <div className="share-progress" aria-label="token usage">
        <span style={{ width: `${Math.min(100, shareUsageRatio(share) * 100)}%` }} />
      </div>

      <div className="share-status-grid">
        <ShareStatus label="ACL" value={sharedEmailCount ? tx("{{count}} users", { count: sharedEmailCount }) : tx("private")} />
        <ShareStatus label="sale" value={share.forSale ? share.saleMarketKind || "share" : tx("no")} />
        <ShareStatus label="market" value={market?.displayName || share.acl?.publicMarketEmail || (marketsLoaded ? "-" : tx("not loaded"))} />
        <ShareStatus label="grant" value={share.marketGrant?.status || "-"} />
      </div>

      {runtimePanel}

      {(result || share.lastError) && <div className="provider-card-result">{result || share.lastError}</div>}

      {connectInfo && (
        <details className="json-details" open>
          <summary>{tx("Connect info")}</summary>
          <div className="connect-info-block">
            <KeyValue label="direct URL" value={connectInfo.directUrl} />
            <button
              className="secondary-button compact"
              type="button"
              onClick={() => void copyConnectInfo(connectInfo.directUrl, tx("Copied URL"))}
            >
              <Copy size={13} />
              <span>{tx("Copy URL")}</span>
            </button>
            <button
              className="secondary-button compact"
              type="button"
              onClick={() => void copyConnectInfo(JSON.stringify(connectInfo, null, 2), tx("Copied JSON"))}
            >
              <Copy size={13} />
              <span>{tx("Copy JSON")}</span>
            </button>
            {copyStatus && <div className={`connect-copy-status ${copyStatus.tone}`}>{copyStatus.message}</div>}
            <JsonPreview value={connectInfo} />
          </div>
        </details>
      )}

      <div className="provider-actions">
        <IconAction title="Edit" onClick={onEdit} wrap={false}>
          <Edit3 size={15} />
        </IconAction>
        <IconAction title="ACL" onClick={onAcl} wrap={false}>
          <Users size={15} />
        </IconAction>
        <IconAction title="Subdomain" onClick={onSubdomain} wrap={false}>
          <Link2 size={15} />
        </IconAction>
        <IconAction title="Connect info" busy={busyId === `${share.id}:connectInfo`} onClick={() => onAction("connectInfo")} wrap={false}>
          <FileJson size={15} />
        </IconAction>
        <IconAction
          title={shareActive ? "Pause" : "Resume"}
          busy={busyId === `${share.id}:${pauseResumeAction}`}
          disabled={pauseResumeDisabled}
          onClick={() => onAction(pauseResumeAction)}
          wrap={false}
        >
          {shareActive ? <Pause size={15} /> : <Play size={15} />}
        </IconAction>
        <IconAction
          title="Start tunnel"
          busy={busyId === `${share.id}:startTunnel`}
          disabled={startTunnelDisabled}
          onClick={() => onAction("startTunnel")}
          wrap={false}
        >
          <Cable size={15} />
        </IconAction>
        <IconAction
          title="Stop tunnel"
          busy={busyId === `${share.id}:stopTunnel`}
          disabled={stopTunnelDisabled}
          onClick={() => onAction("stopTunnel")}
          wrap={false}
        >
          <SlidersHorizontal size={15} />
        </IconAction>
        <IconAction title="Reset usage" busy={busyId === `${share.id}:resetUsage`} onClick={() => setResetConfirmOpen(true)} wrap={false}>
          <RotateCcw size={15} />
        </IconAction>
        <IconAction title="Authorize market" onClick={onMarket} wrap={false}>
          <Store size={15} />
        </IconAction>
        <IconAction title="Delete" busy={busyId === `${share.id}:delete`} onClick={() => setDeleteConfirmOpen(true)} danger wrap={false}>
          <Trash2 size={15} />
        </IconAction>
      </div>
    </article>
      <ConfirmDialog
        isOpen={resetConfirmOpen}
        title={tx("Reset share usage")}
        message={tx("Reset usage counters for share {{name}}? Token and request usage will be cleared.", {
          name: shareName(share),
        })}
        confirmText={tx("Reset usage")}
        onConfirm={() => {
          setResetConfirmOpen(false);
          onAction("resetUsage");
        }}
        onCancel={() => setResetConfirmOpen(false)}
      />
      <ConfirmDialog
        isOpen={deleteConfirmOpen}
        title={tx("Delete share")}
        message={tx("Delete share {{name}}?", { name: shareName(share) })}
        confirmText={tx("Delete")}
        onConfirm={() => {
          setDeleteConfirmOpen(false);
          onAction("delete");
        }}
        onCancel={() => setDeleteConfirmOpen(false)}
      />
    </>
  );
}


function ShareStatus({ label, value }: { label: string; value: ReactNode }) {
  const { tx } = useI18n();
  return (
    <div className="share-status-item">
      <span>{tx(label)}</span>
      <strong>{value}</strong>
    </div>
  );
}
