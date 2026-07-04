import {
  AlertTriangle,
  Archive,
  Cable,
  CheckCircle2,
  Cloud,
  Copy,
  Download,
  FileJson,
  FolderOpen,
  GitCommit,
  Info,
  KeyRound,
  Languages,
  Loader2,
  Mail,
  Monitor,
  Network,
  Moon,
  Palette,
  RefreshCw,
  RotateCcw,
  Save,
  ShieldCheck,
  Shuffle,
  Sun,
  Upload,
} from "lucide-react";
import { FormEvent, ReactNode, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import {
  BackupManifest,
  BuildInfo,
  AppKind,
  FailoverSnapshot,
  batchSyncRouterShares,
  claimClientTunnel,
  createBackup,
  exportProviders,
  exportShares,
  exportUniversalProviders,
  heartbeatRouter,
  importProviders,
  importShares,
  importUniversalProviders,
  loadFailoverSnapshot,
  loadSettingsDashboardData,
  loadStoredProviders,
  registerRouter,
  restoreBackup,
  rotateApiToken,
  requestEmailLoginCode,
  RouterConfigView,
  RouterDiagnosticsResponse,
  RouterStatusResponse,
  SettingsDashboardData,
  ShareRecord,
  startClientTunnel,
  stopClientTunnel,
  StoredProvider,
  TunnelRuntimeStatus,
  updateFailoverApp,
  updateClientTunnel,
  updateRouterConfig,
  updateUpstreamProxy,
  verifyEmailLoginCode,
  UniversalProvider,
} from "@/lib/api";
import { Language, useI18n } from "@/lib/i18n";
import { getWebRuntimeContext, WebRuntimeContext, writeToken } from "@/lib/runtime";
import { AccountsDashboard } from "@/components/AccountsDashboard";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { useTheme } from "@/components/theme-provider";

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

const APP_KINDS: AppKind[] = ["claude", "codex", "gemini"];

interface RouterDraft {
  url: string;
  apiBase: string;
  domain: string;
  region: string;
  sshHost: string;
  sshUser: string;
  custom: boolean;
}

interface TunnelDraft {
  tunnelSubdomain: string;
  tunnelStatus: string;
}

interface ProxyDraft {
  url: string;
  clear: boolean;
  followSystemProxy: boolean;
}

interface EmailDraft {
  email: string;
  code: string;
}

interface FailoverDraft {
  enabled: boolean;
  providerQueue: string[];
  failureThreshold: string;
  openDurationSeconds: string;
  halfOpenMaxProbes: string;
}

export function SettingsDashboard({ initialTab = "general" }: { initialTab?: SettingsTab }) {
  const { language, languages, setLanguage, t, tx } = useI18n();
  const [data, setData] = useState<SettingsDashboardData | null>(null);
  const [runtimeContext, setRuntimeContext] = useState<WebRuntimeContext | null>(null);
  const [failoverSnapshot, setFailoverSnapshot] = useState<FailoverSnapshot>({ apps: {}, breakers: [] });
  const [settingsProviders, setSettingsProviders] = useState<StoredProvider[]>([]);
  const [routerDraft, setRouterDraft] = useState<RouterDraft>(emptyRouterDraft());
  const [tunnelDraft, setTunnelDraft] = useState<TunnelDraft>(emptyTunnelDraft());
  const [proxyDraft, setProxyDraft] = useState<ProxyDraft>(emptyProxyDraft());
  const [failoverDrafts, setFailoverDrafts] = useState<Record<AppKind, FailoverDraft>>(emptyFailoverDrafts());
  const [emailDraft, setEmailDraft] = useState<EmailDraft>({ email: "", code: "" });
  const [backupReason, setBackupReason] = useState("");
  const [apiToken, setApiToken] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const [activeTab, setActiveTab] = useState<SettingsTab>(initialTab);
  const [restoreConfirm, setRestoreConfirm] = useState<BackupManifest | null>(null);
  const [rotateTokenConfirm, setRotateTokenConfirm] = useState(false);
  const [routerSyncConfirm, setRouterSyncConfirm] = useState(false);
  const dashboardRef = useRef<HTMLDivElement>(null);
  const clientTunnelRunning = isClientTunnelRunning(data?.tunnel.runtimeStatus?.status);

  useEffect(() => {
    setActiveTab(initialTab);
  }, [initialTab]);

  useLayoutEffect(() => {
    if (dashboardRef.current) {
      dashboardRef.current.scrollTop = 0;
    }
  }, [activeTab]);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [next, context, snapshot, providers] = await Promise.all([
        loadSettingsDashboardData(),
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
      return tx("new API token generated");
    });
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
    <div className="settings-dashboard" ref={dashboardRef}>
      <div className="provider-toolbar">
        <div className="section-title-row">
          <ShieldCheck size={18} />
          <div>
            <h2>{t("server.settings.title")}</h2>
            <span>{data?.config.ownerEmail || t("server.settings.runtimeSubtitle")}</span>
          </div>
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
        <div className="provider-empty">
          <Loader2 size={22} />
          <span>{t("server.settings.loading")}</span>
        </div>
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
          <section className="settings-card">
            <SectionHeader icon={<Cloud size={17} />} title={t("server.settings.upstreamProxy")} subtitle={data?.config.upstreamProxy.maskedUrl || t("server.settings.notConfigured")} />
            <form className="settings-form" onSubmit={saveProxy}>
              <label className="toggle-row">
                <input
                  type="checkbox"
                  checked={proxyDraft.followSystemProxy}
                  onChange={(event) => setProxyDraft({ ...proxyDraft, followSystemProxy: event.target.checked })}
                />
                <span>{t("server.settings.followSystemProxy")}</span>
              </label>
              <label>
                <span>{t("server.settings.newProxyUrl")}</span>
                <input
                  value={proxyDraft.url}
                  placeholder="http://127.0.0.1:7890"
                  onChange={(event) => setProxyDraft({ ...proxyDraft, url: event.target.value })}
                />
              </label>
              <label className="toggle-row">
                <input
                  type="checkbox"
                  checked={proxyDraft.clear}
                  onChange={(event) => setProxyDraft({ ...proxyDraft, clear: event.target.checked })}
                />
                <span>{t("server.settings.clearConfiguredUrl")}</span>
              </label>
              <FormFooter busy={busy === "proxy-save"} label={t("server.settings.saveProxy")} />
            </form>
          </section>
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
          <section className="settings-card wide">
            <SectionHeader icon={<Network size={17} />} title={t("server.settings.router")} subtitle={routerState(data?.routerStatus)} />
            <form className="settings-form settings-form-grid" onSubmit={saveRouter}>
              <TextField label={t("server.auth.routerUrl")} value={routerDraft.url} onChange={(value) => setRouterDraft({ ...routerDraft, url: value })} />
              <TextField label={t("server.settings.apiBase")} value={routerDraft.apiBase} onChange={(value) => setRouterDraft({ ...routerDraft, apiBase: value })} />
              <TextField label={t("server.settings.domain")} value={routerDraft.domain} onChange={(value) => setRouterDraft({ ...routerDraft, domain: value })} />
              <TextField label={t("server.settings.region")} value={routerDraft.region} onChange={(value) => setRouterDraft({ ...routerDraft, region: value })} />
              <TextField label={t("server.settings.sshHost")} value={routerDraft.sshHost} onChange={(value) => setRouterDraft({ ...routerDraft, sshHost: value })} />
              <TextField label={t("server.settings.sshUser")} value={routerDraft.sshUser} onChange={(value) => setRouterDraft({ ...routerDraft, sshUser: value })} />
              <label className="toggle-row">
                <input
                  type="checkbox"
                  checked={routerDraft.custom}
                  onChange={(event) => setRouterDraft({ ...routerDraft, custom: event.target.checked })}
                />
                <span>{t("server.settings.customRouter")}</span>
              </label>
              <FormFooter busy={busy === "router-save"} label={t("server.settings.saveRouter")} />
            </form>
            <div className="settings-actions">
              <ActionButton label={t("server.settings.register")} icon={<CheckCircle2 size={15} />} busy={busy === "router-register"} onClick={() => void runAction("router-register", async () => `registered ${JSON.stringify(await registerRouter())}`)} />
              <ActionButton label={t("server.settings.heartbeat")} icon={<RefreshCw size={15} />} busy={busy === "router-heartbeat"} onClick={() => void runAction("router-heartbeat", async () => routerStatusText(await heartbeatRouter()))} />
              <ActionButton label={t("server.settings.batchSync")} icon={<RotateCcw size={15} />} busy={busy === "router-sync"} onClick={() => setRouterSyncConfirm(true)} />
            </div>
            <RouterFacts router={data?.router} status={data?.routerStatus} />
          </section>
              </div>
            )}

            {activeTab === "tunnel" && (
              <div className="settings-layout">
          <section className="settings-card">
            <SectionHeader icon={<Cable size={17} />} title={t("server.settings.clientTunnel")} subtitle={data?.tunnel.runtimeStatus?.tunnelUrl || data?.tunnel.tunnelSubdomain || "-"} />
            <form className="settings-form" onSubmit={saveTunnel}>
              <TextField label={t("server.settings.subdomain")} value={tunnelDraft.tunnelSubdomain} onChange={(value) => setTunnelDraft({ ...tunnelDraft, tunnelSubdomain: value })} />
              <TextField label={t("server.settings.status")} value={tunnelDraft.tunnelStatus} onChange={(value) => setTunnelDraft({ ...tunnelDraft, tunnelStatus: value })} />
              <FormFooter busy={busy === "tunnel-save"} label={t("server.settings.saveTunnel")} />
            </form>
            <div className="settings-actions">
              <ActionButton label={t("server.settings.claim")} icon={<CheckCircle2 size={15} />} busy={busy === "tunnel-claim"} onClick={() => void runAction("tunnel-claim", async () => `claim ${JSON.stringify(await claimClientTunnel())}`)} />
              <ActionButton
                label={t("server.settings.start")}
                icon={<Cable size={15} />}
                busy={busy === "tunnel-start"}
                disabled={clientTunnelRunning}
                onClick={() => void runAction("tunnel-start", async () => (await startClientTunnel()).message)}
              />
              <ActionButton
                label={t("server.settings.stop")}
                icon={<AlertTriangle size={15} />}
                busy={busy === "tunnel-stop"}
                disabled={!clientTunnelRunning}
                onClick={() => void runAction("tunnel-stop", async () => `stopped ${(await stopClientTunnel()).tunnelStatus || "client tunnel"}`)}
              />
            </div>
            <TunnelStatus status={data?.tunnel.runtimeStatus} />
          </section>
              </div>
            )}

            {activeTab === "auth" && (
              <div className="settings-layout">
          <section className="settings-card">
            <SectionHeader icon={<KeyRound size={17} />} title={t("server.settings.auth")} subtitle={t("server.settings.authSubtitle")} />
            <div className="settings-actions">
              <ActionButton label={t("server.settings.rotateApiToken")} icon={<KeyRound size={15} />} busy={busy === "api-token"} onClick={() => setRotateTokenConfirm(true)} />
              {apiToken && (
                <button className="secondary-button" type="button" onClick={() => void navigator.clipboard?.writeText(apiToken)}>
                  <Copy size={15} />
                  <span>{t("server.settings.copyToken")}</span>
                </button>
              )}
            </div>
            {apiToken && <pre className="settings-secret-preview">{apiToken}</pre>}
            <div className="settings-form">
              <TextField label={t("server.auth.ownerEmail")} value={emailDraft.email} onChange={(value) => setEmailDraft({ ...emailDraft, email: value })} />
              <TextField label={t("server.settings.verificationCode")} value={emailDraft.code} onChange={(value) => setEmailDraft({ ...emailDraft, code: value })} />
              <div className="settings-actions">
                <ActionButton label={t("server.settings.requestCode")} icon={<Mail size={15} />} busy={busy === "email-request"} onClick={() => void requestCodeAction()} />
                <ActionButton label={t("server.settings.verify")} icon={<CheckCircle2 size={15} />} busy={busy === "email-verify"} onClick={() => void verifyCodeAction()} />
              </div>
            </div>
          </section>
          <section className="settings-card wide settings-accounts-card">
            <SectionHeader
              icon={<KeyRound size={17} />}
              title={t("server.nav.accounts")}
              subtitle={tx("OAuth accounts and quota tools")}
            />
            <AccountsDashboard embedded />
          </section>
              </div>
            )}

            {activeTab === "backup" && (
              <div className="settings-layout">
          <section className="settings-card wide">
            <SectionHeader icon={<Archive size={17} />} title={t("server.settings.backup")} subtitle={t("server.settings.backupSubtitle")} />
            <BackupPolicySummary backups={data?.backups || []} />
            <form className="settings-form backup-create-row" onSubmit={makeBackup}>
              <label>
                <span>{t("server.settings.reason")}</span>
                <input value={backupReason} onChange={(event) => setBackupReason(event.target.value)} />
              </label>
              <FormFooter busy={busy === "backup-create"} label={t("server.settings.createBackup")} />
            </form>
            <BackupTable backups={data?.backups || []} busy={busy} onRestore={setRestoreConfirm} />
          </section>
              </div>
            )}

            {activeTab === "importExport" && (
              <div className="settings-layout">
                <ImportExportPanel busy={busy} runAction={runAction} />
              </div>
            )}

            {activeTab === "diagnostics" && (
              <div className="settings-layout">
          <section className="settings-card wide">
            <SectionHeader
              icon={<Network size={17} />}
              title={t("server.settings.diagnostics")}
              subtitle={t("server.settings.diagnosticsSubtitle", {
                tunnels: data?.diagnostics.tunnels.length || 0,
                shares: data?.diagnostics.shareSync.length || 0,
              })}
            />
            <DiagnosticsSummary diagnostics={data?.diagnostics} />
            <Diagnostics diagnostics={data?.diagnostics} />
          </section>
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

function SectionHeader({ icon, title, subtitle }: { icon: ReactNode; title: string; subtitle: string }) {
  return (
    <header className="settings-card-header">
      <div className="section-title-row compact-title">
        {icon}
        <h3>{title}</h3>
      </div>
      <span>{subtitle}</span>
    </header>
  );
}

function ThemeSettingsPanel() {
  const { theme, setTheme } = useTheme();
  const { tx } = useI18n();
  const options: Array<{ value: "light" | "dark" | "system"; label: string; icon: ReactNode }> = [
    { value: "light", label: "Light", icon: <Sun size={16} /> },
    { value: "dark", label: "Dark", icon: <Moon size={16} /> },
    { value: "system", label: "System", icon: <Monitor size={16} /> },
  ];
  return (
    <section className="settings-card">
      <SectionHeader
        icon={<Palette size={17} />}
        title={tx("Theme")}
        subtitle={tx("Choose the desktop color mode for this browser")}
      />
      <div className="theme-option-grid" role="radiogroup" aria-label={tx("Theme")}>
        {options.map((option) => (
          <button
            key={option.value}
            className={theme === option.value ? "theme-option active" : "theme-option"}
            type="button"
            role="radio"
            aria-checked={theme === option.value}
            onClick={() => setTheme(option.value)}
          >
            {option.icon}
            <span>{tx(option.label)}</span>
          </button>
        ))}
      </div>
    </section>
  );
}

function FailoverSettingsPanel({
  snapshot,
  providers,
  drafts,
  busy,
  onDraftChange,
  onSave,
}: {
  snapshot: FailoverSnapshot;
  providers: StoredProvider[];
  drafts: Record<AppKind, FailoverDraft>;
  busy: string | null;
  onDraftChange: (app: AppKind, draft: FailoverDraft) => void;
  onSave: (app: AppKind, event: FormEvent) => void;
}) {
  const { tx } = useI18n();
  const totalQueued = APP_KINDS.reduce((sum, app) => sum + (snapshot.apps[app]?.providerQueue.length || 0), 0);
  const openBreakers = snapshot.breakers.filter((breaker) => breaker.state !== "closed").length;
  return (
    <section className="settings-card wide settings-failover-card">
      <SectionHeader
        icon={<Shuffle size={17} />}
        title={tx("Failover")}
        subtitle={tx("Automatic provider queue and circuit breaker strategy")}
      />
      <div className="settings-policy-grid">
        <KeyValue label="enabled apps" value={APP_KINDS.filter((app) => snapshot.apps[app]?.enabled).length} />
        <KeyValue label="queued providers" value={totalQueued} />
        <KeyValue label="open breakers" value={openBreakers} />
      </div>
      <div className="failover-settings-grid">
        {APP_KINDS.map((app) => {
          const draft = drafts[app];
          const appProviders = providers.filter((item) => item.app === app);
          const providerNames = new Map(appProviders.map((item) => [item.provider.id, item.provider.name || item.provider.id]));
          const breakers = snapshot.breakers.filter((breaker) => breaker.app === app && breaker.state !== "closed");
          return (
            <form className="failover-settings-app" key={app} onSubmit={(event) => onSave(app, event)}>
              <header>
                <div>
                  <strong>{appLabel(app)}</strong>
                  <span>{tx("{{count}} providers available", { count: appProviders.length })}</span>
                </div>
                <StatusPill tone={draft.enabled ? "success" : "warning"}>
                  {draft.enabled ? tx("enabled") : tx("disabled")}
                </StatusPill>
              </header>
              <label className="toggle-row">
                <input
                  type="checkbox"
                  checked={draft.enabled}
                  onChange={(event) => onDraftChange(app, { ...draft, enabled: event.target.checked })}
                />
                <span>{tx("Enable automatic failover")}</span>
              </label>
              <div className="failover-number-grid">
                <label>
                  <span>{tx("Failure threshold")}</span>
                  <input
                    type="number"
                    min={1}
                    step={1}
                    value={draft.failureThreshold}
                    onChange={(event) => onDraftChange(app, { ...draft, failureThreshold: event.target.value })}
                  />
                </label>
                <label>
                  <span>{tx("Open duration seconds")}</span>
                  <input
                    type="number"
                    min={1}
                    step={1}
                    value={draft.openDurationSeconds}
                    onChange={(event) => onDraftChange(app, { ...draft, openDurationSeconds: event.target.value })}
                  />
                </label>
                <label>
                  <span>{tx("Half-open probes")}</span>
                  <input
                    type="number"
                    min={1}
                    step={1}
                    value={draft.halfOpenMaxProbes}
                    onChange={(event) => onDraftChange(app, { ...draft, halfOpenMaxProbes: event.target.value })}
                  />
                </label>
              </div>
              <div className="failover-queue-summary">
                <div className="section-title-row compact-title">
                  <Shuffle size={15} />
                  <h3>{tx("Provider queue")}</h3>
                </div>
                {draft.providerQueue.length ? (
                  <ol>
                    {draft.providerQueue.map((providerId, index) => (
                      <li key={providerId}>
                        <span>{tx("P{{priority}}", { priority: index + 1 })}</span>
                        <strong>{providerNames.get(providerId) || providerId}</strong>
                        <code>{providerId}</code>
                      </li>
                    ))}
                  </ol>
                ) : (
                  <p>{tx("Queue is empty. Add providers from provider cards.")}</p>
                )}
              </div>
              <div className="failover-breaker-summary">
                <span>{tx("Breakers")}</span>
                {breakers.length ? (
                  breakers.slice(0, 4).map((breaker) => (
                    <StatusPill key={breaker.providerId} tone={breaker.state === "open" ? "danger" : "warning"}>
                      {providerNames.get(breaker.providerId) || breaker.providerId}: {breaker.state}
                    </StatusPill>
                  ))
                ) : (
                  <StatusPill tone="success">{tx("closed")}</StatusPill>
                )}
              </div>
              <FormFooter busy={busy === `failover-save:${app}`} label={tx("Save failover")} />
            </form>
          );
        })}
      </div>
    </section>
  );
}

function DirectoryPanel({ runtimeContext }: { runtimeContext: WebRuntimeContext | null }) {
  const { tx } = useI18n();
  const configDir = runtimeContext?.runtime?.configDir || "~/.cc-switch-server";
  const webDistDir = runtimeContext?.runtime?.webDistDir || tx("embedded web assets");
  const embeddedAssets = runtimeContext?.runtime?.embeddedWebAssets ?? "-";
  const files = [
    "server.json",
    "providers.json",
    "universal-providers.json",
    "accounts.json",
    "shares.json",
    "usage-logs.json",
    "usage-logs.jsonl",
    "usage-rollups.json",
    "model-pricing.json",
    "failover.json",
    "tunnels.json",
    "email-auth.json",
  ];
  return (
    <section className="settings-card wide settings-directory-card">
      <SectionHeader
        icon={<FolderOpen size={17} />}
        title={tx("Directory")}
        subtitle={tx("Server data and embedded web asset locations")}
      />
      <div className="settings-policy-grid">
        <KeyValue label="config dir" value={configDir} />
        <KeyValue label="web dist" value={webDistDir} />
        <KeyValue label="embedded assets" value={embeddedAssets} />
        <KeyValue label="mode" value={runtimeContext?.mode || "-"} />
      </div>
      <div className="settings-directory-list">
        {files.map((file) => (
          <div className="settings-directory-row" key={file}>
            <span>{file}</span>
            <code>{joinPath(configDir, file)}</code>
          </div>
        ))}
      </div>
    </section>
  );
}

function joinPath(dir: string, file: string): string {
  if (!dir || dir === "~/.cc-switch-server") return `~/.cc-switch-server/${file}`;
  return `${dir.replace(/\/+$/, "")}/${file}`;
}

function AboutPanel({ buildInfo }: { buildInfo: BuildInfo }) {
  const { tx } = useI18n();
  const sourceUrl = buildInfo.commitId
    ? `https://github.com/Xiechengqi/cc-switch-server/commit/${buildInfo.commitId}`
    : "https://github.com/Xiechengqi/cc-switch-server";
  return (
    <section className="settings-card wide settings-about-card">
      <SectionHeader
        icon={<Info size={17} />}
        title={tx("About")}
        subtitle={buildInfo.versionLine || `${buildInfo.name} ${buildInfo.version}`}
      />
      <div className="settings-about-hero">
        <div>
          <strong>{tx("CC Switch Server")}</strong>
          <span>{buildInfo.name}</span>
        </div>
        <StatusPill tone={buildInfo.dirty ? "warning" : "success"}>
          {buildInfo.dirty ? tx("dirty build") : tx("clean build")}
        </StatusPill>
      </div>
      <div className="settings-policy-grid">
        <KeyValue label="version" value={buildInfo.version} />
        <KeyValue label="commit" value={buildInfo.commitShort || "-"} />
        <KeyValue label="commit time" value={buildInfo.commitTime || "-"} />
        <KeyValue label="build time" value={buildInfo.buildTime || "-"} />
        <KeyValue label="target" value={buildInfo.target || "-"} />
        <KeyValue label="profile" value={buildInfo.profile || "-"} />
        <KeyValue label="rustc" value={buildInfo.rustcVersion || "-"} />
        <KeyValue label="dirty" value={buildInfo.dirty ? "yes" : "no"} />
      </div>
      <div className="settings-commit-card">
        <div className="section-title-row compact-title">
          <GitCommit size={16} />
          <h3>{tx("Commit message")}</h3>
        </div>
        <p>{buildInfo.commitMessage || "-"}</p>
        <a className="inline-link" href={sourceUrl} target="_blank" rel="noreferrer">
          <GitCommit size={14} />
          <span>{tx("Open source commit")}</span>
        </a>
      </div>
    </section>
  );
}

function SettingsReadinessPanel({ data }: { data: SettingsDashboardData }) {
  const { t } = useI18n();
  const items = settingsReadinessItems(data);
  return (
    <section className="settings-card wide settings-readiness-panel">
      <SectionHeader
        icon={<ShieldCheck size={17} />}
        title={t("server.settings.runtimeReadiness")}
        subtitle={t("server.settings.readinessSubtitle", {
          ready: items.filter((item) => item.tone === "success").length,
          total: items.length,
        })}
      />
      <div className="settings-readiness-grid">
        {items.map((item) => (
          <div className="settings-readiness-item" key={item.label}>
            <div>
              <strong>{item.label}</strong>
              <span>{item.detail}</span>
            </div>
            <StatusPill tone={item.tone}>{item.value}</StatusPill>
          </div>
        ))}
      </div>
    </section>
  );
}

function SettingsOverviewStrip({ data }: { data: SettingsDashboardData }) {
  const { t, tx } = useI18n();
  const tunnelStatus = data.tunnel.runtimeStatus?.status || data.tunnel.tunnelStatus || "-";
  const items: Array<{ label: string; value: ReactNode; detail: string; tone: "success" | "warning" | "danger" }> = [
    {
      label: t("server.settings.owner"),
      value: data.config.ownerEmail || "-",
      detail: data.config.ownerEmail ? tx("owner-bound") : tx("owner pending"),
      tone: data.config.ownerEmail ? "success" : "warning",
    },
    {
      label: t("server.settings.router"),
      value: data.routerStatus.registered ? tx("registered") : tx("not registered"),
      detail: data.routerStatus.lastError || formatTime(data.routerStatus.lastHeartbeatMs),
      tone: data.routerStatus.lastError ? "danger" : data.routerStatus.registered ? "success" : "warning",
    },
    {
      label: t("server.settings.tunnel"),
      value: tunnelStatus,
      detail: data.tunnel.runtimeStatus?.tunnelUrl || data.tunnel.tunnelSubdomain || "-",
      tone: diagnosticTone(tunnelStatus, data.tunnel.runtimeStatus?.lastError),
    },
    {
      label: t("server.settings.pendingLogs"),
      value: data.routerStatus.pendingRequestLogSync,
      detail: tx("request log sync backlog"),
      tone: data.routerStatus.pendingRequestLogSync > 0 ? "warning" : "success",
    },
    {
      label: t("server.settings.backups"),
      value: data.backups.length,
      detail: data.backups[0] ? formatTime(Math.max(...data.backups.map((backup) => backup.createdAtMs))) : tx("no snapshots"),
      tone: data.backups.length ? "success" : "warning",
    },
  ];
  return (
    <div className="settings-overview-strip">
      {items.map((item) => (
        <article className="settings-overview-card" key={item.label}>
          <div>
            <span>{item.label}</span>
            <strong>{item.value}</strong>
            <small>{item.detail}</small>
          </div>
          <StatusPill tone={item.tone}>{item.tone}</StatusPill>
        </article>
      ))}
    </div>
  );
}

function BackupPolicySummary({ backups }: { backups: BackupManifest[] }) {
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

function ImportExportPanel({
  busy,
  runAction,
}: {
  busy: string | null;
  runAction: (action: string, task: () => Promise<string>) => Promise<void>;
}) {
  const { tx } = useI18n();
  return (
    <>
      <section className="settings-card wide">
        <SectionHeader
          icon={<FileJson size={17} />}
          title={tx("Import / Export")}
          subtitle={tx("Move server provider, share, and universal provider JSON data")}
        />
        <div className="settings-import-export-grid">
          <ImportExportCard<StoredProvider>
            title={tx("Providers")}
            subtitle={tx("Claude, Codex, and Gemini provider configurations")}
            actionKey="providers"
            busy={busy}
            exportData={exportProviders}
            importData={importProviders}
            normalize={normalizeProvidersImport}
            runAction={runAction}
          />
          <ImportExportCard<ShareRecord>
            title={tx("Shares")}
            subtitle={tx("Share records, bindings, ACL, tunnel, and market metadata")}
            actionKey="shares"
            busy={busy}
            exportData={exportShares}
            importData={importShares}
            normalize={normalizeSharesImport}
            runAction={runAction}
          />
          <ImportExportCard<UniversalProvider>
            title={tx("Universal Providers")}
            subtitle={tx("Reusable provider templates shared across supported apps")}
            actionKey="universal"
            exportKey="providers"
            busy={busy}
            exportData={exportUniversalProviders}
            importData={importUniversalProviders}
            normalize={normalizeUniversalProvidersImport}
            runAction={runAction}
          />
        </div>
      </section>
    </>
  );
}

function ImportExportCard<T>({
  title,
  subtitle,
  actionKey,
  exportKey,
  busy,
  exportData,
  importData,
  normalize,
  runAction,
}: {
  title: string;
  subtitle: string;
  actionKey: string;
  exportKey?: string;
  busy: string | null;
  exportData: () => Promise<T[]>;
  importData: (items: T[]) => Promise<number>;
  normalize: (value: unknown) => T[];
  runAction: (action: string, task: () => Promise<string>) => Promise<void>;
}) {
  const { tx } = useI18n();
  const [text, setText] = useState("");
  const [importConfirmOpen, setImportConfirmOpen] = useState(false);
  const exportBusy = busy === `import-export:${actionKey}:export`;
  const importBusy = busy === `import-export:${actionKey}:import`;

  async function exportAction() {
    await runAction(`import-export:${actionKey}:export`, async () => {
      const items = await exportData();
      setText(formatExportJson(exportKey || actionKey, items));
      return tx("exported {{count}} {{name}}", { count: items.length, name: title });
    });
  }

  async function importAction() {
    await runAction(`import-export:${actionKey}:import`, async () => {
      const items = normalize(parseJsonText(text));
      const count = await importData(items);
      return tx("imported {{count}} {{name}}", { count, name: title });
    });
  }

  return (
    <>
      <article className="settings-import-export-card">
        <header>
          <div>
            <h3>{title}</h3>
            <span>{subtitle}</span>
          </div>
        </header>
        <textarea
          value={text}
          onChange={(event) => setText(event.target.value)}
          spellCheck={false}
          placeholder={tx("Export JSON appears here, or paste JSON to import")}
        />
        <div className="settings-actions">
          <button className="secondary-button" type="button" onClick={() => void exportAction()} disabled={exportBusy}>
            {exportBusy ? <Loader2 size={15} /> : <Download size={15} />}
            <span>{tx("Export")}</span>
          </button>
          <button
            className="primary-button"
            type="button"
            onClick={() => setImportConfirmOpen(true)}
            disabled={importBusy || !text.trim()}
          >
            {importBusy ? <Loader2 size={15} /> : <Upload size={15} />}
            <span>{tx("Import")}</span>
          </button>
        </div>
      </article>
      <ConfirmDialog
        isOpen={importConfirmOpen}
        title={tx("Import {{name}}", { name: title })}
        message={tx("Import pasted JSON into {{name}}? Existing records with matching IDs may be updated.", { name: title })}
        confirmText={tx("Import")}
        variant="info"
        onConfirm={() => {
          setImportConfirmOpen(false);
          void importAction();
        }}
        onCancel={() => setImportConfirmOpen(false)}
      />
    </>
  );
}

function formatExportJson(key: string, items: unknown[]): string {
  return JSON.stringify({ [key]: items }, null, 2);
}

function parseJsonText(text: string): unknown {
  if (!text.trim()) {
    throw new Error("import JSON is required");
  }
  try {
    return JSON.parse(text);
  } catch (error) {
    throw new Error(`import JSON is invalid: ${errorMessage(error)}`);
  }
}

function normalizeProvidersImport(value: unknown): StoredProvider[] {
  return normalizeArrayProperty<StoredProvider>(value, "providers");
}

function normalizeSharesImport(value: unknown): ShareRecord[] {
  return normalizeArrayProperty<ShareRecord>(value, "shares");
}

function normalizeUniversalProvidersImport(value: unknown): UniversalProvider[] {
  return normalizeArrayProperty<UniversalProvider>(value, "universal");
}

function normalizeArrayProperty<T>(value: unknown, key: string): T[] {
  if (Array.isArray(value)) return value as T[];
  if (isRecord(value)) {
    const byKey = value[key];
    if (Array.isArray(byKey)) return byKey as T[];
    if (key === "universal" && Array.isArray(value.providers)) return value.providers as T[];
  }
  throw new Error(`${key} import must be an array or { "${key}": [...] }`);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function DiagnosticsSummary({ diagnostics }: { diagnostics?: RouterDiagnosticsResponse }) {
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

function TextField({ label, value, onChange }: { label: string; value: string; onChange: (value: string) => void }) {
  const { tx } = useI18n();
  return (
    <label>
      <span>{tx(label)}</span>
      <input value={value} onChange={(event) => onChange(event.target.value)} />
    </label>
  );
}

function FormFooter({ busy, label }: { busy: boolean; label: string }) {
  const { tx } = useI18n();
  return (
    <button className="primary-button" type="submit" disabled={busy}>
      {busy ? <Loader2 size={15} /> : <Save size={15} />}
      <span>{tx(label)}</span>
    </button>
  );
}

function ActionButton({
  label,
  icon,
  busy,
  disabled,
  onClick,
}: {
  label: string;
  icon: ReactNode;
  busy: boolean;
  disabled?: boolean;
  onClick: () => void;
}) {
  const { tx } = useI18n();
  return (
    <button className="secondary-button" type="button" onClick={onClick} disabled={busy || disabled}>
      {busy ? <Loader2 size={15} /> : icon}
      <span>{tx(label)}</span>
    </button>
  );
}

function isClientTunnelRunning(status?: string | null): boolean {
  const normalized = status?.trim().toLowerCase();
  return Boolean(
    normalized &&
      !["stopped", "ended", "error", "failed"].includes(normalized),
  );
}

function RouterFacts({ router, status }: { router?: RouterConfigView; status?: RouterStatusResponse }) {
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

function TunnelStatus({ status }: { status?: TunnelRuntimeStatus | null }) {
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

function BackupTable({
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

function Diagnostics({ diagnostics }: { diagnostics?: RouterDiagnosticsResponse }) {
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

function KeyValue({ label, value }: { label: string; value: ReactNode }) {
  const { tx } = useI18n();
  return (
    <div className="compact-kv">
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

function settingsReadinessItems(data: SettingsDashboardData): Array<{
  label: string;
  value: string;
  detail: string;
  tone: "success" | "warning" | "danger";
}> {
  const latestBackup = [...data.backups].sort((left, right) => right.createdAtMs - left.createdAtMs)[0];
  const latestBackupAge = latestBackup ? Date.now() - latestBackup.createdAtMs : Number.POSITIVE_INFINITY;
  const tunnelErrors = data.diagnostics.tunnels.filter((tunnel) => tunnel.lastError).length;
  const shareErrors = data.diagnostics.shareSync.filter((share) => share.routerLastSyncError).length;
  const diagnosticsIssues = tunnelErrors + shareErrors + (data.routerStatus.lastError ? 1 : 0);
  return [
    {
      label: "setup",
      value: data.config.ownerEmail ? "owner" : "pending",
      detail: data.config.ownerEmail || "no owner email",
      tone: data.config.ownerEmail ? "success" : "warning",
    },
    {
      label: "login",
      value: "email code",
      detail: data.config.ownerEmail ? "owner-bound" : "owner pending",
      tone: data.config.ownerEmail ? "success" : "warning",
    },
    {
      label: "router",
      value: data.routerStatus.registered ? "registered" : "local",
      detail: data.routerStatus.lastError || formatTime(data.routerStatus.lastHeartbeatMs),
      tone: data.routerStatus.lastError ? "danger" : data.routerStatus.registered ? "success" : "warning",
    },
    {
      label: "backup",
      value: latestBackup ? "ready" : "missing",
      detail: latestBackup ? formatTime(latestBackup.createdAtMs) : "no snapshots",
      tone: latestBackupAge <= 24 * 60 * 60 * 1000 ? "success" : latestBackup ? "warning" : "danger",
    },
    {
      label: "diagnostics",
      value: diagnosticsIssues ? `${diagnosticsIssues} issue` : "clean",
      detail: `${data.diagnostics.tunnels.length} tunnels / ${data.diagnostics.shareSync.length} shares`,
      tone: diagnosticsIssues ? "warning" : "success",
    },
  ];
}

function routerState(status?: RouterStatusResponse): string {
  if (!status) return "loading";
  if (status.lastError) return status.lastError;
  return status.registered ? "registered" : "not registered";
}

function routerStatusText(status: RouterStatusResponse): string {
  return status.registered ? `heartbeat ok; pending logs ${status.pendingRequestLogSync}` : "heartbeat recorded locally";
}

function routerDraftFrom(router: RouterConfigView): RouterDraft {
  return {
    url: router.url || "",
    apiBase: router.apiBase || "",
    domain: router.domain || "",
    region: router.region || "",
    sshHost: router.sshHost || "",
    sshUser: router.sshUser || "",
    custom: router.custom,
  };
}

function tunnelDraftFrom(tunnel: { tunnelSubdomain?: string | null; tunnelStatus?: string | null }): TunnelDraft {
  return {
    tunnelSubdomain: tunnel.tunnelSubdomain || "",
    tunnelStatus: tunnel.tunnelStatus || "",
  };
}

function emptyRouterDraft(): RouterDraft {
  return { url: "", apiBase: "", domain: "", region: "", sshHost: "", sshUser: "", custom: false };
}

function emptyTunnelDraft(): TunnelDraft {
  return { tunnelSubdomain: "", tunnelStatus: "" };
}

function emptyProxyDraft(): ProxyDraft {
  return { url: "", clear: false, followSystemProxy: true };
}

function emptyFailoverDrafts(): Record<AppKind, FailoverDraft> {
  return APP_KINDS.reduce(
    (drafts, app) => {
      drafts[app] = failoverDraftFrom();
      return drafts;
    },
    {} as Record<AppKind, FailoverDraft>,
  );
}

function failoverDraftsFrom(snapshot: FailoverSnapshot): Record<AppKind, FailoverDraft> {
  return APP_KINDS.reduce(
    (drafts, app) => {
      drafts[app] = failoverDraftFrom(snapshot.apps[app]);
      return drafts;
    },
    {} as Record<AppKind, FailoverDraft>,
  );
}

function failoverDraftFrom(config?: FailoverSnapshot["apps"][AppKind]): FailoverDraft {
  return {
    enabled: Boolean(config?.enabled),
    providerQueue: [...(config?.providerQueue || [])],
    failureThreshold: String(config?.failureThreshold ?? 2),
    openDurationSeconds: String(Math.max(1, Math.round((config?.openDurationMs ?? 300000) / 1000))),
    halfOpenMaxProbes: String(config?.halfOpenMaxProbes ?? 1),
  };
}

function positiveInteger(value: string, fallback: number): number {
  const parsed = Number.parseInt(value, 10);
  if (!Number.isFinite(parsed) || parsed < 1) return fallback;
  return parsed;
}

function appLabel(app: AppKind): string {
  if (app === "claude") return "Claude Code";
  if (app === "codex") return "Codex";
  return "Gemini";
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

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}
