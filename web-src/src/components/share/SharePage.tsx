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
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { IconAction } from "@/components/IconAction";
import { LoadingBlock } from "@/components/LoadingBlock";
import { SimpleModal } from "@/components/SimpleModal";
import { ImportSharesModal } from "@/components/share/ImportSharesModal";
import {
  createBindingDraft,
  createShareDraft,
  editShareDraft,
  ShareFormModal,
  shareInputFromDraft,
  shareOwnerChanged,
  type ShareDraft,
} from "@/components/share/ShareFormModal";
import {
  AclModal,
  BindingModal,
  MarketModal,
  SubdomainModal,
  type BindingDraft,
} from "@/components/share/ShareEditModals";
import { OwnerChangeModal } from "@/components/share/OwnerChangeModal";
import { filterShares } from "@/components/share/ShareListFiltering";
import { ShareCard } from "@/components/share/ShareCard";
import { ShareEmptyState } from "@/components/share/ShareEmptyState";
import { ShareRequestLogPanel } from "@/components/share/ShareRequestLogPanel";
import { ShareRuntimePanel } from "@/components/share/ShareRuntimePanel";
import { ShareStatsBar } from "@/components/share/ShareStatsBar";
import { ShareTunnelConfigPanel } from "@/components/share/ShareTunnelConfigPanel";
import { ShareToolbar, type ShareFilter, type ShareSort } from "@/components/share/ShareToolbar";
import { shareName } from "@/components/share/shareDisplay";
import {
  AppKind,
  authorizeShareMarket,
  deleteShare,
  exportShares,
  importShares,
  loadShareConnectInfo,
  loadSharePageData,
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
  ShareConnectInfo,
  ShareRecord,
  startShareTunnel,
  stopShareTunnel,
  StoredProvider,
  updateShareBinding,
  updateShareSubdomain,
  UsageLog,
  verifyShareOwnerChangeCode,
} from "@/lib/api";
import { useI18n } from "@/lib/i18n";

interface SharePageState {
  shares: ShareRecord[];
  providers: StoredProvider[];
  requestLogs: UsageLog[];
}

export function SharePage() {
  const { t, tx } = useI18n();
  const [data, setData] = useState<SharePageState>({ shares: [], providers: [], requestLogs: [] });
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
  const [exportCopyStatus, setExportCopyStatus] = useState<{ tone: "success" | "warning"; message: string } | null>(null);
  const [ownerChangeDraft, setOwnerChangeDraft] = useState<ShareDraft | null>(null);
  const [shareQuery, setShareQuery] = useState("");
  const [shareFilter, setShareFilter] = useState<ShareFilter>("all");
  const [shareSort, setShareSort] = useState<ShareSort>("createdAtDesc");
  const [toolbarConfirm, setToolbarConfirm] = useState<"restore" | "edits" | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setData(await loadSharePageData());
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
  const filteredShares = useMemo(
    () => filterShares(data.shares, shareQuery, shareFilter, shareSort),
    [data.shares, shareFilter, shareQuery, shareSort],
  );

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
      setExportCopyStatus(null);
      let copied = false;
      try {
        if (navigator.clipboard) {
          await navigator.clipboard.writeText(text);
          copied = true;
        }
      } catch {
        copied = false;
      }
      setExportCopyStatus({
        tone: copied ? "success" : "warning",
        message: copied ? tx("Copied JSON") : tx("Clipboard unavailable; copy the visible value manually."),
      });
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

  async function copyExportText() {
    if (!exportText) return;
    if (!navigator.clipboard?.writeText) {
      setExportCopyStatus({ tone: "warning", message: tx("Clipboard unavailable; copy the visible value manually.") });
      return;
    }
    try {
      await navigator.clipboard.writeText(exportText);
      setExportCopyStatus({ tone: "success", message: tx("Copied JSON") });
    } catch {
      setExportCopyStatus({ tone: "warning", message: tx("Copy failed; copy the visible value manually.") });
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
    <div className="share-page">
      <div className="provider-toolbar">
        <div className="provider-toolbar-status">
          <span>{t("server.shares.routes", { count: data.shares.length })}</span>
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
          <button className="secondary-button" type="button" onClick={() => setToolbarConfirm("restore")} disabled={busyId === "restore"}>
            {busyId === "restore" ? <Loader2 size={15} /> : <Cable size={15} />}
            <span>{t("server.shares.restoreTunnels")}</span>
          </button>
          <button className="secondary-button" type="button" onClick={() => setToolbarConfirm("edits")} disabled={busyId === "edits"}>
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

      <ShareStatsBar shares={data.shares} />

      <ShareToolbar
        query={shareQuery}
        filter={shareFilter}
        sort={shareSort}
        total={data.shares.length}
        visible={filteredShares.length}
        onQueryChange={setShareQuery}
        onFilterChange={setShareFilter}
        onSortChange={setShareSort}
      />

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
        <LoadingBlock label="server.shares.loading" />
      ) : data.shares.length ? (
        filteredShares.length ? (
          <div className="share-card-grid">
            {filteredShares.map((share) => (
              <ShareCard
                key={share.id}
                share={share}
                providerByKey={providerByKey}
                markets={markets}
                marketsLoaded={marketsLoaded}
                result={resultById[share.id]}
                connectInfo={connectInfoById[share.id]}
                runtimePanel={<ShareRuntimePanel share={share} />}
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
          <div className="provider-empty compact-empty">
            <SlidersHorizontal size={20} />
            <span>{tx("No shares match the current filter")}</span>
          </div>
        )
      ) : (
        <ShareEmptyState
          canCreate={data.providers.length > 0}
          onCreate={() => setDraft(createShareDraft(providersByApp))}
          onImport={() => setImportOpen(true)}
        />
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

      <ConfirmDialog
        isOpen={toolbarConfirm !== null}
        title={tx(toolbarConfirm === "restore" ? "Restore share tunnels" : "Pull share edits")}
        message={
          toolbarConfirm === "restore"
            ? tx("Restore share tunnel runtime from saved share configuration? Active tunnel state may be replaced.")
            : tx("Pull and apply pending router share edits? Matching shares may be updated.")
        }
        confirmText={tx(toolbarConfirm === "restore" ? "Restore" : "Pull edits")}
        onConfirm={() => {
          const action = toolbarConfirm;
          setToolbarConfirm(null);
          if (action) void toolbarAction(action);
        }}
        onCancel={() => setToolbarConfirm(null)}
      />

      {exportText && (
        <SimpleModal
          title="Export Shares"
          subtitle="Copy this JSON when clipboard access is unavailable."
          onClose={() => setExportText(null)}
        >
          <textarea readOnly value={exportText} />
          {exportCopyStatus && <div className={`connect-copy-status ${exportCopyStatus.tone}`}>{exportCopyStatus.message}</div>}
          <footer className="modal-inline-footer">
            <button className="secondary-button" type="button" onClick={() => void copyExportText()}>
              <Copy size={15} />
              <span>{tx("Copy JSON")}</span>
            </button>
            <button className="secondary-button" type="button" onClick={() => setExportText(null)}>
              {tx("Close")}
            </button>
          </footer>
        </SimpleModal>
      )}
    </div>
  );
}

function providerKey(app: AppKind, providerId: string): string {
  return `${app}:${providerId}`;
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

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
