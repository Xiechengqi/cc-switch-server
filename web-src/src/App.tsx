import { useCallback, useEffect, useRef, useState } from "react";
import { Network } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import { LeaseWorkspace } from "@/components/LeaseWorkspace";
import ServerApp from "@/ServerApp";
import { ClientWebLoginPage } from "@/components/ClientWebLoginPage";
import { LoginPanel } from "@/components/LoginPanel";
import { Button } from "@/components/ui/button";
import { isRemoteWebMode } from "@/lib/api/auth";
import { getWebRuntimeContext, WebRuntimeContext, writeToken } from "@/lib/runtime";
import { clearRouterSessionTokens, SERVER_AUTH_EXPIRED_EVENT } from "@/lib/routerAuth";
import { initializeEmbeddedAuth, isTenantHost } from "@/lib/embeddedAuth";
import { WorkspaceCommands } from "@/lib/workspaceCommands";

export default function App() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [context, setContext] = useState<WebRuntimeContext | null>(null);
  const [error, setError] = useState("");
  const generation = useRef(0);
  const refreshContext = useCallback(async (background = false) => {
    const current = ++generation.current;
    try {
      await initializeEmbeddedAuth();
      const next = await getWebRuntimeContext();
      if (current === generation.current) {
        setContext(next);
        setError("");
      }
      return next;
    } catch (cause) {
      if (!background && current === generation.current)
        setError(cause instanceof Error ? cause.message : String(cause));
      throw cause;
    }
  }, []);

  useEffect(() => {
    void refreshContext().catch(() => {});
    const expire = (e: Event) => {
      generation.current++;
      queryClient.clear();
      setError("");
      setContext({
        mode: "client-login",
        status: (e as CustomEvent).detail === "lease_ended" ? "lease_ended" : "auth-required",
      });
    };
    window.addEventListener(SERVER_AUTH_EXPIRED_EVENT, expire);
    return () => {
      generation.current++;
      window.removeEventListener(SERVER_AUTH_EXPIRED_EVENT, expire);
    };
  }, [queryClient, refreshContext]);
  useEffect(() => {
    if (context?.mode !== "local-admin") return;
    const refresh = () => {
      if (!context.marketListed && document.visibilityState === "hidden") return;
      void refreshContext(true).catch(() => {});
    };
    const timer = window.setInterval(refresh, context.marketListed ? 25_000 : 60_000);
    window.addEventListener("focus", refresh);
    return () => {
      clearInterval(timer);
      window.removeEventListener("focus", refresh);
    };
  }, [context?.mode, context?.marketListed, refreshContext]);
  const signOut = () => {
    writeToken(null);
    clearRouterSessionTokens();
    window.dispatchEvent(new CustomEvent(SERVER_AUTH_EXPIRED_EVENT));
  };
  if (error)
    return (
      <EmptyState title={t("common.error")} value={error}>
        <Button onClick={() => void refreshContext().catch(() => {})}>{t("lease.retry")}</Button>
      </EmptyState>
    );
  if (!context) return <EmptyState title={t("common.loading")} value={t("server.common.runtime")} />;
  if (context.mode !== "local-admin") {
    if (isTenantHost() || context.workspaceRole === "tenant") {
      const ended = context.status === "lease_ended";
      return (
        <EmptyState
          title={t(ended ? "lease.ended" : "lease.signIn")}
          value={t(ended ? "lease.endedHelp" : "lease.routerHelp")}
        >
          <Button asChild variant="outline">
            <a
              href={context.openUrl || "/"}
              target={window.parent === window ? "_self" : "_blank"}
              rel="noopener noreferrer"
            >
              {t("lease.reenter")}
            </a>
          </Button>
        </EmptyState>
      );
    }
    return isRemoteWebMode() ? (
      <ClientWebLoginPage onAuthenticated={refreshContext} />
    ) : (
      <LoginPanel context={context} onAuthenticated={refreshContext} />
    );
  }
  if (context.workspaceRole === "assistance")
    return (
      <LeaseWorkspace
        key={context.leaseId}
        role="assistance"
        leaseId={context.leaseId}
        expiresAt={context.leaseExpiresAt}
        localOwner={context.localOwner}
        onSignOut={signOut}
      />
    );
  const tenant = context.workspaceRole === "tenant";
  const app = (
    <ServerApp
      onSignOut={tenant ? signOut : () => void refreshContext().catch(() => {})}
      workspaceOnly={tenant}
      fillParent={tenant || context.marketListed}
      enableWebTerminal={!tenant && context.runtime?.enableWebTerminal !== false}
    />
  );
  if (tenant)
    return (
      <WorkspaceCommands.Provider value={new Set(context.commands?.map((c) => c.name))}>
        <LeaseWorkspace
          key={context.leaseId}
          role="tenant"
          leaseId={context.leaseId}
          expiresAt={context.leaseExpiresAt}
          onSignOut={signOut}
        >
          {app}
        </LeaseWorkspace>
      </WorkspaceCommands.Provider>
    );
  if (!context.marketListed) return app;
  return (
    <div className="flex h-dvh min-h-0 flex-col">
      <aside className="shrink-0 border-b bg-muted px-4 py-2 text-sm">{t("lease.listed")}</aside>
      {app}
    </div>
  );
}
function EmptyState({
  title,
  value,
  children,
}: {
  title: string;
  value: string;
  children?: React.ReactNode;
}) {
  return (
    <main className="panel">
      <div className="empty-state">
        <Network size={24} />
        <h1 className="font-semibold">{title}</h1>
        <span>{value}</span>
        {children}
      </div>
    </main>
  );
}
