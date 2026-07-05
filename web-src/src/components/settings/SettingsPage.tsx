import { Languages, RefreshCw } from "lucide-react";
import { FormEvent, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import {
  BackupManifest,
  AppKind,
  FailoverSnapshot,
  batchSyncRouterShares,
  claimClientTunnel,
  createBackup,
  heartbeatRouter,
  loadFailoverSnapshot,
  loadSettingsPageData,
  loadStoredProviders,
  registerRouter,
  restoreBackup,
  rotateApiToken,
  requestEmailLoginCode,
  SettingsPageData,
  startClientTunnel,
  stopClientTunnel,
  StoredProvider,
  updateFailoverApp,
  updateClientTunnel,
  updateRouterConfig,
  updateUpstreamProxy,
  verifyEmailLoginCode,
} from "@/lib/api";
import { Language, useI18n } from "@/lib/i18n";
import { getWebRuntimeContext, WebRuntimeContext, writeToken } from "@/lib/runtime";
import {
  AuthSettingsPanel,
  BackupSettingsPanel,
  DiagnosticsSettingsPanel,
} from "@/components/settings/SettingsAccountPanels";
import { FailoverSettingsPanel } from "@/components/settings/FailoverSettingsPanel";
import {
  ProxySettingsPanel,
  RouterSettingsPanel,
  TunnelSettingsPanel,
} from "@/components/settings/SettingsConnectionPanels";
import {
  AboutPanel,
  DirectoryPanel,
  SettingsOverviewStrip,
  SettingsReadinessPanel,
  ThemeSettingsPanel,
} from "@/components/settings/SettingsInfoPanels";
import { ImportExportPanel } from "@/components/settings/ImportExportPanel";
import { SectionHeader } from "@/components/settings/SettingsSectionHeader";
import {
  appLabel,
  emptyEmailDraft,
  emptyFailoverDrafts,
  emptyProxyDraft,
  emptyRouterDraft,
  emptyTunnelDraft,
  errorMessage,
  failoverDraftsFrom,
  isClientTunnelRunning,
  positiveInteger,
  routerDraftFrom,
  routerStatusText,
  tunnelDraftFrom,
  type EmailDraft,
  type FailoverDraft,
  type ProxyDraft,
  type RouterDraft,
  type TunnelDraft,
} from "@/components/settings/settingsDrafts";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { LoadingBlock } from "@/components/LoadingBlock";

export type SettingsTab =
  | "general"
  | "language"
  | "theme"
  | "directory"
  | "proxy"
  | "failover"
  | "router"
  | "tunnel"
  | "auth"
  | "backup"
  | "importExport"
  | "diagnostics"
  | "about";

const settingsTabs: Array<{ id: SettingsTab; label: string }> = [
  { id: "general", label: "General" },
  { id: "language", label: "Language" },
  { id: "theme", label: "Theme" },
  { id: "directory", label: "Directory" },
  { id: "proxy", label: "Proxy" },
  { id: "failover", label: "Failover" },
  { id: "router", label: "Router" },
  { id: "tunnel", label: "Tunnel" },
  { id: "auth", label: "Auth" },
  { id: "backup", label: "Backup" },
  { id: "importExport", label: "Import / Export" },
  { id: "diagnostics", label: "Diagnostics" },
  { id: "about", label: "About" },
];

export function SettingsPage({ initialTab = "general" }: { initialTab?: SettingsTab }) {
  const { language, languages, setLanguage, t, tx } = useI18n();
  const [data, setData] = useState<SettingsPageData | null>(null);
  const [runtimeContext, setRuntimeContext] = useState<WebRuntimeContext | null>(null);
  const [failoverSnapshot, setFailoverSnapshot] = useState<FailoverSnapshot>({ apps: {}, breakers: [] });
  const [settingsProviders, setSettingsProviders] = useState<StoredProvider[]>([]);
  const [routerDraft, setRouterDraft] = useState<RouterDraft>(emptyRouterDraft());
  const [tunnelDraft, setTunnelDraft] = useState<TunnelDraft>(emptyTunnelDraft());
  const [proxyDraft, setProxyDraft] = useState<ProxyDraft>(emptyProxyDraft());
  const [failoverDrafts, setFailoverDrafts] = useState<Record<AppKind, FailoverDraft>>(emptyFailoverDrafts());
  const [emailDraft, setEmailDraft] = useState<EmailDraft>(emptyEmailDraft());
  const [backupReason, setBackupReason] = useState("");
  const [apiToken, setApiToken] = useState<string | null>(null);
  const [apiTokenCopyStatus, setApiTokenCopyStatus] = useState<{ tone: "success" | "warning"; message: string } | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const [activeTab, setActiveTab] = useState<SettingsTab>(initialTab);
  const [restoreConfirm, setRestoreConfirm] = useState<BackupManifest | null>(null);
  const [rotateTokenConfirm, setRotateTokenConfirm] = useState(false);
  const [routerSyncConfirm, setRouterSyncConfirm] = useState(false);
  const settingsPageRef = useRef<HTMLDivElement>(null);
  const clientTunnelRunning = isClientTunnelRunning(data?.tunnel.runtimeStatus?.status);

  useEffect(() => {
    setActiveTab(initialTab);
  }, [initialTab]);

  useLayoutEffect(() => {
    if (settingsPageRef.current) {
      settingsPageRef.current.scrollTop = 0;
    }
  }, [activeTab]);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [next, context, snapshot, providers] = await Promise.all([
        loadSettingsPageData(),
        getWebRuntimeContext().catch(() => null),
        loadFailoverSnapshot(),
        loadStoredProviders(),
      ]);
      setData(next);
      setRuntimeContext(context);
      setFailoverSnapshot(snapshot);
      setSettingsProviders(providers);
      setFailoverDrafts(failoverDraftsFrom(snapshot));
      setRouterDraft(routerDraftFrom(next.router));
      setTunnelDraft(tunnelDraftFrom(next.tunnel));
      setProxyDraft({
        url: "",
        clear: false,
        followSystemProxy: next.config.upstreamProxy.followSystemProxy,
      });
      setEmailDraft((current) => ({
        ...current,
        email: current.email || next.config.ownerEmail || "",
      }));
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  async function runAction(action: string, task: () => Promise<string>) {
    setBusy(action);
    setError(null);
    try {
      setResult(await task());
      await refresh();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function saveRouter(event: FormEvent) {
    event.preventDefault();
    await runAction("router-save", async () => {
      const router = await updateRouterConfig({
        url: routerDraft.url,
        apiBase: routerDraft.apiBase,
        domain: routerDraft.domain,
        region: routerDraft.region,
        sshHost: routerDraft.sshHost,
        sshUser: routerDraft.sshUser,
        custom: routerDraft.custom,
      });
      return tx("router saved: {{value}}", { value: router.url || "-" });
    });
  }

  async function saveTunnel(event: FormEvent) {
    event.preventDefault();
    await runAction("tunnel-save", async () => {
      const tunnel = await updateClientTunnel({
        tunnelSubdomain: tunnelDraft.tunnelSubdomain,
        tunnelStatus: tunnelDraft.tunnelStatus,
      });
      return tx("client tunnel saved: {{value}}", { value: tunnel.tunnelSubdomain || "-" });
    });
  }

  async function saveProxy(event: FormEvent) {
    event.preventDefault();
    await runAction("proxy-save", async () => {
      const proxy = await updateUpstreamProxy({
        url: proxyDraft.url.trim() || undefined,
        clear: proxyDraft.clear,
        followSystemProxy: proxyDraft.followSystemProxy,
      });
      return proxy.enabled
        ? tx("proxy saved: {{value}}", { value: proxy.maskedUrl || tx("configured") })
        : tx("proxy disabled");
    });
  }

  async function saveFailover(app: AppKind, event: FormEvent) {
    event.preventDefault();
    const draft = failoverDrafts[app];
    await runAction(`failover-save:${app}`, async () => {
      const config = await updateFailoverApp(app, {
        enabled: draft.enabled,
        providerQueue: draft.providerQueue,
        failureThreshold: positiveInteger(draft.failureThreshold, 2),
        openDurationMs: positiveInteger(draft.openDurationSeconds, 300) * 1000,
        halfOpenMaxProbes: positiveInteger(draft.halfOpenMaxProbes, 1),
      });
      return config.enabled
        ? tx("{{app}} failover enabled", { app: appLabel(app) })
        : tx("{{app}} failover disabled", { app: appLabel(app) });
    });
  }

  async function makeBackup(event: FormEvent) {
    event.preventDefault();
    await runAction("backup-create", async () => {
      const backup = await createBackup(backupReason);
      setBackupReason("");
      return tx("backup created: {{id}}", { id: backup.id });
    });
  }

  async function restoreBackupAction(backup: BackupManifest) {
    await runAction(`backup-restore:${backup.id}`, async () => {
      const restored = await restoreBackup(backup.id);
      return tx("restored {{id}}; safety {{safety}}", {
        id: restored.restored.id,
        safety: restored.preRestore?.id || "-",
      });
    });
  }

  async function rotateTokenAction() {
    await runAction("api-token", async () => {
      const token = await rotateApiToken();
      setApiToken(token);
      setApiTokenCopyStatus(null);
      return tx("new API token generated");
    });
  }

  async function copyApiToken() {
    if (!apiToken) return;
    if (!navigator.clipboard?.writeText) {
      setApiTokenCopyStatus({ tone: "warning", message: tx("Clipboard unavailable; copy the visible value manually.") });
      return;
    }
    try {
      await navigator.clipboard.writeText(apiToken);
      setApiTokenCopyStatus({ tone: "success", message: tx("API token copied") });
    } catch {
      setApiTokenCopyStatus({ tone: "warning", message: tx("Copy failed; copy the visible value manually.") });
    }
  }

  async function requestCodeAction() {
    await runAction("email-request", async () => {
      const response = await requestEmailLoginCode(emailDraft.email);
      return tx("code sent to {{destination}}; cooldown {{seconds}}s", {
        destination: response.maskedDestination,
        seconds: response.cooldownSecs,
      });
    });
  }

  async function verifyCodeAction() {
    await runAction("email-verify", async () => {
      const login = await verifyEmailLoginCode(emailDraft);
      writeToken(login.token);
      return tx("email verified and local session updated");
    });
  }

  return (
    <div className="settings-page" ref={settingsPageRef}>
      <div className="provider-toolbar">
        <div className="provider-toolbar-status">
          <span>{data?.config.ownerEmail || t("server.settings.runtimeSubtitle")}</span>
        </div>
        <div className="provider-toolbar-actions">
          {error && <span className="error-text">{error}</span>}
          {result && <span className="usage-result">{result}</span>}
          <button className="secondary-button" type="button" onClick={() => void refresh()}>
            <RefreshCw size={15} />
            <span>{t("common.refresh")}</span>
          </button>
        </div>
      </div>

      {loading && !data ? (
        <LoadingBlock label="server.settings.loading" />
      ) : (
        <div className="settings-tab-shell">
          <div className="settings-tabs" role="tablist" aria-label={tx("Settings sections")}>
            {settingsTabs.map((tab) => (
              <button
                key={tab.id}
                type="button"
                role="tab"
                aria-selected={activeTab === tab.id}
                className={activeTab === tab.id ? "active" : ""}
                onClick={() => setActiveTab(tab.id)}
              >
                {tx(tab.label)}
              </button>
            ))}
          </div>

          <div className="settings-tab-panel">
            {activeTab === "general" && (
              <div className="settings-layout">
                {data && <SettingsOverviewStrip data={data} />}

                {data && <SettingsReadinessPanel data={data} />}
              </div>
            )}

            {activeTab === "language" && (
              <div className="settings-layout">
                <section className="settings-card">
                  <SectionHeader icon={<Languages size={17} />} title={t("server.settings.language")} subtitle={t("server.settings.languageSubtitle")} />
                  <label>
                    <span>{t("server.settings.displayLanguage")}</span>
                    <select value={language} onChange={(event) => setLanguage(event.target.value as Language)}>
                      {languages.map((option) => (
                        <option key={option.value} value={option.value}>
                          {option.label}
                        </option>
                      ))}
                    </select>
                  </label>
                </section>
              </div>
            )}

            {activeTab === "theme" && (
              <div className="settings-layout">
                <ThemeSettingsPanel />
              </div>
            )}

            {activeTab === "directory" && (
              <div className="settings-layout">
                <DirectoryPanel runtimeContext={runtimeContext} />
              </div>
            )}

            {activeTab === "proxy" && (
              <div className="settings-layout">
                <ProxySettingsPanel
                  maskedUrl={data?.config.upstreamProxy.maskedUrl}
                  draft={proxyDraft}
                  busy={busy}
                  onDraftChange={setProxyDraft}
                  onSave={saveProxy}
                />
              </div>
            )}

            {activeTab === "failover" && (
              <div className="settings-layout">
                <FailoverSettingsPanel
                  snapshot={failoverSnapshot}
                  providers={settingsProviders}
                  drafts={failoverDrafts}
                  busy={busy}
                  onDraftChange={(app, draft) => setFailoverDrafts((current) => ({ ...current, [app]: draft }))}
                  onSave={saveFailover}
                />
              </div>
            )}

            {activeTab === "router" && (
              <div className="settings-layout">
                <RouterSettingsPanel
                  router={data?.router}
                  status={data?.routerStatus}
                  draft={routerDraft}
                  busy={busy}
                  onDraftChange={setRouterDraft}
                  onSave={saveRouter}
                  onRegister={() => void runAction("router-register", async () => `registered ${JSON.stringify(await registerRouter())}`)}
                  onHeartbeat={() => void runAction("router-heartbeat", async () => routerStatusText(await heartbeatRouter()))}
                  onBatchSync={() => setRouterSyncConfirm(true)}
                />
              </div>
            )}

            {activeTab === "tunnel" && (
              <div className="settings-layout">
                <TunnelSettingsPanel
                  tunnel={data?.tunnel}
                  draft={tunnelDraft}
                  busy={busy}
                  clientTunnelRunning={clientTunnelRunning}
                  onDraftChange={setTunnelDraft}
                  onSave={saveTunnel}
                  onClaim={() => void runAction("tunnel-claim", async () => `claim ${JSON.stringify(await claimClientTunnel())}`)}
                  onStart={() => void runAction("tunnel-start", async () => (await startClientTunnel()).message)}
                  onStop={() => void runAction("tunnel-stop", async () => `stopped ${(await stopClientTunnel()).tunnelStatus || "client tunnel"}`)}
                />
              </div>
            )}

            {activeTab === "auth" && (
              <div className="settings-layout">
                <AuthSettingsPanel
                  emailDraft={emailDraft}
                  apiToken={apiToken}
                  apiTokenCopyStatus={apiTokenCopyStatus}
                  busy={busy}
                  onEmailDraftChange={setEmailDraft}
                  onRotateToken={() => setRotateTokenConfirm(true)}
                  onCopyToken={() => void copyApiToken()}
                  onRequestCode={() => void requestCodeAction()}
                  onVerifyCode={() => void verifyCodeAction()}
                />
              </div>
            )}

            {activeTab === "backup" && (
              <div className="settings-layout">
                <BackupSettingsPanel
                  backups={data?.backups || []}
                  backupReason={backupReason}
                  busy={busy}
                  onBackupReasonChange={setBackupReason}
                  onCreateBackup={makeBackup}
                  onRestore={setRestoreConfirm}
                />
              </div>
            )}

            {activeTab === "importExport" && (
              <div className="settings-layout">
                <ImportExportPanel busy={busy} runAction={runAction} />
              </div>
            )}

            {activeTab === "diagnostics" && (
              <div className="settings-layout">
                <DiagnosticsSettingsPanel diagnostics={data?.diagnostics} />
              </div>
            )}

            {activeTab === "about" && (
              <div className="settings-layout">
                {data && <AboutPanel buildInfo={data.buildInfo} />}
              </div>
            )}
          </div>
        </div>
      )}
      <ConfirmDialog
        isOpen={routerSyncConfirm}
        title={tx("Batch sync router shares")}
        message={tx("Batch sync share state to the router? Remote router records for matching shares may be updated.")}
        confirmText={tx("Sync")}
        onConfirm={() => {
          setRouterSyncConfirm(false);
          void runAction("router-sync", async () => (await batchSyncRouterShares()).message);
        }}
        onCancel={() => setRouterSyncConfirm(false)}
      />
      <ConfirmDialog
        isOpen={rotateTokenConfirm}
        title={tx("Rotate API token")}
        message={tx("Rotate the server API token? Existing clients using the current token will stop working until updated.")}
        confirmText={tx("Rotate")}
        variant="destructive"
        onConfirm={() => {
          setRotateTokenConfirm(false);
          void rotateTokenAction();
        }}
        onCancel={() => setRotateTokenConfirm(false)}
      />
      <ConfirmDialog
        isOpen={restoreConfirm !== null}
        title={tx("Restore backup")}
        message={tx("Restore backup {{id}}? Current stores will be backed up first.", { id: restoreConfirm?.id || "-" })}
        confirmText={tx("Restore")}
        variant="info"
        onConfirm={() => {
          const backup = restoreConfirm;
          setRestoreConfirm(null);
          if (backup) void restoreBackupAction(backup);
        }}
        onCancel={() => setRestoreConfirm(null)}
      />
    </div>
  );
}
