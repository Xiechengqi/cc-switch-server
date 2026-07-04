import {
  ArrowLeft,
  BarChart3,
  Download,
  FolderArchive,
  KeyRound,
  Layers3,
  Network,
  Plus,
  RefreshCw,
  Settings,
  Share2,
  Shuffle,
} from "lucide-react";
import { FormEvent, ReactNode, useCallback, useEffect, useMemo, useState } from "react";

import { AppKind, BuildInfo, loadBuildInfo } from "@/lib/api";
import {
  getWebRuntimeContext,
  jsonFetch,
  readToken,
  WebRuntimeContext,
  writeToken,
} from "@/lib/runtime";
import { useI18n } from "@/lib/i18n";
import { ProviderIcon } from "@/components/ProviderIcon";
import { ProviderDashboard } from "@/components/ProviderDashboard";
import { SettingsDashboard, SettingsTab } from "@/components/SettingsDashboard";
import { ShareDashboard } from "@/components/ShareDashboard";
import { UniversalDashboard } from "@/components/UniversalDashboard";
import { UsageDashboard, UsageTab } from "@/components/UsageDashboard";
import { appIcon } from "@/lib/provider-icons";

type View = "providers" | "shares" | "usage" | "settings" | "universal";
type UsageFocus = { app: AppKind; providerId: string; tab: UsageTab; key: number };

const appTabs: Array<{ id: AppKind; label: string; iconName: string; iconColor?: string }> = [
  { id: "claude", label: "Claude Code", iconName: appIcon("claude").icon, iconColor: appIcon("claude").color },
  { id: "codex", label: "Codex", iconName: appIcon("codex").icon, iconColor: appIcon("codex").color },
  { id: "gemini", label: "Gemini", iconName: appIcon("gemini").icon, iconColor: appIcon("gemini").color },
];

function App() {
  const { t, tx } = useI18n();
  const [context, setContext] = useState<WebRuntimeContext | null>(null);
  const [view, setView] = useState<View>("providers");
  const [activeApp, setActiveApp] = useState<AppKind>("claude");
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("general");
  const [usageFocus, setUsageFocus] = useState<UsageFocus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [buildInfo, setBuildInfo] = useState<BuildInfo | null>(null);
  const isAuthenticated = context?.mode === "local-admin";

  const refreshContext = useCallback(async () => {
    const next = await getWebRuntimeContext();
    setContext(next);
    return next;
  }, []);

  const loadData = useCallback(async () => {
    const next = await refreshContext();
    if (next.mode !== "local-admin") {
      return;
    }
  }, [refreshContext]);

  useEffect(() => {
    let active = true;
    setLoading(true);
    loadData()
      .catch((reason) => {
        if (active) setError(errorMessage(reason));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [loadData]);

  useEffect(() => {
    if (!isAuthenticated) {
      setBuildInfo(null);
      return;
    }
    let active = true;
    loadBuildInfo()
      .then((info) => {
        if (active) setBuildInfo(info);
      })
      .catch(() => {
        if (active) setBuildInfo(null);
      });
    return () => {
      active = false;
    };
  }, [isAuthenticated]);

  const content = useMemo(() => {
    if (!context) return <EmptyState title={t("common.loading")} value={t("server.common.runtime")} />;
    if (context.mode !== "local-admin") {
      return <LoginPanel context={context} onAuthenticated={loadData} />;
    }
    if (loading) {
      return <EmptyState title={t("common.loading")} value={t("server.common.server")} />;
    }
    switch (view) {
      case "providers":
        return (
          <ProviderDashboard
            activeApp={activeApp}
            onActiveAppChange={setActiveApp}
            onOpenImportExport={() => openSettings("importExport")}
            onOpenUsage={(target) => {
              setUsageFocus({
                app: target.app,
                providerId: target.providerId,
                tab: target.tab,
                key: Date.now(),
              });
              setView("usage");
            }}
          />
        );
      case "shares":
        return <ShareDashboard />;
      case "usage":
        return <UsageDashboard initialFocus={usageFocus} />;
      case "settings":
        return <SettingsDashboard initialTab={settingsTab} />;
      case "universal":
        return <UniversalDashboard />;
    }
  }, [activeApp, context, loading, loadData, settingsTab, t, usageFocus, view]);

  const activeViewLabel = viewLabel(view, t);

  return (
    <main className="app-shell desktop-shell">
      <section className="workspace">
        <header className="desktop-topbar">
          <div className="desktop-topbar-left">
            {view !== "providers" ? (
              <div className="desktop-back-title">
                <button
                  className="icon-button desktop-header-icon"
                  type="button"
                  onClick={() => setView("providers")}
                  aria-label={tx("Back")}
                  title={tx("Back")}
                >
                  <ArrowLeft size={16} />
                </button>
                <h1>{activeViewLabel}</h1>
              </div>
            ) : (
              <div className="desktop-brand-row">
                <button
                  className="desktop-brand-link"
                  type="button"
                  onClick={() => setView("providers")}
                  aria-label={tx("CC Switch")}
                  title={tx("CC Switch")}
                >
                  {tx("CC Switch")}
                </button>
                <button
                  className="icon-button desktop-header-icon"
                  type="button"
                  onClick={() => openSettings("general")}
                  disabled={!isAuthenticated}
                  aria-label={t("server.nav.settings")}
                  title={t("server.nav.settings")}
                >
                  <Settings size={16} />
                </button>
              </div>
            )}

            {isAuthenticated && (
              <div className="desktop-header-switches">
                <HeaderShareToggle active={view === "shares"} onClick={() => setView("shares")} />
                <HeaderFailoverToggle activeApp={activeApp} />
              </div>
            )}
          </div>

          <div className="desktop-app-switcher" role="tablist" aria-label={tx("App switcher")}>
            {appTabs.map((app) => (
              <button
                key={app.id}
                className={app.id === activeApp ? "active" : ""}
                type="button"
                role="tab"
                aria-selected={app.id === activeApp}
                onClick={() => {
                  setActiveApp(app.id);
                  setView("providers");
                }}
                disabled={!isAuthenticated}
              >
                <ProviderIcon
                  icon={app.iconName}
                  name={app.label}
                  color={app.iconColor}
                  size={16}
                  showFallback={false}
                />
                <span>{app.label}</span>
              </button>
            ))}
          </div>

          <div className="desktop-topbar-actions">
            {error && <span className="error-text">{error}</span>}
            {isAuthenticated && (
              <HeaderBuildBadge
                buildInfo={buildInfo}
                onClick={() => openSettings("about")}
              />
            )}
            <button
              className="icon-button desktop-header-icon"
              type="button"
              onClick={() => {
                setUsageFocus(null);
                setView("usage");
              }}
              disabled={!isAuthenticated}
              aria-label={t("server.nav.usage")}
              title={t("server.nav.usage")}
            >
              <BarChart3 size={16} />
            </button>
            <button
              className="icon-button desktop-header-icon"
              type="button"
              onClick={() => setView("universal")}
              disabled={!isAuthenticated}
              aria-label={t("server.nav.universal")}
              title={t("server.nav.universal")}
            >
              <Layers3 size={16} />
            </button>
            <button
              className="icon-button desktop-header-icon"
              type="button"
              onClick={() => setView("shares")}
              disabled={!isAuthenticated}
              aria-label={t("server.nav.shares")}
              title={t("server.nav.shares")}
            >
              <Share2 size={16} />
            </button>
            <button
              className="icon-button desktop-header-icon"
              type="button"
              onClick={() => openSettings("backup")}
              disabled={!isAuthenticated}
              aria-label={tx("Backup")}
              title={tx("Backup")}
            >
              <FolderArchive size={16} />
            </button>
            <button
              className="icon-button desktop-header-icon"
              type="button"
              onClick={() => openSettings("importExport")}
              disabled={!isAuthenticated}
              aria-label={t("common.import")}
              title={t("common.import")}
            >
              <Download size={16} />
            </button>
            {readToken() && (
              <button
                className="icon-button desktop-header-icon"
                type="button"
                onClick={() => {
                  writeToken(null);
                  void refreshContext();
                }}
                aria-label={t("server.common.signOut")}
                title={t("server.common.signOut")}
              >
                <KeyRound size={16} />
              </button>
            )}
            <button
              className="icon-button desktop-header-icon"
              type="button"
              onClick={() => {
                setError(null);
                setLoading(true);
                loadData()
                  .catch((reason) => setError(errorMessage(reason)))
                  .finally(() => setLoading(false));
              }}
              aria-label={t("common.refresh")}
              title={t("common.refresh")}
            >
              <RefreshCw size={16} />
            </button>
            <button
              className="desktop-add-button"
              type="button"
              onClick={() => {
                setView("providers");
                document.dispatchEvent(new CustomEvent("cc-switch-server:add-provider"));
              }}
              disabled={!isAuthenticated}
              aria-label={t("server.providers.addProvider")}
              title={t("server.providers.addProvider")}
            >
              <Plus size={19} />
            </button>
          </div>
        </header>
        {content}
      </section>
    </main>
  );

  function openSettings(tab: SettingsTab) {
    setSettingsTab(tab);
    setView("settings");
  }
}

function HeaderBuildBadge({
  buildInfo,
  onClick,
}: {
  buildInfo: BuildInfo | null;
  onClick: () => void;
}) {
  const { tx } = useI18n();
  const label = buildInfo?.commitShort || buildInfo?.version || tx("build");
  const title = buildInfo
    ? [buildInfo.versionLine || buildInfo.version, buildInfo.commitMessage, buildInfo.buildTime]
      .filter(Boolean)
      .join(" · ")
    : tx("Build info");
  return (
    <button
      className={buildInfo?.dirty ? "desktop-build-badge dirty" : "desktop-build-badge"}
      type="button"
      onClick={onClick}
      title={title}
      aria-label={tx("Build info")}
    >
      <span>{label}</span>
    </button>
  );
}

function HeaderShareToggle({ active, onClick }: { active: boolean; onClick: () => void }) {
  const { tx } = useI18n();
  return (
    <button
      className={active ? "desktop-mini-toggle active" : "desktop-mini-toggle"}
      type="button"
      onClick={onClick}
      title={tx("Share routing")}
      aria-pressed={active}
    >
      <Network size={14} />
      <span>{tx("Share")}</span>
    </button>
  );
}

function HeaderFailoverToggle({ activeApp }: { activeApp: AppKind }) {
  const { tx } = useI18n();
  const [enabled, setEnabled] = useState(false);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    const response = await jsonFetch<{
      failover?: { apps?: Record<string, { enabled?: boolean }> };
    }>("/api/failover");
    setEnabled(Boolean(response.failover?.apps?.[activeApp]?.enabled));
  }, [activeApp]);

  useEffect(() => {
    let active = true;
    refresh().catch(() => {
      if (active) setEnabled(false);
    });
    return () => {
      active = false;
    };
  }, [refresh]);

  async function toggle() {
    setBusy(true);
    try {
      const response = await jsonFetch<{ config?: { enabled?: boolean } }>(`/api/failover/apps/${activeApp}`, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ enabled: !enabled }),
      });
      setEnabled(Boolean(response.config?.enabled));
    } finally {
      setBusy(false);
    }
  }

  return (
    <button
      className={enabled ? "desktop-mini-toggle active" : "desktop-mini-toggle"}
      type="button"
      onClick={() => void toggle()}
      disabled={busy}
      title={tx("Auto failover")}
      aria-pressed={enabled}
    >
      <Shuffle size={14} />
      <span>{tx("Failover")}</span>
    </button>
  );
}

function viewLabel(view: View, t: (key: string) => string): string {
  const labels: Record<View, string> = {
    providers: t("server.nav.providers"),
    shares: t("server.nav.shares"),
    usage: t("server.nav.usage"),
    settings: t("server.nav.settings"),
    universal: t("server.nav.universal"),
  };
  return labels[view];
}

function LoginPanel({
  context,
  onAuthenticated,
}: {
  context: WebRuntimeContext;
  onAuthenticated: () => Promise<void>;
}) {
  const { t } = useI18n();
  const setupRequired = context.status === "setup-required" || context.auth?.setupRequired;
  const [password, setPassword] = useState("");
  const [ownerEmail, setOwnerEmail] = useState("");
  const [routerUrl, setRouterUrl] = useState("https://jptokenswitch.cc");
  const [clientTunnelSubdomain, setClientTunnelSubdomain] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setError(null);
    try {
      if (setupRequired) {
        await jsonFetch("/api/setup", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            password,
            ownerEmail,
            routerUrl,
            clientTunnelSubdomain,
          }),
        });
      }
      const login = await jsonFetch<{ token: string }>("/api/auth/login", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ method: "password", password }),
      });
      writeToken(login.token);
      await onAuthenticated();
    } catch (reason) {
      setError(errorMessage(reason));
    }
  }

  return (
    <form className="auth-panel" onSubmit={submit}>
      <div className="auth-grid">
        {setupRequired && (
          <>
            <label>
              <span>{t("server.auth.ownerEmail")}</span>
              <input value={ownerEmail} onChange={(event) => setOwnerEmail(event.target.value)} />
            </label>
            <label>
              <span>{t("server.auth.routerUrl")}</span>
              <input value={routerUrl} onChange={(event) => setRouterUrl(event.target.value)} />
            </label>
            <label>
              <span>{t("server.auth.clientSubdomain")}</span>
              <input
                value={clientTunnelSubdomain}
                onChange={(event) => setClientTunnelSubdomain(event.target.value)}
              />
            </label>
          </>
        )}
        <label>
          <span>{t("server.common.password")}</span>
          <input
            type="password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
          />
        </label>
      </div>
      {error && <div className="form-error">{error}</div>}
      <button className="primary-button" type="submit">
        <KeyRound size={16} />
        <span>{setupRequired ? t("server.common.setup") : t("server.common.login")}</span>
      </button>
    </form>
  );
}

function Panel({ children }: { children: ReactNode }) {
  return <div className="panel">{children}</div>;
}

function StatusPill({ children, tone }: { children: ReactNode; tone: "success" | "warning" }) {
  return <span className={`status-pill ${tone}`}>{children}</span>;
}

function EmptyState({ title, value }: { title: string; value: ReactNode }) {
  return (
    <Panel>
      <div className="empty-state">
        <Network size={24} />
        <strong>{title}</strong>
        <span>{value}</span>
      </div>
    </Panel>
  );
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export default App;
