import { Network, Radio, Shuffle } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { AppKind, BuildInfo } from "@/lib/server-legacy-api";
import { invokeCommand, jsonFetch } from "@/lib/runtime";
import { useI18n } from "@/lib/i18n";

interface ProxyStatusView {
  running?: boolean;
  status?: string;
  mode?: string;
  baseUrl?: string;
}

export function HeaderBuildBadge({
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

export function HeaderShareToggle({ active, onClick }: { active: boolean; onClick: () => void }) {
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

export function HeaderProxyStatus({ onClick }: { onClick: () => void }) {
  const { tx } = useI18n();
  const [status, setStatus] = useState<ProxyStatusView | null>(null);

  useEffect(() => {
    let active = true;
    invokeCommand<ProxyStatusView>("get_proxy_status")
      .then((next) => {
        if (active) setStatus(next);
      })
      .catch(() => {
        if (active) setStatus({ running: false, status: "unknown" });
      });
    return () => {
      active = false;
    };
  }, []);

  const running = status?.running !== false;
  const title = [
    tx("Proxy status"),
    status?.status,
    status?.mode,
    status?.baseUrl,
  ].filter(Boolean).join(" · ");
  return (
    <button
      className={running ? "desktop-mini-toggle active" : "desktop-mini-toggle"}
      type="button"
      onClick={onClick}
      title={title || tx("Proxy status")}
      aria-pressed={running}
    >
      <Radio size={14} />
      <span>{tx("Proxy")}</span>
    </button>
  );
}

export function HeaderFailoverToggle({ activeApp }: { activeApp: AppKind }) {
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
