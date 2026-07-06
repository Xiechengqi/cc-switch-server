import { FormEvent, useCallback, useEffect, useState } from "react";

import {
  AppKind,
  BackupManifest,
  EmailCodeRequestResponse,
  FailoverSnapshot,
  RouterDiagnosticsResponse,
  RouterStatusResponse,
  SettingsPageData,
  StoredProvider,
  batchSyncRouterShares,
  claimClientTunnel,
  createBackup,
  heartbeatRouter,
  loadFailoverSnapshot,
  loadSettingsPageData,
  loadStoredProviders,
  registerRouter,
  requestEmailLoginCode,
  restoreBackup,
  rotateApiToken,
  startClientTunnel,
  stopClientTunnel,
  updateClientTunnel,
  updateFailoverApp,
  updateRouterConfig,
  updateUpstreamProxy,
  verifyEmailLoginCode,
} from "@/lib/server-legacy-api";
import { writeToken } from "@/lib/runtime";
import { useI18n } from "@/lib/i18n";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { LoadingBlock } from "@/components/LoadingBlock";
import {
  ServerAdminAuthPanel,
  BackupSettingsPanel,
  DiagnosticsSettingsPanel,
} from "@/components/settings/SettingsAccountPanels";
import { FailoverSettingsPanel } from "@/components/settings/FailoverSettingsPanel";
import {
  ProxySettingsPanel,
  RouterSettingsPanel,
  TunnelSettingsPanel,
} from "@/components/settings/SettingsConnectionPanels";
import { ImportExportPanel } from "@/components/settings/ImportExportPanel";
import {
  appLabel,
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

export type ServerSettingsTab =
  | "proxy"
  | "failover"
  | "router"
  | "tunnel"
  | "auth"
  | "backup"
  | "importExport"
  | "diagnostics";

interface ServerSettingsExtensionsProps {
  activeTab?: ServerSettingsTab;
  /** Render multiple server panels stacked (used inside desktop advanced tab). */
  sections?: ServerSettingsTab[];
}

/**
 * Server-only settings panels (router/tunnel/failover/diagnostics/auth/backup/
 * import-export) wired to the legacy server REST surface. Rendered inside the
 * desktop-style SettingsPage as extension tabs after the proxy tab.
 */
export function ServerSettingsExtensions({
  activeTab,
  sections,
}: ServerSettingsExtensionsProps) {
  const { t, tx } = useI18n();
  const [data, setData] = useState<SettingsPageData | null>(null);
  const [failoverSnapshot, setFailoverSnapshot] = useState<FailoverSnapshot>({
    apps: {},
    breakers: [],
  });
  const [settingsProviders, setSettingsProviders] = useState<StoredProvider[]>(
    [],
  );
  const [routerDraft, setRouterDraft] = useState<RouterDraft>(
    emptyRouterDraft(),
  );
  const [tunnelDraft, setTunnelDraft] = useState<TunnelDraft>(
    emptyTunnelDraft(),
  );
  const [proxyDraft, setProxyDraft] = useState<ProxyDraft>(emptyProxyDraft());
  const [failoverDrafts, setFailoverDrafts] = useState<
    Record<AppKind, FailoverDraft>
  >(emptyFailoverDrafts());
  const [emailDraft, setEmailDraft] = useState<EmailDraft>({
    email: "",
    code: "",
  });
  const [backupReason, setBackupReason] = useState("");
  const [apiToken, setApiToken] = useState<string | null>(null);
  const [apiTokenCopyStatus, setApiTokenCopyStatus] = useState<
    { tone: "success" | "warning"; message: string } | null
  >(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const [restoreConfirm, setRestoreConfirm] = useState<BackupManifest | null>(
    null,
  );
  const [rotateTokenConfirm, setRotateTokenConfirm] = useState(false);
  const [routerSyncConfirm, setRouterSyncConfirm] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [next, snapshot, providers] = await Promise.all([
        loadSettingsPageData(),
        loadFailoverSnapshot(),
        loadStoredProviders(),
      ]);
      setData(next);
      setFailoverSnapshot(snapshot);
      setSettingsProviders(providers);
      setFailoverDrafts(failoverDraftsFrom(snapshot));
      setRouterDraft(routerDraftFrom(next.router));
      setTunnelDraft(tunnelDraftFrom(next.tunnel));
      setProxyDraft({
        url: "",
        clear: false,
        followSystemProxy: next.config?.upstreamProxy.followSystemProxy ?? false,
      });
      setEmailDraft((current) => ({
        ...current,
        email: current.email || next.config?.ownerEmail || "",
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

  const runAction = useCallback(
    async (action: string, task: () => Promise<string>) => {
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
    },
    [refresh],
  );

  const saveRouter = useCallback(
    async (event: FormEvent) => {
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
    },
    [routerDraft, runAction, tx],
  );

  const saveTunnel = useCallback(
    async (event: FormEvent) => {
      event.preventDefault();
      await runAction("tunnel-save", async () => {
        const tunnel = await updateClientTunnel({
          tunnelSubdomain: tunnelDraft.tunnelSubdomain,
          tunnelStatus: tunnelDraft.tunnelStatus,
        });
        return tx("client tunnel saved: {{value}}", {
          value: tunnel.tunnelSubdomain || "-",
        });
      });
    },
    [tunnelDraft, runAction, tx],
  );

  const saveProxy = useCallback(
    async (event: FormEvent) => {
      event.preventDefault();
      await runAction("proxy-save", async () => {
        const proxy = await updateUpstreamProxy({
          url: proxyDraft.url.trim() || undefined,
          clear: proxyDraft.clear,
          followSystemProxy: proxyDraft.followSystemProxy,
        });
        return proxy.enabled
          ? tx("proxy saved: {{value}}", {
              value: proxy.maskedUrl || tx("configured"),
            })
          : tx("proxy disabled");
      });
    },
    [proxyDraft, runAction, tx],
  );

  const saveFailover = useCallback(
    async (app: AppKind, event: FormEvent) => {
      event.preventDefault();
      const draft = failoverDrafts[app];
      await runAction(`failover-save:${app}`, async () => {
        const config = await updateFailoverApp(app, {
          enabled: draft.enabled,
          providerQueue: draft.providerQueue,
          failureThreshold: positiveInteger(draft.failureThreshold, 2),
          openDurationMs:
            positiveInteger(draft.openDurationSeconds, 300) * 1000,
          halfOpenMaxProbes: positiveInteger(draft.halfOpenMaxProbes, 1),
        });
        return config.enabled
          ? tx("{{app}} failover enabled", { app: appLabel(app) })
          : tx("{{app}} failover disabled", { app: appLabel(app) });
      });
    },
    [failoverDrafts, runAction, tx],
  );

  const makeBackup = useCallback(
    async (event: FormEvent) => {
      event.preventDefault();
      await runAction("backup-create", async () => {
        const backup = await createBackup(backupReason);
        setBackupReason("");
        return tx("backup created: {{id}}", { id: backup.id });
      });
    },
    [backupReason, runAction, tx],
  );

  const restoreBackupAction = useCallback(
    async (backup: BackupManifest) => {
      await runAction(`backup-restore:${backup.id}`, async () => {
        const restored = await restoreBackup(backup.id);
        return tx("restored {{id}}; safety {{safety}}", {
          id: restored.restored.id,
          safety: restored.preRestore?.id || "-",
        });
      });
    },
    [runAction, tx],
  );

  const rotateTokenAction = useCallback(async () => {
    await runAction("api-token", async () => {
      const token = await rotateApiToken();
      setApiToken(token);
      setApiTokenCopyStatus(null);
      return tx("new API token generated");
    });
  }, [runAction, tx]);

  const copyApiToken = useCallback(async () => {
    if (!apiToken) return;
    if (!navigator.clipboard?.writeText) {
      setApiTokenCopyStatus({
        tone: "warning",
        message: tx("Clipboard unavailable; copy the visible value manually."),
      });
      return;
    }
    try {
      await navigator.clipboard.writeText(apiToken);
      setApiTokenCopyStatus({
        tone: "success",
        message: tx("API token copied"),
      });
    } catch {
      setApiTokenCopyStatus({
        tone: "warning",
        message: tx("Copy failed; copy the visible value manually."),
      });
    }
  }, [apiToken, tx]);

  const requestCodeAction = useCallback(async () => {
    await runAction("email-request", async () => {
      const response: EmailCodeRequestResponse =
        await requestEmailLoginCode(emailDraft.email);
      return tx("code sent to {{destination}}; cooldown {{seconds}}s", {
        destination: response.maskedDestination,
        seconds: response.cooldownSecs,
      });
    });
  }, [emailDraft.email, runAction, tx]);

  const verifyCodeAction = useCallback(async () => {
    await runAction("email-verify", async () => {
      const login = await verifyEmailLoginCode(emailDraft);
      writeToken(login.token);
      return tx("email verified and local session updated");
    });
  }, [emailDraft, runAction, tx]);

  if (loading && !data) {
    return <LoadingBlock label="server.settings.loading" />;
  }

  const clientTunnelRunning = isClientTunnelRunning(
    data?.tunnel.runtimeStatus?.status,
  );
  const visibleTabs = sections ?? (activeTab ? [activeTab] : []);
  const shows = (tab: ServerSettingsTab) => visibleTabs.includes(tab);
  const stacked = visibleTabs.length > 1;

  return (
    <div className={stacked ? "space-y-8" : "settings-layout"}>
      {!stacked && (
      <div className="provider-toolbar">
        <div className="provider-toolbar-status">
          <span>
            {data?.config?.ownerEmail || t("server.settings.runtimeSubtitle")}
          </span>
        </div>
        <div className="provider-toolbar-actions">
          {error && <span className="error-text">{error}</span>}
          {result && <span className="usage-result">{result}</span>}
          <button
            className="secondary-button"
            type="button"
            onClick={() => void refresh()}
          >
            <span>{t("common.refresh")}</span>
          </button>
        </div>
      </div>
      )}

      {shows("proxy") && (
        <ProxySettingsPanel
          maskedUrl={data?.config?.upstreamProxy.maskedUrl}
          draft={proxyDraft}
          busy={busy}
          onDraftChange={setProxyDraft}
          onSave={saveProxy}
        />
      )}

      {shows("failover") && (
        <FailoverSettingsPanel
          snapshot={failoverSnapshot}
          providers={settingsProviders}
          drafts={failoverDrafts}
          busy={busy}
          onDraftChange={(app, draft) =>
            setFailoverDrafts((current) => ({ ...current, [app]: draft }))
          }
          onSave={saveFailover}
        />
      )}

      {shows("router") && (
        <RouterSettingsPanel
          router={data?.router}
          status={data?.routerStatus as RouterStatusResponse | undefined}
          draft={routerDraft}
          busy={busy}
          onDraftChange={setRouterDraft}
          onSave={saveRouter}
          onRegister={() =>
            void runAction("router-register", async () =>
              `registered ${JSON.stringify(await registerRouter())}`,
            )
          }
          onHeartbeat={() =>
            void runAction("router-heartbeat", async () =>
              routerStatusText(await heartbeatRouter()),
            )
          }
          onBatchSync={() => setRouterSyncConfirm(true)}
        />
      )}

      {shows("tunnel") && (
        <TunnelSettingsPanel
          tunnel={data?.tunnel}
          draft={tunnelDraft}
          busy={busy}
          clientTunnelRunning={clientTunnelRunning}
          onDraftChange={setTunnelDraft}
          onSave={saveTunnel}
          onClaim={() =>
            void runAction("tunnel-claim", async () =>
              `claim ${JSON.stringify(await claimClientTunnel())}`,
            )
          }
          onStart={() =>
            void runAction("tunnel-start", async () =>
              (await startClientTunnel()).message,
            )
          }
          onStop={() =>
            void runAction("tunnel-stop", async () =>
              `stopped ${(await stopClientTunnel()).tunnelStatus || "client tunnel"}`,
            )
          }
        />
      )}

      {shows("auth") && (
        <ServerAdminAuthPanel
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
      )}

      {shows("backup") && (
        <BackupSettingsPanel
          backups={data?.backups || []}
          backupReason={backupReason}
          busy={busy}
          onBackupReasonChange={setBackupReason}
          onCreateBackup={makeBackup}
          onRestore={setRestoreConfirm}
        />
      )}

      {shows("importExport") && (
        <ImportExportPanel busy={busy} runAction={runAction} />
      )}

      {shows("diagnostics") && (
        <DiagnosticsSettingsPanel
          diagnostics={
            data?.diagnostics as RouterDiagnosticsResponse | undefined
          }
        />
      )}

      <ConfirmDialog
        isOpen={routerSyncConfirm}
        title={tx("Batch sync router shares")}
        message={tx(
          "Batch sync share state to the router? Remote router records for matching shares may be updated.",
        )}
        confirmText={tx("Sync")}
        onConfirm={() => {
          setRouterSyncConfirm(false);
          void runAction("router-sync", async () =>
            (await batchSyncRouterShares()).message,
          );
        }}
        onCancel={() => setRouterSyncConfirm(false)}
      />
      <ConfirmDialog
        isOpen={rotateTokenConfirm}
        title={tx("Rotate API token")}
        message={tx(
          "Rotate the server API token? Existing clients using the current token will stop working until updated.",
        )}
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
        message={tx(
          "Restore backup {{id}}? Current stores will be backed up first.",
          { id: restoreConfirm?.id || "-" },
        )}
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
