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
  X,
} from "lucide-react";
import { FormEvent, ReactNode, useCallback, useEffect, useMemo, useState } from "react";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { IconAction } from "@/components/IconAction";
import { LoadingBlock } from "@/components/LoadingBlock";
import { ModalFooter } from "@/components/ModalFooter";
import { SimpleModal } from "@/components/SimpleModal";
import { ImportSharesModal } from "@/components/share/ImportSharesModal";
import {
  AclModal,
  BindingModal,
  MarketModal,
  SubdomainModal,
  type BindingDraft,
} from "@/components/share/ShareEditModals";
import { OwnerChangeModal } from "@/components/share/OwnerChangeModal";
import { ShareCard } from "@/components/share/ShareCard";
import { ShareEmptyState } from "@/components/share/ShareEmptyState";
import { ShareRequestLogPanel } from "@/components/share/ShareRequestLogPanel";
import { ShareRuntimePanel } from "@/components/share/ShareRuntimePanel";
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

interface SharePageState {
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

const apps: Array<{ id: AppKind; label: string }> = [
  { id: "claude", label: "Claude" },
  { id: "codex", label: "Codex" },
  { id: "gemini", label: "Gemini" },
];

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

      <div className="share-stats-bar">
        <ShareStat label={t("server.shares.active")} value={data.shares.filter((share) => share.status === "active").length} />
        <ShareStat label={t("server.shares.paused")} value={data.shares.filter((share) => share.status === "paused").length} />
        <ShareStat label={t("server.shares.forSale")} value={data.shares.filter((share) => share.forSale).length} />
        <ShareStat label={t("server.shares.requests")} value={data.shares.reduce((sum, share) => sum + (share.requestsCount || 0), 0)} />
      </div>

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
              <option value="selected">{tx("selected")}</option>
              <option value="all">{tx("all")}</option>
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

function filterShares(
  shares: ShareRecord[],
  query: string,
  filter: ShareFilter,
  sort: ShareSort,
): ShareRecord[] {
  const normalizedQuery = query.trim().toLowerCase();
  return shares.filter((share) => {
    if (filter === "active" && share.status !== "active") return false;
    if (filter === "paused" && share.status !== "paused") return false;
    if (filter === "expired" && share.status !== "expired") return false;
    if (filter === "exhausted" && share.status !== "exhausted") return false;
    if (filter === "sale" && !share.forSale) return false;
    if (!normalizedQuery) return true;
    return [
      share.id,
      share.displayName,
      share.ownerEmail,
      share.status,
      share.tunnelSubdomain,
      share.description,
      share.saleMarketKind,
      share.app,
      share.providerId,
      share.providerType,
      ...(share.bindings || []).map((binding) => `${binding.app} ${binding.providerId} ${binding.providerType}`),
    ]
      .filter(Boolean)
      .join(" ")
      .toLowerCase()
      .includes(normalizedQuery);
  }).sort((left, right) => compareShares(left, right, sort));
}

function compareShares(left: ShareRecord, right: ShareRecord, sort: ShareSort): number {
  if (sort === "expiresAtAsc") {
    return compareNullableNumber(left.expiresAt, right.expiresAt, "asc");
  }
  if (sort === "tokensUsedDesc") {
    return compareNumber(right.tokensUsed, left.tokensUsed) || compareShareName(left, right);
  }
  if (sort === "nameAsc") {
    return compareShareName(left, right);
  }
  return compareNullableNumber(shareCreatedAtMs(left), shareCreatedAtMs(right), "desc");
}

function shareCreatedAtMs(share: ShareRecord): number | null {
  return normalizeTimeMs(
    share.createdAtMs ??
      share.createdAt ??
      share.created_at_ms ??
      share.created_at,
  );
}

function normalizeTimeMs(value: number | string | null | undefined): number | null {
  if (typeof value === "number") {
    if (!Number.isFinite(value)) return null;
    return value > 0 && value < 10_000_000_000 ? value * 1000 : value;
  }
  if (typeof value !== "string" || !value.trim()) return null;
  const numeric = Number(value);
  if (Number.isFinite(numeric)) {
    return numeric > 0 && numeric < 10_000_000_000 ? numeric * 1000 : numeric;
  }
  const parsed = new Date(value).getTime();
  return Number.isFinite(parsed) ? parsed : null;
}

function compareNullableNumber(
  left: number | null | undefined,
  right: number | null | undefined,
  direction: "asc" | "desc",
): number {
  const leftValid = typeof left === "number" && Number.isFinite(left);
  const rightValid = typeof right === "number" && Number.isFinite(right);
  if (!leftValid && !rightValid) return 0;
  if (!leftValid) return 1;
  if (!rightValid) return -1;
  return direction === "asc" ? compareNumber(left, right) : compareNumber(right, left);
}

function compareNumber(left: number | null | undefined, right: number | null | undefined): number {
  return (left || 0) - (right || 0);
}

function compareShareName(left: ShareRecord, right: ShareRecord): number {
  const leftName = (left.displayName || left.id || "").toLowerCase();
  const rightName = (right.displayName || right.id || "").toLowerCase();
  return leftName.localeCompare(rightName);
}

function ShareStat({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="share-stat">
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
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

function firstBoundApp(bindings: Record<AppKind, string>): AppKind | null {
  return apps.find((app) => bindings[app.id])?.id || null;
}

function firstProviderApp(providersByApp: Record<AppKind, StoredProvider[]>): AppKind {
  return apps.find((app) => providersByApp[app.id].length)?.id || "claude";
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

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
