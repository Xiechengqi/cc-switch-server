import { useCallback, useEffect, useState } from "react";
import { Network } from "lucide-react";

import ServerDesktopApp from "@/ServerDesktopApp";
import { ClientWebLoginPage } from "@/components/ClientWebLoginPage";
import { LoginPanel } from "@/components/LoginPanel";
import { isRemoteWebMode } from "@/lib/api/auth";
import { getWebRuntimeContext, WebRuntimeContext } from "@/lib/runtime";
import { useI18n } from "@/lib/i18n";

function App() {
  const { t } = useI18n();
  const [context, setContext] = useState<WebRuntimeContext | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refreshContext = useCallback(async () => {
    const next = await getWebRuntimeContext();
    setContext(next);
    return next;
  }, []);

  useEffect(() => {
    let active = true;
    setLoading(true);
    refreshContext()
      .catch((reason) => {
        if (active) setError(errorMessage(reason));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [refreshContext]);

  if (!context || loading) {
    return <EmptyState title={t("common.loading")} value={t("server.common.runtime")} />;
  }

  if (context.mode !== "local-admin") {
    if (isRemoteWebMode()) {
      return <ClientWebLoginPage onAuthenticated={refreshContext} />;
    }
    return <LoginPanel context={context} onAuthenticated={refreshContext} />;
  }

  if (error) {
    return <EmptyState title={t("common.error", { defaultValue: "Error" })} value={error} />;
  }

  return <ServerDesktopApp onSignOut={refreshContext} />;
}

function EmptyState({ title, value }: { title: string; value: string }) {
  return (
    <div className="panel">
      <div className="empty-state">
        <Network size={24} />
        <strong>{title}</strong>
        <span>{value}</span>
      </div>
    </div>
  );
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export default App;
