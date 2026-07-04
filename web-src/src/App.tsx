import {
  BarChart3,
  Boxes,
  Cloud,
  KeyRound,
  Layers3,
  Network,
  RefreshCw,
  Settings,
  Share2,
} from "lucide-react";
import { FormEvent, ReactNode, useCallback, useEffect, useMemo, useState } from "react";

import {
  getWebRuntimeContext,
  jsonFetch,
  readToken,
  WebRuntimeContext,
  writeToken,
} from "@/lib/runtime";
import { useI18n } from "@/lib/i18n";
import { AccountsDashboard } from "@/components/AccountsDashboard";
import { ProviderDashboard } from "@/components/ProviderDashboard";
import { SettingsDashboard } from "@/components/SettingsDashboard";
import { ShareDashboard } from "@/components/ShareDashboard";
import { UniversalDashboard } from "@/components/UniversalDashboard";
import { UsageDashboard } from "@/components/UsageDashboard";

type View = "providers" | "shares" | "usage" | "settings" | "universal" | "accounts";

const views: Array<{ id: View; labelKey: string; icon: ReactNode }> = [
  { id: "providers", labelKey: "server.nav.providers", icon: <Boxes size={17} /> },
  { id: "shares", labelKey: "server.nav.shares", icon: <Share2 size={17} /> },
  { id: "usage", labelKey: "server.nav.usage", icon: <BarChart3 size={17} /> },
  { id: "settings", labelKey: "server.nav.settings", icon: <Settings size={17} /> },
  { id: "universal", labelKey: "server.nav.universal", icon: <Layers3 size={17} /> },
  { id: "accounts", labelKey: "server.nav.accounts", icon: <KeyRound size={17} /> },
];

function App() {
  const { t } = useI18n();
  const [context, setContext] = useState<WebRuntimeContext | null>(null);
  const [view, setView] = useState<View>("providers");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

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
        return <ProviderDashboard />;
      case "shares":
        return <ShareDashboard />;
      case "usage":
        return <UsageDashboard />;
      case "settings":
        return <SettingsDashboard />;
      case "universal":
        return <UniversalDashboard />;
      case "accounts":
        return <AccountsDashboard />;
    }
  }, [context, loading, loadData, t, view]);

  const activeViewLabel = t(views.find((item) => item.id === view)?.labelKey || "server.common.server");

  return (
    <main className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <Cloud size={21} />
          <div>
            <div className="brand-name">cc-switch-server</div>
            <div className="brand-subtitle">{t("server.brand.subtitle")}</div>
          </div>
        </div>
        <nav className="nav-list">
          {views.map((item) => (
            <button
              key={item.id}
              className={item.id === view ? "nav-item active" : "nav-item"}
              type="button"
              onClick={() => setView(item.id)}
              disabled={context?.mode !== "local-admin"}
            >
              {item.icon}
              <span>{t(item.labelKey)}</span>
            </button>
          ))}
        </nav>
        <div className="runtime-block">
          <StatusPill tone={context?.status === "authenticated" ? "success" : "warning"}>
            {context?.status || "loading"}
          </StatusPill>
          <div className="runtime-line">{context?.router?.clientSubdomain || t("server.common.local")}</div>
        </div>
      </aside>

      <section className="workspace">
        <header className="topbar">
          <div>
            <h1>{activeViewLabel}</h1>
            <p>{context?.auth?.ownerEmail || context?.status || t("server.common.runtime")}</p>
          </div>
          <div className="topbar-actions">
            {error && <span className="error-text">{error}</span>}
            {readToken() && (
              <button
                className="icon-button"
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
              className="icon-button"
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
          </div>
        </header>
        {content}
      </section>
    </main>
  );
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
