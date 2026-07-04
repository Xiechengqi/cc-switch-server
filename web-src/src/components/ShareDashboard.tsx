import {
  Cable,
  Copy,
  Download,
  Edit3,
  FileJson,
  Link2,
  Loader2,
  Pause,
  Play,
  RefreshCw,
  RotateCcw,
  Route,
  Share2,
  SlidersHorizontal,
  Store,
  Trash2,
  Upload,
  Users,
  X,
} from "lucide-react";
import { FormEvent, ReactNode, useCallback, useEffect, useMemo, useState } from "react";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { JsonPreview } from "@/components/JsonPreview";
import {
  AppKind,
  authorizeShareMarket,
  deleteShare,
  exportShares,
  importShares,
  loadShareConnectInfo,
  loadShareDashboardData,
  loadShareMarkets,
  pauseShare,
  PublicShareMarket,
  pullRouterShareEdits,
  refreshShareRuntimeSnapshots,
  replaceShareAcl,
  requestShareOwnerChangeCode,
  resetShareUsage,
  restoreShareTunnels,
  resumeShare,
  saveShare,
  ShareAcl,
  ShareBinding,
  ShareConnectInfo,
  ShareRecord,
  startShareTunnel,
  stopShareTunnel,
  StoredProvider,
  updateShareBinding,
  updateShareSubdomain,
  UpsertShareInput,
  UsageLog,
  verifyShareOwnerChangeCode,
} from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { ProviderIcon } from "@/components/ProviderIcon";
import { appIcon, storedProviderIcon } from "@/lib/provider-icons";

interface ShareDashboardState {
  shares: ShareRecord[];
  providers: StoredProvider[];
  requestLogs: UsageLog[];
}

interface ShareDraft {
  mode: "create" | "edit";
  id: string;
  originalOwnerEmail: string;
  displayName: string;
  ownerEmail: string;
  primaryApp: AppKind;
  bindings: Record<AppKind, string>;
  enabled: boolean;
  status: string;
  tokenLimit: string;
  parallelLimit: string;
  expiresAt: string;
  subdomain: string;
  description: string;
  autoStart: boolean;
  forSale: boolean;
  saleMarketKind: string;
  officialPricePercent: string;
  aclEmails: string;
  marketAccessMode: string;
}

interface BindingDraft {
  share: ShareRecord;
  app: AppKind;
  providerId: string;
}

const apps: Array<{ id: AppKind; label: string }> = [
  { id: "claude", label: "Claude" },
  { id: "codex", label: "Codex" },
  { id: "gemini", label: "Gemini" },
];

export function ShareDashboard() {
  const { t, tx } = useI18n();
  const [data, setData] = useState<ShareDashboardState>({ shares: [], providers: [], requestLogs: [] });
  const [markets, setMarkets] = useState<PublicShareMarket[]>([]);
  const [marketsLoaded, setMarketsLoaded] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [resultById, setResultById] = useState<Record<string, string>>({});
  const [connectInfoById, setConnectInfoById] = useState<Record<string, ShareConnectInfo>>({});
  const [draft, setDraft] = useState<ShareDraft | null>(null);
  const [aclDraft, setAclDraft] = useState<ShareRecord | null>(null);
  const [subdomainDraft, setSubdomainDraft] = useState<ShareRecord | null>(null);
  const [bindingDraft, setBindingDraft] = useState<BindingDraft | null>(null);
  const [marketDraft, setMarketDraft] = useState<ShareRecord | null>(null);
  const [importOpen, setImportOpen] = useState(false);
  const [exportText, setExportText] = useState<string | null>(null);
  const [ownerChangeDraft, setOwnerChangeDraft] = useState<ShareDraft | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setData(await loadShareDashboardData());
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const providersByApp = useMemo(() => {
    const grouped: Record<AppKind, StoredProvider[]> = { claude: [], codex: [], gemini: [] };
    data.providers.forEach((provider) => grouped[provider.app].push(provider));
    return grouped;
  }, [data.providers]);

  const providerByKey = useMemo(() => {
    const map = new Map<string, StoredProvider>();
    data.providers.forEach((provider) => map.set(providerKey(provider.app, provider.provider.id), provider));
    return map;
  }, [data.providers]);

  const shareRequestLogs = useMemo(() => {
    const shareIds = new Set(data.shares.map((share) => share.id));
    return data.requestLogs.filter((log) => log.shareId && shareIds.has(log.shareId));
  }, [data.requestLogs, data.shares]);

  async function runShareAction(
    share: ShareRecord,
    action:
      | "pause"
      | "resume"
      | "startTunnel"
      | "stopTunnel"
      | "resetUsage"
      | "connectInfo"
      | "delete",
  ) {
    const key = `${share.id}:${action}`;
    setBusyId(key);
    setError(null);
    try {
      if (action === "delete") {
        const deleted = await deleteShare(share.id);
        setResultById((current) => ({ ...current, [share.id]: deleted ? tx("share deleted") : tx("share not found") }));
        await refresh();
        return;
      }
      if (action === "connectInfo") {
        const info = await loadShareConnectInfo(share.id);
        setConnectInfoById((current) => ({ ...current, [share.id]: info }));
        setResultById((current) => ({ ...current, [share.id]: info.directUrl }));
        return;
      }
      const next =
        action === "pause"
          ? await pauseShare(share.id)
          : action === "resume"
            ? await resumeShare(share.id)
            : action === "startTunnel"
              ? await startShareTunnel(share.id)
              : action === "stopTunnel"
                ? await stopShareTunnel(share.id)
                : await resetShareUsage(share.id);
      setResultById((current) => ({
        ...current,
        [share.id]: tx("{{action}} ok", { action: tx(shareActionLabel(action)) }),
      }));
      replaceShareInState(next);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusyId(null);
    }
  }

  async function loadMarketsAction() {
    setBusyId("markets");
    setError(null);
    try {
      setMarkets(await loadShareMarkets());
      setMarketsLoaded(true);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusyId(null);
    }
  }

  async function toolbarAction(action: "restore" | "snapshot" | "edits") {
    setBusyId(action);
    setError(null);
    try {
      if (action === "restore") {
        const shares = await restoreShareTunnels();
        setData((current) => ({ ...current, shares }));
        return;
      }
      if (action === "snapshot") {
        const shares = await refreshShareRuntimeSnapshots();
        setData((current) => ({ ...current, shares }));
        return;
      }
      const result = await pullRouterShareEdits();
      setResultById((current) => ({ ...current, __global: JSON.stringify(result.summary) }));
      await refresh();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusyId(null);
    }
  }

  async function exportSharesAction() {
    setBusyId("share-export");
    setError(null);
    try {
      const shares = await exportShares();
      const text = JSON.stringify(shares, null, 2);
      setExportText(text);
      let copied = false;
      try {
        if (navigator.clipboard) {
          await navigator.clipboard.writeText(text);
          copied = true;
        }
      } catch {
        copied = false;
      }
      setResultById((current) => ({
        ...current,
        __global: copied
          ? tx("exported {{count}} shares to clipboard", { count: shares.length })
          : tx("exported {{count}} shares", { count: shares.length }),
      }));
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusyId(null);
    }
  }

  async function importSharesAction(shares: ShareRecord[]) {
    setBusyId("share-import");
    setError(null);
    try {
      const imported = await importShares(shares);
      setImportOpen(false);
      setResultById((current) => ({ ...current, __global: tx("imported {{count}} shares", { count: imported }) }));
      await refresh();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusyId(null);
    }
  }

  function replaceShareInState(share: ShareRecord) {
    setData((current) => ({
      ...current,
      shares: current.shares.map((item) => (item.id === share.id ? share : item)),
    }));
  }

  return (
    <div className="share-dashboard">
      <div className="provider-toolbar">
        <div className="section-title-row">
          <Share2 size={18} />
          <div>
            <h2>{t("server.shares.title")}</h2>
            <span>{t("server.shares.routes", { count: data.shares.length })}</span>
          </div>
        </div>
        <div className="provider-toolbar-actions">
          {error && <span className="error-text">{error}</span>}
          <button className="secondary-button" type="button" onClick={() => void refresh()}>
            <RefreshCw size={15} />
            <span>{t("common.refresh")}</span>
          </button>
          <button className="secondary-button" type="button" onClick={() => void toolbarAction("snapshot")} disabled={busyId === "snapshot"}>
            {busyId === "snapshot" ? <Loader2 size={15} /> : <FileJson size={15} />}
            <span>{t("server.shares.runtime")}</span>
          </button>
          <button className="secondary-button" type="button" onClick={() => void toolbarAction("restore")} disabled={busyId === "restore"}>
            {busyId === "restore" ? <Loader2 size={15} /> : <Cable size={15} />}
            <span>{t("server.shares.restoreTunnels")}</span>
          </button>
          <button className="secondary-button" type="button" onClick={() => void toolbarAction("edits")} disabled={busyId === "edits"}>
            {busyId === "edits" ? <Loader2 size={15} /> : <Route size={15} />}
            <span>{t("server.shares.pullEdits")}</span>
          </button>
          <button className="secondary-button" type="button" onClick={() => void loadMarketsAction()} disabled={busyId === "markets"}>
            {busyId === "markets" ? <Loader2 size={15} /> : <Store size={15} />}
            <span>{t("server.shares.markets")}</span>
          </button>
          <button className="secondary-button" type="button" onClick={() => void exportSharesAction()} disabled={busyId === "share-export"}>
            {busyId === "share-export" ? <Loader2 size={15} /> : <Download size={15} />}
            <span>{t("server.common.export")}</span>
          </button>
          <button className="secondary-button" type="button" onClick={() => setImportOpen(true)}>
            <Upload size={15} />
            <span>{t("common.import")}</span>
          </button>
          <button
            className="primary-button"
            type="button"
            onClick={() => setDraft(createShareDraft(providersByApp))}
            disabled={!data.providers.length}
          >
            <Share2 size={15} />
            <span>{t("server.shares.createShare")}</span>
          </button>
        </div>
      </div>

      <div className="share-stats-bar">
        <ShareStat label={t("server.shares.active")} value={data.shares.filter((share) => share.status === "active").length} />
        <ShareStat label={t("server.shares.paused")} value={data.shares.filter((share) => share.status === "paused").length} />
        <ShareStat label={t("server.shares.forSale")} value={data.shares.filter((share) => share.forSale).length} />
        <ShareStat label={t("server.shares.requests")} value={data.shares.reduce((sum, share) => sum + (share.requestsCount || 0), 0)} />
      </div>

      <ShareTunnelConfigPanel
        shares={data.shares}
        markets={markets}
        marketsLoaded={marketsLoaded}
        busyId={busyId}
        onSnapshot={() => void toolbarAction("snapshot")}
        onRestore={() => void toolbarAction("restore")}
        onPullEdits={() => void toolbarAction("edits")}
        onLoadMarkets={() => void loadMarketsAction()}
      />

      {resultById.__global && <div className="share-global-result">{resultById.__global}</div>}

      {loading ? (
        <div className="provider-empty">
          <Loader2 size={22} />
          <span>{t("server.shares.loading")}</span>
        </div>
      ) : data.shares.length ? (
        <div className="share-card-grid">
          {data.shares.map((share) => (
            <ShareCard
              key={share.id}
              share={share}
              providerByKey={providerByKey}
              markets={markets}
              marketsLoaded={marketsLoaded}
              result={resultById[share.id]}
              connectInfo={connectInfoById[share.id]}
              busyId={busyId}
              onEdit={() => setDraft(editShareDraft(share, providersByApp))}
              onAcl={() => setAclDraft(share)}
              onSubdomain={() => setSubdomainDraft(share)}
              onBinding={(app) => setBindingDraft(createBindingDraft(share, app))}
              onMarket={() => setMarketDraft(share)}
              onAction={(action) => void runShareAction(share, action)}
            />
          ))}
        </div>
      ) : (
        <div className="provider-empty">
          <Share2 size={24} />
          <strong>{t("server.shares.noShares")}</strong>
          <span>{t("server.shares.noSharesHint")}</span>
        </div>
      )}

      <ShareRequestLogPanel logs={shareRequestLogs} shares={data.shares} />

      {draft && (
        <ShareFormModal
          draft={draft}
          providersByApp={providersByApp}
          saving={busyId === "share-save"}
          onChange={setDraft}
          onClose={() => setDraft(null)}
          onSubmit={async (event) => {
            event.preventDefault();
            if (shareOwnerChanged(draft)) {
              setOwnerChangeDraft(draft);
              setDraft(null);
              return;
            }
            setBusyId("share-save");
            setError(null);
            try {
              const share = await saveShare(shareInputFromDraft(draft, providersByApp));
              setDraft(null);
              await refresh();
              setResultById((current) => ({ ...current, [share.id]: "share saved" }));
            } catch (reason) {
              setError(errorMessage(reason));
            } finally {
              setBusyId(null);
            }
          }}
        />
      )}

      {ownerChangeDraft && (
        <OwnerChangeModal
          draft={ownerChangeDraft}
          saving={busyId === "share-owner"}
          onClose={() => setOwnerChangeDraft(null)}
          onRequestCode={async () => {
            setBusyId("share-owner");
            setError(null);
            try {
              return await requestShareOwnerChangeCode(ownerChangeDraft.id, ownerChangeDraft.ownerEmail);
            } catch (reason) {
              setError(errorMessage(reason));
              throw reason;
            } finally {
              setBusyId(null);
            }
          }}
          onVerify={async (code) => {
            setBusyId("share-owner");
            setError(null);
            try {
              await verifyShareOwnerChangeCode({
                id: ownerChangeDraft.id,
                newOwnerEmail: ownerChangeDraft.ownerEmail,
                code,
              });
              const share = await saveShare(shareInputFromDraft(ownerChangeDraft, providersByApp));
              setOwnerChangeDraft(null);
              await refresh();
              setResultById((current) => ({ ...current, [share.id]: "owner verified and share saved" }));
            } catch (reason) {
              setError(errorMessage(reason));
            } finally {
              setBusyId(null);
            }
          }}
        />
      )}

      {aclDraft && (
        <AclModal
          share={aclDraft}
          saving={busyId === "share-acl"}
          onClose={() => setAclDraft(null)}
          onSubmit={async (acl) => {
            setBusyId("share-acl");
            setError(null);
            try {
              replaceShareInState(await replaceShareAcl(aclDraft.id, acl));
              setAclDraft(null);
            } catch (reason) {
              setError(errorMessage(reason));
            } finally {
              setBusyId(null);
            }
          }}
        />
      )}

      {subdomainDraft && (
        <SubdomainModal
          share={subdomainDraft}
          saving={busyId === "share-subdomain"}
          onClose={() => setSubdomainDraft(null)}
          onSubmit={async (subdomain) => {
            setBusyId("share-subdomain");
            setError(null);
            try {
              const result = await updateShareSubdomain(subdomainDraft.id, subdomain);
              replaceShareInState(result.share);
              setResultById((current) => ({
                ...current,
                [subdomainDraft.id]: result.remoteClaimed ? "subdomain claimed remotely" : "subdomain saved locally",
              }));
              setSubdomainDraft(null);
            } catch (reason) {
              setError(errorMessage(reason));
            } finally {
              setBusyId(null);
            }
          }}
        />
      )}

      {bindingDraft && (
        <BindingModal
          draft={bindingDraft}
          providers={providersByApp[bindingDraft.app]}
          saving={busyId === "share-binding"}
          onChange={setBindingDraft}
          onClose={() => setBindingDraft(null)}
          onSubmit={async () => {
            setBusyId("share-binding");
            setError(null);
            try {
              const provider = providersByApp[bindingDraft.app].find(
                (item) => item.provider.id === bindingDraft.providerId,
              );
              if (!provider) throw new Error(tx("provider is required for binding"));
              replaceShareInState(
                await updateShareBinding(bindingDraft.share.id, {
                  app: bindingDraft.app,
                  providerId: provider.provider.id,
                  providerType: provider.providerTypeId,
                }),
              );
              setBindingDraft(null);
            } catch (reason) {
              setError(errorMessage(reason));
            } finally {
              setBusyId(null);
            }
          }}
        />
      )}

      {marketDraft && (
        <MarketModal
          share={marketDraft}
          markets={markets}
          marketsLoaded={marketsLoaded}
          saving={busyId === "share-market"}
          onLoadMarkets={() => void loadMarketsAction()}
          onClose={() => setMarketDraft(null)}
          onSubmit={async (marketEmail) => {
            setBusyId("share-market");
            setError(null);
            try {
              replaceShareInState(await authorizeShareMarket(marketDraft.id, marketEmail));
              setMarketDraft(null);
            } catch (reason) {
              setError(errorMessage(reason));
            } finally {
              setBusyId(null);
            }
          }}
        />
      )}

      {importOpen && (
        <ImportSharesModal
          saving={busyId === "share-import"}
          onClose={() => setImportOpen(false)}
          onSubmit={(shares) => void importSharesAction(shares)}
        />
      )}

      {exportText && (
        <SimpleModal
          title="Export Shares"
          subtitle="Copy this JSON when clipboard access is unavailable."
          onClose={() => setExportText(null)}
        >
          <textarea readOnly value={exportText} />
          <footer className="modal-inline-footer">
            <button className="secondary-button" type="button" onClick={() => setExportText(null)}>
              {tx("Close")}
            </button>
          </footer>
        </SimpleModal>
      )}
    </div>
  );
}

function ShareCard({
  share,
  providerByKey,
  markets,
  marketsLoaded,
  result,
  connectInfo,
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
  const usage = shareUsage(share);
  const syncText = share.routerLastSyncError || formatTime(share.routerLastSyncedAtMs);
  const market = markets.find((item) => item.email === share.acl?.publicMarketEmail);
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
        <ShareStatus label="ACL" value={(share.acl?.sharedWithEmails || []).length ? `${share.acl?.sharedWithEmails.length} users` : "private"} />
        <ShareStatus label="sale" value={share.forSale ? share.saleMarketKind || "share" : "no"} />
        <ShareStatus label="market" value={market?.displayName || share.acl?.publicMarketEmail || (marketsLoaded ? "-" : "not loaded")} />
        <ShareStatus label="grant" value={share.marketGrant?.status || "-"} />
      </div>

      <ShareRuntimePanel share={share} />

      {(result || share.lastError) && <div className="provider-card-result">{result || share.lastError}</div>}

      {connectInfo && (
        <details className="json-details" open>
          <summary>{tx("Connect info")}</summary>
          <div className="connect-info-block">
            <KeyValue label="direct URL" value={connectInfo.directUrl} />
            <button
              className="secondary-button compact"
              type="button"
              onClick={() => void navigator.clipboard?.writeText(JSON.stringify(connectInfo, null, 2))}
            >
              <Copy size={13} />
              <span>{tx("Copy JSON")}</span>
            </button>
            <JsonPreview value={connectInfo} />
          </div>
        </details>
      )}

      <div className="provider-actions">
        <IconAction title="Edit" onClick={onEdit}>
          <Edit3 size={15} />
        </IconAction>
        <IconAction title="ACL" onClick={onAcl}>
          <Users size={15} />
        </IconAction>
        <IconAction title="Subdomain" onClick={onSubdomain}>
          <Link2 size={15} />
        </IconAction>
        <IconAction title="Connect info" busy={busyId === `${share.id}:connectInfo`} onClick={() => onAction("connectInfo")}>
          <FileJson size={15} />
        </IconAction>
        <IconAction title={share.status === "paused" ? "Resume" : "Pause"} busy={busyId === `${share.id}:${share.status === "paused" ? "resume" : "pause"}`} onClick={() => onAction(share.status === "paused" ? "resume" : "pause")}>
          {share.status === "paused" ? <Play size={15} /> : <Pause size={15} />}
        </IconAction>
        <IconAction title="Start tunnel" busy={busyId === `${share.id}:startTunnel`} onClick={() => onAction("startTunnel")}>
          <Cable size={15} />
        </IconAction>
        <IconAction title="Stop tunnel" busy={busyId === `${share.id}:stopTunnel`} onClick={() => onAction("stopTunnel")}>
          <SlidersHorizontal size={15} />
        </IconAction>
        <IconAction title="Reset usage" busy={busyId === `${share.id}:resetUsage`} onClick={() => onAction("resetUsage")}>
          <RotateCcw size={15} />
        </IconAction>
        <IconAction title="Authorize market" onClick={onMarket}>
          <Store size={15} />
        </IconAction>
        <IconAction title="Delete" busy={busyId === `${share.id}:delete`} onClick={() => setDeleteConfirmOpen(true)} danger>
          <Trash2 size={15} />
        </IconAction>
      </div>
    </article>
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

function ShareRequestLogPanel({ logs, shares }: { logs: UsageLog[]; shares: ShareRecord[] }) {
  const { tx } = useI18n();
  const shareById = new Map(shares.map((share) => [share.id, share]));
  return (
    <section className="share-request-log-panel">
      <div className="section-heading">
        <div className="section-title-row compact-title">
          <FileJson size={16} />
          <div>
            <h2>{tx("Request Logs")}</h2>
            <span>{tx("{{count}} recent share requests", { count: logs.length })}</span>
          </div>
        </div>
      </div>
      {logs.length ? (
        <div className="share-request-log-list">
          {logs.slice(0, 80).map((log) => (
            <ShareRequestLogCard
              key={log.requestId}
              log={log}
              share={log.shareId ? shareById.get(log.shareId) : undefined}
            />
          ))}
        </div>
      ) : (
        <div className="provider-empty compact-empty">
          <FileJson size={20} />
          <span>{tx("No share request logs")}</span>
        </div>
      )}
    </section>
  );
}

function ShareRequestLogCard({ log, share }: { log: UsageLog; share?: ShareRecord }) {
  const { tx } = useI18n();
  const app = appIcon(log.app);
  const model = log.actualModel || log.requestedModel || log.model || "-";
  const ok = log.statusCode >= 200 && log.statusCode < 400;
  return (
    <article className="share-request-log-card">
      <header>
        <div className="share-request-title">
          <span className="provider-icon-frame small">
            <ProviderIcon icon={app.icon} color={app.color} name={appLabel(log.app)} size={18} />
          </span>
          <div>
            <strong title={log.shareId || undefined}>{log.shareName || (share ? shareName(share) : log.shareId || "-")}</strong>
            <span title={model}>{model}</span>
          </div>
        </div>
        <div className="share-request-status">
          <StatusPill tone={ok ? "success" : "danger"}>{log.statusCode || "-"}</StatusPill>
          <small>{formatTime(log.createdAtMs)}</small>
        </div>
      </header>
      <div className="share-request-metrics">
        <KeyValue label="app" value={appLabel(log.app)} />
        <KeyValue label="tokens" value={formatTokens(log.totalTokens)} />
        <KeyValue label="cost" value={formatUsd(log.totalCostUsd)} />
        <KeyValue label="latency" value={formatDuration(log.durationMs)} />
      </div>
      <div className="share-request-tags">
        <span>{log.userEmail || tx("anonymous")}</span>
        {log.dataSource && <span>{log.dataSource}</span>}
        {log.streamStatus && <span>{tx(log.streamStatus)}</span>}
        {log.isHealthCheck && <span>{tx("health")}</span>}
      </div>
    </article>
  );
}

function ShareTunnelConfigPanel({
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

function ShareFormModal({
  draft,
  providersByApp,
  saving,
  onChange,
  onClose,
  onSubmit,
}: {
  draft: ShareDraft;
  providersByApp: Record<AppKind, StoredProvider[]>;
  saving: boolean;
  onChange: (draft: ShareDraft) => void;
  onClose: () => void;
  onSubmit: (event: FormEvent) => void;
}) {
  const { tx } = useI18n();
  function patch(next: Partial<ShareDraft>) {
    onChange({ ...draft, ...next });
  }
  function patchBinding(app: AppKind, providerId: string) {
    onChange({ ...draft, bindings: { ...draft.bindings, [app]: providerId } });
  }
  return (
    <div className="modal-backdrop" role="presentation">
      <form className="provider-form-modal share-form-modal" onSubmit={onSubmit}>
        <header>
          <div>
            <h2>{tx(draft.mode === "create" ? "Create Share" : "Edit Share")}</h2>
            <p>{tx("Share routes expose selected providers through router and market flows.")}</p>
          </div>
          <button className="icon-button" type="button" onClick={onClose} aria-label={tx("Close")}>
            <X size={16} />
          </button>
        </header>
        <div className="provider-form-grid">
          <label>
            <span>{tx("Name")}</span>
            <input value={draft.displayName} onChange={(event) => patch({ displayName: event.target.value })} />
          </label>
          <label>
            <span>{tx("Owner email")}</span>
            <input value={draft.ownerEmail} onChange={(event) => patch({ ownerEmail: event.target.value })} />
          </label>
          <label>
            <span>{tx("Primary app")}</span>
            <select value={draft.primaryApp} onChange={(event) => patch({ primaryApp: event.target.value as AppKind })}>
              {apps.map((app) => (
                <option key={app.id} value={app.id}>
                  {app.label}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>{tx("Status")}</span>
            <select value={draft.status} onChange={(event) => patch({ status: event.target.value })}>
              <option value="active">active</option>
              <option value="paused">paused</option>
              <option value="disabled">disabled</option>
            </select>
          </label>
          {apps.map((app) => (
            <label key={app.id}>
              <span>{tx("{{app}} provider", { app: app.label })}</span>
              <select value={draft.bindings[app.id]} onChange={(event) => patchBinding(app.id, event.target.value)}>
                <option value="">{tx("Unbound")}</option>
                {providersByApp[app.id].map((provider) => (
                  <option key={provider.provider.id} value={provider.provider.id}>
                    {provider.provider.name} ({provider.providerTypeId})
                  </option>
                ))}
              </select>
            </label>
          ))}
          <label>
            <span>{tx("Token limit")}</span>
            <input value={draft.tokenLimit} onChange={(event) => patch({ tokenLimit: event.target.value })} />
          </label>
          <label>
            <span>{tx("Parallel limit")}</span>
            <input value={draft.parallelLimit} onChange={(event) => patch({ parallelLimit: event.target.value })} />
          </label>
          <label>
            <span>{tx("Expires at")}</span>
            <input type="datetime-local" value={draft.expiresAt} onChange={(event) => patch({ expiresAt: event.target.value })} />
          </label>
          <label>
            <span>{tx("Subdomain")}</span>
            <input value={draft.subdomain} onChange={(event) => patch({ subdomain: event.target.value })} />
          </label>
          <label>
            <span>{tx("Sale kind")}</span>
            <select value={draft.saleMarketKind} onChange={(event) => patch({ saleMarketKind: event.target.value })}>
              <option value="share">share</option>
              <option value="token">token</option>
            </select>
          </label>
          <label>
            <span>{tx("Official price %")}</span>
            <input value={draft.officialPricePercent} onChange={(event) => patch({ officialPricePercent: event.target.value })} />
          </label>
          <label className="toggle-row">
            <input type="checkbox" checked={draft.enabled} onChange={(event) => patch({ enabled: event.target.checked })} />
            <span>{tx("Enabled")}</span>
          </label>
          <label className="toggle-row">
            <input type="checkbox" checked={draft.autoStart} onChange={(event) => patch({ autoStart: event.target.checked })} />
            <span>{tx("Auto start tunnel")}</span>
          </label>
          <label className="toggle-row">
            <input type="checkbox" checked={draft.forSale} onChange={(event) => patch({ forSale: event.target.checked })} />
            <span>{tx("For sale")}</span>
          </label>
          <label>
            <span>{tx("Market ACL mode")}</span>
            <select value={draft.marketAccessMode} onChange={(event) => patch({ marketAccessMode: event.target.value })}>
              <option value="selected">selected</option>
              <option value="all">all</option>
            </select>
          </label>
          <label className="wide-field">
            <span>{tx("Shared emails")}</span>
            <input value={draft.aclEmails} onChange={(event) => patch({ aclEmails: event.target.value })} />
          </label>
          <label className="wide-field">
            <span>{tx("Description")}</span>
            <textarea value={draft.description} onChange={(event) => patch({ description: event.target.value })} />
          </label>
        </div>
        <footer>
          <button className="secondary-button" type="button" onClick={onClose}>
            {tx("Cancel")}
          </button>
          <button className="primary-button" type="submit" disabled={saving}>
            {saving && <Loader2 size={15} />}
            <span>{tx("Save Share")}</span>
          </button>
        </footer>
      </form>
    </div>
  );
}

function AclModal({
  share,
  saving,
  onClose,
  onSubmit,
}: {
  share: ShareRecord;
  saving: boolean;
  onClose: () => void;
  onSubmit: (acl: ShareAcl) => void;
}) {
  const { tx } = useI18n();
  const [emails, setEmails] = useState((share.acl?.sharedWithEmails || []).join(", "));
  const [marketAccessMode, setMarketAccessMode] = useState(share.acl?.marketAccessMode || "selected");
  const [publicMarketEmail, setPublicMarketEmail] = useState(share.acl?.publicMarketEmail || "");
  return (
    <SimpleModal title="Share ACL" subtitle={shareName(share)} onClose={onClose}>
      <form
        className="modal-form-stack"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit({
            sharedWithEmails: splitList(emails),
            marketAccessMode,
            publicMarketEmail: publicMarketEmail.trim() || null,
          });
        }}
      >
        <label>
          <span>{tx("Shared emails")}</span>
          <input value={emails} onChange={(event) => setEmails(event.target.value)} />
        </label>
        <label>
          <span>{tx("Public market email")}</span>
          <input value={publicMarketEmail} onChange={(event) => setPublicMarketEmail(event.target.value)} />
        </label>
        <label>
          <span>{tx("Market mode")}</span>
          <select value={marketAccessMode} onChange={(event) => setMarketAccessMode(event.target.value)}>
            <option value="selected">selected</option>
            <option value="all">all</option>
          </select>
        </label>
        <ModalFooter saving={saving} onClose={onClose} label="Save ACL" />
      </form>
    </SimpleModal>
  );
}

function SubdomainModal({
  share,
  saving,
  onClose,
  onSubmit,
}: {
  share: ShareRecord;
  saving: boolean;
  onClose: () => void;
  onSubmit: (subdomain: string) => void;
}) {
  const { tx } = useI18n();
  const [subdomain, setSubdomain] = useState(share.tunnelSubdomain || "");
  return (
    <SimpleModal title="Share Subdomain" subtitle={shareName(share)} onClose={onClose}>
      <form
        className="modal-form-stack"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit(subdomain);
        }}
      >
        <label>
          <span>{tx("Subdomain")}</span>
          <input value={subdomain} onChange={(event) => setSubdomain(event.target.value)} />
        </label>
        <ModalFooter saving={saving} onClose={onClose} label="Save Subdomain" />
      </form>
    </SimpleModal>
  );
}

function BindingModal({
  draft,
  providers,
  saving,
  onChange,
  onClose,
  onSubmit,
}: {
  draft: BindingDraft;
  providers: StoredProvider[];
  saving: boolean;
  onChange: (draft: BindingDraft) => void;
  onClose: () => void;
  onSubmit: () => void;
}) {
  const { tx } = useI18n();
  return (
    <SimpleModal title={`${appLabel(draft.app)} Binding`} subtitle="Share must be paused before binding changes are accepted." onClose={onClose}>
      <form
        className="modal-form-stack"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit();
        }}
      >
        <label>
          <span>{tx("Provider")}</span>
          <select value={draft.providerId} onChange={(event) => onChange({ ...draft, providerId: event.target.value })}>
            <option value="">{tx("Select provider")}</option>
            {providers.map((provider) => (
              <option key={provider.provider.id} value={provider.provider.id}>
                {provider.provider.name} ({provider.providerTypeId})
              </option>
            ))}
          </select>
        </label>
        <ModalFooter saving={saving} onClose={onClose} label="Update Binding" />
      </form>
    </SimpleModal>
  );
}

function MarketModal({
  share,
  markets,
  marketsLoaded,
  saving,
  onLoadMarkets,
  onClose,
  onSubmit,
}: {
  share: ShareRecord;
  markets: PublicShareMarket[];
  marketsLoaded: boolean;
  saving: boolean;
  onLoadMarkets: () => void;
  onClose: () => void;
  onSubmit: (marketEmail: string) => void;
}) {
  const { tx } = useI18n();
  const shareMarkets = markets.filter((market) => market.marketKind === "share");
  const [marketEmail, setMarketEmail] = useState(shareMarkets[0]?.email || "");
  useEffect(() => {
    if (!marketEmail && shareMarkets[0]?.email) setMarketEmail(shareMarkets[0].email);
  }, [marketEmail, shareMarkets]);
  return (
    <SimpleModal title="Authorize Share Market" subtitle={shareName(share)} onClose={onClose}>
      <form
        className="modal-form-stack"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit(marketEmail);
        }}
      >
        {!marketsLoaded && (
          <button className="secondary-button" type="button" onClick={onLoadMarkets}>
            <Store size={15} />
            <span>{tx("Load markets")}</span>
          </button>
        )}
        <label>
          <span>{tx("Share market")}</span>
          <select value={marketEmail} onChange={(event) => setMarketEmail(event.target.value)}>
            <option value="">{tx("Select market")}</option>
            {shareMarkets.map((market) => (
              <option key={market.id} value={market.email}>
                {market.displayName} ({market.email})
              </option>
            ))}
          </select>
        </label>
        <ModalFooter saving={saving} onClose={onClose} label="Authorize" />
      </form>
    </SimpleModal>
  );
}

function OwnerChangeModal({
  draft,
  saving,
  onClose,
  onRequestCode,
  onVerify,
}: {
  draft: ShareDraft;
  saving: boolean;
  onClose: () => void;
  onRequestCode: () => Promise<{ maskedDestination: string; cooldownSecs: number }>;
  onVerify: (code: string) => Promise<void>;
}) {
  const { tx } = useI18n();
  const [code, setCode] = useState("");
  const [result, setResult] = useState<string | null>(null);
  const [localError, setLocalError] = useState<string | null>(null);
  const hasCode = code.trim().length > 0;
  return (
    <SimpleModal title="Verify Share Owner" subtitle={draft.ownerEmail} onClose={onClose}>
      <form
        className="modal-form-stack owner-change-form"
        onSubmit={(event) => {
          event.preventDefault();
          setLocalError(null);
          void onVerify(code).catch((reason) => setLocalError(errorMessage(reason)));
        }}
      >
        <section className="owner-change-panel">
          <header>
            <span className="provider-icon-frame">
              <Users size={20} />
            </span>
            <div>
              <h3>{tx("Owner handoff")}</h3>
              <p>{tx("Email verification is required before saving this share owner change.")}</p>
            </div>
          </header>
          <div className="owner-change-flow">
            <OwnerNode label="current owner" value={draft.originalOwnerEmail || "-"} muted />
            <span className="owner-change-arrow">-&gt;</span>
            <OwnerNode label="new owner" value={draft.ownerEmail || "-"} />
          </div>
          <div className="owner-change-steps">
            <OwnerStep label="request code" active />
            <OwnerStep label="verify email" active={Boolean(result) || hasCode} />
            <OwnerStep label="save share" active={hasCode} />
          </div>
        </section>
        <button
          className="secondary-button owner-request-button"
          type="button"
          disabled={saving}
          onClick={() => {
            setLocalError(null);
            void onRequestCode()
              .then((response) =>
                setResult(
                  response.cooldownSecs
                    ? tx("code sent to {{destination}}; cooldown {{seconds}}s", {
                        destination: response.maskedDestination,
                        seconds: response.cooldownSecs,
                      })
                    : tx("code sent to {{destination}}", { destination: response.maskedDestination }),
                ),
              )
              .catch((reason) => setLocalError(errorMessage(reason)));
          }}
        >
          {saving ? <Loader2 size={15} /> : <RefreshCw size={15} />}
          <span>{tx("Request Code")}</span>
        </button>
        <label>
          <span>{tx("Email code")}</span>
          <input value={code} onChange={(event) => setCode(event.target.value)} required />
        </label>
        {result && <div className="provider-card-result">{result}</div>}
        {localError && <div className="form-error">{localError}</div>}
        <ModalFooter saving={saving} disabled={!hasCode} onClose={onClose} label="Verify Owner" />
      </form>
    </SimpleModal>
  );
}

function OwnerNode({ label, value, muted = false }: { label: string; value: string; muted?: boolean }) {
  const { tx } = useI18n();
  return (
    <div className={muted ? "owner-node muted" : "owner-node"}>
      <span>{tx(label)}</span>
      <strong title={value}>{value}</strong>
    </div>
  );
}

function OwnerStep({ label, active }: { label: string; active: boolean }) {
  const { tx } = useI18n();
  return (
    <div className={active ? "owner-step active" : "owner-step"}>
      <span />
      <strong>{tx(label)}</strong>
    </div>
  );
}

function ImportSharesModal({
  saving,
  onClose,
  onSubmit,
}: {
  saving: boolean;
  onClose: () => void;
  onSubmit: (shares: ShareRecord[]) => void;
}) {
  const { tx } = useI18n();
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  return (
    <SimpleModal title="Import Shares" subtitle="Paste an exported array or { shares } object." onClose={onClose}>
      <form
        className="modal-form-stack"
        onSubmit={(event) => {
          event.preventDefault();
          try {
            const parsed = JSON.parse(text) as { shares?: ShareRecord[] } | ShareRecord[];
            const shares = Array.isArray(parsed) ? parsed : parsed.shares;
            if (!shares?.length) throw new Error(tx("shares array is required"));
            onSubmit(shares);
          } catch (reason) {
            setError(errorMessage(reason));
          }
        }}
      >
        {error && <div className="form-error">{error}</div>}
        <textarea value={text} onChange={(event) => setText(event.target.value)} />
        <ModalFooter saving={saving} onClose={onClose} label="Import Shares" />
      </form>
    </SimpleModal>
  );
}

function SimpleModal({
  title,
  subtitle,
  children,
  onClose,
}: {
  title: string;
  subtitle?: string;
  children: ReactNode;
  onClose: () => void;
}) {
  const { tx } = useI18n();
  return (
    <div className="modal-backdrop" role="presentation">
      <section className="provider-form-modal simple-modal">
        <header>
          <div>
            <h2>{tx(title)}</h2>
            {subtitle && <p>{tx(subtitle)}</p>}
          </div>
          <button className="icon-button" type="button" onClick={onClose} aria-label={tx("Close")}>
            <X size={16} />
          </button>
        </header>
        <div className="simple-modal-body">{children}</div>
      </section>
    </div>
  );
}

function ModalFooter({
  saving,
  disabled = false,
  onClose,
  label,
}: {
  saving: boolean;
  disabled?: boolean;
  onClose: () => void;
  label: string;
}) {
  const { tx } = useI18n();
  return (
    <footer className="modal-inline-footer">
      <button className="secondary-button" type="button" onClick={onClose}>
        {tx("Cancel")}
      </button>
      <button className="primary-button" type="submit" disabled={saving || disabled}>
        {saving && <Loader2 size={15} />}
        <span>{tx(label)}</span>
      </button>
    </footer>
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

function KeyValue({ label, value }: { label: string; value: ReactNode }) {
  const { tx } = useI18n();
  return (
    <div className="compact-kv">
      <span>{tx(label)}</span>
      <strong>{value}</strong>
    </div>
  );
}

function ShareRuntimePanel({ share }: { share: ShareRecord }) {
  const { tx } = useI18n();
  const snapshot = asRecord(share.runtimeSnapshot);
  if (!snapshot) {
    return (
      <section className="share-runtime-panel">
        <div className="compact-empty">
          <span>{tx("No runtime snapshot")}</span>
        </div>
      </section>
    );
  }
  const health = asRecord(snapshot.health);
  const lastRequest = asRecord(snapshot.lastRequest);
  const upstreamProvider = asRecord(snapshot.upstreamProvider);
  const availability = asRecord(snapshot.appAvailability);
  const appProviders = asRecord(snapshot.appProviders);
  const appRuntimes = asRecord(snapshot.appRuntimes);
  const healthStatus = providerHealthStatus(health);
  const modelHealth = modelHealthResults(snapshot.modelHealth);
  return (
    <section className="share-runtime-panel">
      <div className="share-runtime-grid">
        <RuntimeValue label="updated" value={formatTime(numberValue(snapshot.updatedAtMs))} />
        <RuntimeValue label="provider" value={stringValue(snapshot.providerName) || share.providerType || "-"} />
        <RuntimeValue label="account" value={stringValue(snapshot.accountEmail) || stringValue(upstreamProvider?.accountEmail) || share.accountEmail || "-"} />
        <RuntimeValue label="plan" value={stringValue(snapshot.subscriptionLevel) || stringValue(upstreamProvider?.subscriptionLevel) || share.subscriptionLevel || "-"} />
        <RuntimeValue label="quota" value={formatPercent(numberValue(snapshot.quotaPercent) ?? share.quotaPercent)} />
        <RuntimeValue label="health" value={healthStatus.label} tone={healthStatus.tone} />
      </div>

      {lastRequest && (
        <div className="runtime-mini-row">
          <span>{tx("last request")}</span>
          <strong>{stringValue(lastRequest.requestId) || "-"}</strong>
          <span>{numberValue(lastRequest.statusCode) ?? "-"}</span>
          <span>{modelPair(lastRequest)}</span>
          <span>{formatTime(numberValue(lastRequest.createdAtMs))}</span>
        </div>
      )}

      <div className="share-app-runtime-list">
        {apps.map((app) => (
          <ShareAppRuntimeRow
            key={app.id}
            app={app.id}
            availability={asRecord(availability?.[app.id])}
            providers={arrayRecords(appProviders?.[app.id])}
            runtime={asRecord(appRuntimes?.[app.id])}
          />
        ))}
      </div>

      {modelHealth.length > 0 && (
        <div className="model-health-list">
          {modelHealth.map((item, index) => (
            <div className="model-health-row" key={`${item.app}:${item.providerId || item.requestedModel}:${index}`}>
              <StatusPill tone={modelHealthTone(item.status)}>{item.status || "unknown"}</StatusPill>
              <div>
                <strong>{appLabel(item.app)} · {item.providerName || item.providerId || "provider"}</strong>
                <span>{`${item.requestedModel || "-"} -> ${item.actualModel || "-"}`}</span>
              </div>
              <span>{item.statusCode ?? "-"}</span>
              <span>{item.latencyMs == null ? "-" : `${item.latencyMs}ms`}</span>
              <span>{formatHealthCheckedAt(item.checkedAt)}</span>
            </div>
          ))}
        </div>
      )}

      <details className="json-details">
        <summary>{tx("Runtime JSON")}</summary>
        <JsonPreview value={snapshot} />
      </details>
    </section>
  );
}

function RuntimeValue({
  label,
  value,
  tone,
}: {
  label: string;
  value: ReactNode;
  tone?: "success" | "warning" | "danger";
}) {
  const { tx } = useI18n();
  return (
    <div className="share-runtime-value">
      <span>{tx(label)}</span>
      {tone ? <StatusPill tone={tone}>{value}</StatusPill> : <strong>{value}</strong>}
    </div>
  );
}

function ShareAppRuntimeRow({
  app,
  availability,
  providers,
  runtime,
}: {
  app: AppKind;
  availability?: Record<string, unknown>;
  providers: Array<Record<string, unknown>>;
  runtime?: Record<string, unknown>;
}) {
  const { tx } = useI18n();
  const available = booleanValue(availability?.available);
  const providerName =
    stringValue(runtime?.name) ||
    stringValue(runtime?.providerName) ||
    providers.map((provider) => stringValue(provider.name)).find(Boolean) ||
    "-";
  const reason = stringValue(availability?.reason);
  const quotaBlocked = booleanValue(availability?.quotaBlocked);
  return (
    <div className="share-app-runtime-row">
      <strong>{appLabel(app)}</strong>
      <span>{providerName}</span>
      <StatusPill tone={available === false || quotaBlocked ? "danger" : available === true ? "success" : "warning"}>
        {tx(quotaBlocked ? "quota" : available === false ? "blocked" : available === true ? "available" : "unknown")}
      </StatusPill>
      <span>{reason || `${providers.length} provider${providers.length === 1 ? "" : "s"}`}</span>
    </div>
  );
}

interface ModelHealthView {
  app: AppKind;
  requestedModel: string;
  actualModel: string;
  status: string;
  statusCode?: number;
  latencyMs?: number;
  checkedAt?: number;
  providerId?: string;
  providerName?: string;
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

function StatusPill({
  children,
  tone,
}: {
  children: ReactNode;
  tone: "success" | "warning" | "danger";
}) {
  return <span className={`status-pill ${tone}`}>{children}</span>;
}

function IconAction({
  title,
  children,
  busy,
  danger,
  onClick,
}: {
  title: string;
  children: ReactNode;
  busy?: boolean;
  danger?: boolean;
  onClick: () => void;
}) {
  const { tx } = useI18n();
  const translatedTitle = tx(title);
  return (
    <button
      className={danger ? "icon-button danger" : "icon-button"}
      type="button"
      title={translatedTitle}
      aria-label={translatedTitle}
      onClick={onClick}
      disabled={busy}
    >
      {busy ? <Loader2 size={15} /> : children}
    </button>
  );
}

function createShareDraft(providersByApp: Record<AppKind, StoredProvider[]>): ShareDraft {
  const firstApp = apps.find((app) => providersByApp[app.id].length)?.id || "claude";
  return {
    mode: "create",
    id: "",
    originalOwnerEmail: "",
    displayName: "",
    ownerEmail: "",
    primaryApp: firstApp,
    bindings: {
      claude: providersByApp.claude[0]?.provider.id || "",
      codex: providersByApp.codex[0]?.provider.id || "",
      gemini: providersByApp.gemini[0]?.provider.id || "",
    },
    enabled: true,
    status: "active",
    tokenLimit: "",
    parallelLimit: "",
    expiresAt: "",
    subdomain: "",
    description: "",
    autoStart: false,
    forSale: false,
    saleMarketKind: "share",
    officialPricePercent: "",
    aclEmails: "",
    marketAccessMode: "selected",
  };
}

function editShareDraft(share: ShareRecord, providersByApp: Record<AppKind, StoredProvider[]>): ShareDraft {
  const bindings = bindingMap(share);
  return {
    mode: "edit",
    id: share.id,
    originalOwnerEmail: share.ownerEmail || "",
    displayName: share.displayName || "",
    ownerEmail: share.ownerEmail || "",
    primaryApp: share.app || firstBoundApp(bindings) || firstProviderApp(providersByApp),
    bindings,
    enabled: share.enabled,
    status: share.status || "active",
    tokenLimit: share.tokenLimit?.toString() || "",
    parallelLimit: share.parallelLimit?.toString() || "",
    expiresAt: toDateTimeInput(share.expiresAt),
    subdomain: share.tunnelSubdomain || "",
    description: share.description || "",
    autoStart: share.autoStart,
    forSale: share.forSale,
    saleMarketKind: share.saleMarketKind || "share",
    officialPricePercent: share.officialPricePercent?.toString() || "",
    aclEmails: (share.acl?.sharedWithEmails || []).join(", "),
    marketAccessMode: share.acl?.marketAccessMode || "selected",
  };
}

function shareOwnerChanged(draft: ShareDraft): boolean {
  return (
    draft.mode === "edit" &&
    Boolean(draft.ownerEmail.trim()) &&
    draft.ownerEmail.trim().toLowerCase() !== draft.originalOwnerEmail.trim().toLowerCase()
  );
}

function shareInputFromDraft(
  draft: ShareDraft,
  providersByApp: Record<AppKind, StoredProvider[]>,
): UpsertShareInput {
  const bindings = apps
    .map((app) => {
      const providerId = draft.bindings[app.id];
      if (!providerId) return null;
      const provider = providersByApp[app.id].find((item) => item.provider.id === providerId);
      if (!provider) return null;
      return {
        app: app.id,
        providerId,
        providerType: provider.providerTypeId,
      };
    })
    .filter(Boolean) as ShareBinding[];
  if (!bindings.length) throw new Error("share requires at least one provider binding");
  const primary =
    bindings.find((binding) => binding.app === draft.primaryApp) ||
    bindings[0];
  const input: UpsertShareInput = {
    app: primary.app,
    providerId: primary.providerId,
    providerType: primary.providerType,
    bindings,
    enabled: draft.enabled,
    status: draft.status,
    forSale: draft.forSale,
    saleMarketKind: draft.saleMarketKind || "share",
    autoStart: draft.autoStart,
    acl: {
      sharedWithEmails: splitList(draft.aclEmails),
      marketAccessMode: draft.marketAccessMode || "selected",
    },
  };
  if (draft.id) input.id = draft.id;
  assignString(input, "displayName", draft.displayName);
  assignString(input, "ownerEmail", draft.ownerEmail);
  assignString(input, "tunnelSubdomain", draft.subdomain);
  assignString(input, "description", draft.description);
  assignNumber(input, "tokenLimit", draft.tokenLimit);
  assignNumber(input, "parallelLimit", draft.parallelLimit);
  assignNumber(input, "officialPricePercent", draft.officialPricePercent);
  const expiresAt = parseDateTime(draft.expiresAt);
  if (expiresAt != null) input.expiresAt = expiresAt;
  return input;
}

function createBindingDraft(share: ShareRecord, app: AppKind): BindingDraft {
  return {
    share,
    app,
    providerId: bindingMap(share)[app],
  };
}

function bindingMap(share: ShareRecord): Record<AppKind, string> {
  const result: Record<AppKind, string> = { claude: "", codex: "", gemini: "" };
  for (const binding of share.bindings || []) {
    result[binding.app] = binding.providerId;
  }
  if (!result[share.app]) result[share.app] = share.providerId;
  return result;
}

function shareBindings(share: ShareRecord): ShareBinding[] {
  const seen = new Set<string>();
  const bindings: ShareBinding[] = [];
  for (const binding of share.bindings || []) {
    if (binding.providerId && !seen.has(binding.app)) {
      seen.add(binding.app);
      bindings.push(binding);
    }
  }
  if (share.providerId && !seen.has(share.app)) {
    bindings.unshift({ app: share.app, providerId: share.providerId, providerType: share.providerType });
  }
  return apps.flatMap((app) => bindings.filter((binding) => binding.app === app.id));
}

function firstBoundApp(bindings: Record<AppKind, string>): AppKind | null {
  return apps.find((app) => bindings[app.id])?.id || null;
}

function firstProviderApp(providersByApp: Record<AppKind, StoredProvider[]>): AppKind {
  return apps.find((app) => providersByApp[app.id].length)?.id || "claude";
}

function providerKey(app: AppKind, providerId: string): string {
  return `${app}:${providerId}`;
}

function shareName(share: ShareRecord): string {
  return share.displayName || share.id;
}

function appLabel(app: AppKind): string {
  return apps.find((item) => item.id === app)?.label || app;
}

function shareActionLabel(action: "pause" | "resume" | "startTunnel" | "stopTunnel" | "resetUsage"): string {
  const labels: Record<typeof action, string> = {
    pause: "Pause",
    resume: "Resume",
    startTunnel: "Start tunnel",
    stopTunnel: "Stop tunnel",
    resetUsage: "Reset usage",
  };
  return labels[action];
}

function shareUsage(share: ShareRecord): string {
  if (!share.tokenLimit) return `${share.tokensUsed || 0} tokens`;
  return `${share.tokensUsed || 0}/${share.tokenLimit}`;
}

function shareUsageRatio(share: ShareRecord): number {
  if (!share.tokenLimit) return 0;
  return Math.max(0, Math.min(1, (share.tokensUsed || 0) / share.tokenLimit));
}

function splitList(value: string): string[] {
  return value
    .split(/[\s,;]+/)
    .map((item) => item.trim())
    .filter(Boolean);
}

function assignString(target: UpsertShareInput, key: keyof UpsertShareInput, value: string) {
  const trimmed = value.trim();
  if (trimmed) {
    (target as unknown as Record<string, unknown>)[key] = trimmed;
  }
}

function assignNumber(target: UpsertShareInput, key: keyof UpsertShareInput, value: string) {
  if (!value.trim()) return;
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) throw new Error(`${String(key)} must be a number`);
  (target as unknown as Record<string, unknown>)[key] = parsed;
}

function parseDateTime(value: string): number | undefined {
  if (!value.trim()) return undefined;
  const parsed = Date.parse(value);
  if (!Number.isFinite(parsed)) throw new Error("expires at is invalid");
  return parsed;
}

function toDateTimeInput(value?: number | null): string {
  if (!value) return "";
  const millis = value < 10_000_000_000 ? value * 1000 : value;
  const date = new Date(millis);
  if (Number.isNaN(date.getTime())) return "";
  const offset = date.getTimezoneOffset() * 60_000;
  return new Date(date.getTime() - offset).toISOString().slice(0, 16);
}

function formatTime(value?: number | null): string {
  if (!value) return "-";
  const millis = value < 10_000_000_000 ? value * 1000 : value;
  const date = new Date(millis);
  if (Number.isNaN(date.getTime())) return "-";
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
}

function formatTokens(value?: number | null): string {
  if (value == null) return "-";
  return new Intl.NumberFormat(undefined, { maximumFractionDigits: 0 }).format(value);
}

function formatUsd(value?: number | null): string {
  if (value == null) return "-";
  if (Math.abs(value) < 0.01 && value !== 0) return `$${value.toFixed(6)}`;
  return `$${value.toFixed(4)}`;
}

function formatDuration(value?: number | null): string {
  if (value == null) return "-";
  if (value < 1000) return `${Math.round(value)}ms`;
  return `${(value / 1000).toFixed(2)}s`;
}

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

function arrayRecords(value: unknown): Array<Record<string, unknown>> {
  if (!Array.isArray(value)) return [];
  const records: Array<Record<string, unknown>> = [];
  for (const item of value) {
    const record = asRecord(item);
    if (record) records.push(record);
  }
  return records;
}

function stringValue(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function numberValue(value: unknown): number | undefined {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string" && value.trim()) {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) return parsed;
  }
  return undefined;
}

function booleanValue(value: unknown): boolean | undefined {
  if (typeof value === "boolean") return value;
  if (value === "true") return true;
  if (value === "false") return false;
  return undefined;
}

function providerHealthStatus(
  health?: Record<string, unknown>,
): { label: string; tone: "success" | "warning" | "danger" } {
  if (!health) return { label: "unknown", tone: "warning" };
  const healthy = booleanValue(health.healthy);
  const reason = stringValue(health.reason);
  if (healthy === true) return { label: "healthy", tone: "success" };
  if (healthy === false) return { label: reason || "unhealthy", tone: "danger" };
  const successRate = numberValue(health.successRate);
  if (successRate != null) return { label: `${successRate.toFixed(1)}%`, tone: "warning" };
  return { label: reason || "unknown", tone: "warning" };
}

function modelHealthResults(value: unknown): ModelHealthView[] {
  const summary = asRecord(value);
  if (!summary) return [];
  return apps.flatMap((app) =>
    arrayRecords(summary[app.id]).map((record) => ({
      app: app.id,
      requestedModel: stringValue(record.requestedModel) || "-",
      actualModel: stringValue(record.actualModel) || "-",
      status: stringValue(record.status) || "unknown",
      statusCode: numberValue(record.statusCode),
      latencyMs: numberValue(record.latencyMs),
      checkedAt: numberValue(record.checkedAt),
      providerId: stringValue(record.providerId),
      providerName: stringValue(record.providerName),
    })),
  );
}

function modelHealthTone(status: string): "success" | "warning" | "danger" {
  if (status === "success" || status === "healthy") return "success";
  if (status === "quota_blocked" || status === "unknown") return "warning";
  return "danger";
}

function modelPair(record: Record<string, unknown>): string {
  const requested =
    stringValue(record.requestedModel) ||
    stringValue(record.model) ||
    stringValue(record.requestModel) ||
    "-";
  const actual = stringValue(record.actualModel) || requested;
  return requested === actual ? requested : `${requested} -> ${actual}`;
}

function formatPercent(value?: number | null): string {
  return value == null ? "-" : `${value.toFixed(1)}%`;
}

function formatHealthCheckedAt(value?: number): string {
  return value == null ? "-" : formatTime(value);
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
