import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { apiFetch, jsonFetch } from "@/lib/runtime";
import { setMaintenanceVisit, setWorkspaceOwnerProof } from "@/lib/routerAuth";
import { WorkspaceChat } from "./WorkspaceChat";

const TerminalPage = lazy(() => import("@/components/terminal/TerminalPage"));
const base = "/web-api/market";
type Maintenance = {
  terminalEnabled: boolean;
  terminalAvailable: boolean;
  upgradePolicy: {
    autoUpgradeEnabled: boolean;
    autoUpgradeCheckIntervalMinutes: number;
    delegateUpgradeToRouterOwner: boolean;
  };
};
type View = {
  leaseId: string;
  expiresAt: number;
  fenced: boolean;
  cleaning: boolean;
  pendingEvents: number;
  waitingOnShares?: boolean;
  shareCount: number;
  version: string;
  hardware: { arch: string; cpu: number | null; memoryBytes: number | null; diskBytes: number | null } | null;
  connectivity: { registered: boolean; lastHeartbeatMs: number | null };
  maintenance: Maintenance | null;
  providers: { app: string; providerId: string; status: string; requests: number; failures: number }[];
  audit: { sequence: number; time: number; status: number | null }[];
};
type Proof = { accessToken: string; refreshToken: string; expiresAt: string };
type Action = "restart" | "upgrade" | "rollback";
const commands: Record<Action, string> = {
  restart: "restart_server_service",
  upgrade: "start_admin_upgrade",
  rollback: "rollback_server_service",
};
function visitId(lease: string) {
  const key = "cc-workspace-visit:" + lease;
  const fresh = Array.from(crypto.getRandomValues(new Uint8Array(16)), (n) =>
    n.toString(16).padStart(2, "0"),
  ).join("");
  try {
    const stored = sessionStorage.getItem(key);
    if (stored && /^[0-9a-f]{32}$/.test(stored)) return stored;
    sessionStorage.setItem(key, fresh);
  } catch {
    /* Private browsing can disable tab storage. */
  }
  return fresh;
}
const post = (body: unknown) => ({
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify(body),
});
const needsProof = (cause: unknown) => (cause as { code?: string })?.code === "owner_proof_required";

export function LeaseWorkspace({
  role,
  leaseId = "",
  expiresAt,
  localOwner = false,
  onSignOut,
  children,
}: {
  role: "tenant" | "assistance";
  leaseId?: string;
  expiresAt?: number;
  localOwner?: boolean;
  onSignOut?: () => void;
  children?: React.ReactNode;
}) {
  const { t, i18n } = useTranslation();
  const tenant = role === "tenant";
  const [visit] = useState(() => visitId(leaseId));
  const [view, setView] = useState<View | null>(null);
  const [backups, setBackups] = useState<{ backups: string[]; restorable: boolean }>({
    backups: [],
    restorable: false,
  });
  const [snapshot, setSnapshot] = useState("");
  const [backupDone, setBackupDone] = useState(false);
  const [chat, setChat] = useState(false);
  const [password, setPassword] = useState("");
  const [authorized, setAuthorized] = useState(false);
  const [proof, setProof] = useState<Proof | null>(null);
  const [terminal, setTerminal] = useState(
    () => new URLSearchParams(location.search).get("view") === "terminal",
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [revision, reload] = useState(0);
  const [now, setNow] = useState(Date.now);
  const [job, setJob] = useState<{ action: Action; id: string; start: number } | null>(null);
  const instance = useRef("");
  const alive = useRef(true);
  const date = (ms: number) => new Date(ms).toLocaleString(i18n.language);
  const verify = async (signal?: AbortSignal) => {
    const result = await jsonFetch<{ authorized: boolean; instanceId: string }>(
      base + "/maintenance/status",
      { signal },
    );
    if (!result.authorized) throw new Error(t("lease.ownerProof"));
    instance.current = result.instanceId;
  };
  function lock() {
    setWorkspaceOwnerProof(null);
    setProof(null);
    setAuthorized(false);
    setTerminal(false);
    setJob(null);
  }
  useEffect(() => {
    alive.current = true;
    if (!tenant) setMaintenanceVisit(visit);
    return () => {
      alive.current = false;
      setMaintenanceVisit(null);
      setWorkspaceOwnerProof(null);
    };
  }, [tenant, visit]);
  useEffect(() => {
    const ctl = new AbortController();
    const opts = { signal: ctl.signal, headers: { "x-cc-switch-workspace-visit": visit } };
    let timer: ReturnType<typeof setTimeout>;
    const read = async () => {
      try {
        if (tenant) {
          const value = await jsonFetch<typeof backups>(base + "/backups", opts);
          if (!ctl.signal.aborted) setBackups(value);
        } else {
          const value = await jsonFetch<View>(base + "/workspace", opts);
          if (!ctl.signal.aborted) setView(value);
        }
      } catch (cause) {
        if (!ctl.signal.aborted) setError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        if (!tenant && !ctl.signal.aborted) timer = setTimeout(read, 25_000);
      }
    };
    void read();
    return () => {
      ctl.abort();
      clearTimeout(timer);
    };
  }, [role, revision, visit, tenant]);
  useEffect(() => {
    if (!localOwner || tenant) return;
    const ctl = new AbortController();
    void verify(ctl.signal)
      .then(() => {
        if (!ctl.signal.aborted) setAuthorized(true);
      })
      .catch((cause) => {
        if (!ctl.signal.aborted) setError(String(cause));
      });
    return () => ctl.abort();
  }, [localOwner, tenant]);
  useEffect(() => {
    if (!proof) return;
    const ctl = new AbortController();
    const expires = Date.parse(proof.expiresAt);
    const refresh = async () => {
      try {
        const next = await jsonFetch<Proof>("/web-api/auth/password/refresh", {
          ...post({ refreshToken: proof.refreshToken }),
          signal: ctl.signal,
        });
        if (ctl.signal.aborted) return;
        setWorkspaceOwnerProof(next.accessToken);
        await verify(ctl.signal);
        if (!ctl.signal.aborted) setProof(next);
      } catch {
        if (!ctl.signal.aborted) {
          lock();
          setError(t("lease.ownerProof"));
        }
      }
    };
    const timer = setTimeout(refresh, Math.max(0, expires - Date.now() - 30_000));
    const deadline = setTimeout(
      () => {
        ctl.abort();
        lock();
      },
      Math.max(0, expires - Date.now()),
    );
    return () => {
      ctl.abort();
      clearTimeout(timer);
      clearTimeout(deadline);
    };
  }, [proof]);
  useEffect(() => {
    if (!expiresAt) return;
    const timer = setInterval(() => setNow(Date.now()), 15_000);
    return () => clearInterval(timer);
  }, [expiresAt]);
  useEffect(() => {
    if (!job || !authorized) return;
    const ctl = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        if (job.action === "upgrade") {
          const v = await jsonFetch<{ status: string }>(
            base + "/maintenance/upgrade-status?taskId=" + encodeURIComponent(job.id),
            { signal: ctl.signal },
          );
          if (ctl.signal.aborted) return;
          if (v.status === "success" || v.status === "failed") {
            setMessage(t(v.status === "success" ? "lease.done" : "lease.failed"));
            setJob(null);
            return;
          }
        } else {
          await verify(ctl.signal);
          if (ctl.signal.aborted) return;
          if (instance.current !== job.id) {
            setMessage(t("lease.done"));
            setJob(null);
            return;
          }
        }
      } catch (cause) {
        if (!ctl.signal.aborted && needsProof(cause)) {
          lock();
          setError(t("lease.ownerProof"));
          return;
        }
      }
      if (ctl.signal.aborted) return;
      if (Date.now() - job.start > 300_000) {
        setError(t("lease.jobTimeout"));
        setJob(null);
        return;
      }
      timer = setTimeout(poll, 2000);
    };
    timer = setTimeout(poll, 1500);
    return () => {
      ctl.abort();
      clearTimeout(timer);
    };
  }, [job, authorized, t]);
  async function perform(fn: () => Promise<void>) {
    if (busy) return;
    setBusy(true);
    setError("");
    setMessage("");
    try {
      await fn();
    } catch (cause) {
      if (needsProof(cause)) {
        lock();
        setError(t("lease.ownerProof"));
      } else setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (alive.current) setBusy(false);
    }
  }
  const remaining = expiresAt == null ? null : Math.max(0, expiresAt - Math.floor(now / 1000));
  const expiry =
    remaining == null
      ? ""
      : remaining > 86400
        ? t("lease.daysLeft", { count: Math.ceil(remaining / 86400) })
        : remaining > 3600
          ? t("lease.hoursLeft", { count: Math.ceil(remaining / 3600) })
          : t("lease.minutesLeft", { count: Math.ceil(remaining / 60) });
  const gib = (n: number | null) =>
    n == null ? "—" : (n / 2 ** 30).toLocaleString(i18n.language, { maximumFractionDigits: 1 }) + " GiB";
  return (
    <main className={tenant ? "flex h-dvh min-h-0 flex-col" : "mx-auto h-dvh w-full max-w-5xl overflow-y-auto p-4"}>
      <section
        aria-label={t("lease.title")}
        className={cn(
          "[&_button]:min-h-11 [&_summary]:min-h-11 [&_summary]:cursor-pointer [&_summary]:py-3 [&_label]:min-h-11 [&_select]:min-h-11",
          "[&_:is(button,input,select,summary):focus-visible]:ring-2 [&_:is(button,input,select,summary):focus-visible]:ring-ring [&_:is(button,input,select,summary):focus-visible]:ring-offset-2",
          tenant
            ? "max-h-[45dvh] shrink-0 overflow-auto border-b px-3 py-1.5 text-sm"
            : "grid gap-3 rounded-lg border p-4",
        )}
      >
        <header className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <h1 className="font-semibold">{t(tenant ? "lease.tenant" : "lease.assistance")}</h1>
          <span className="break-all font-mono text-xs">{leaseId || view?.leaseId}</span>
          {expiresAt && (
            <time
              dateTime={new Date(expiresAt * 1000).toISOString()}
              aria-live={remaining !== null && remaining <= 86400 ? "polite" : "off"}
              className={remaining === null || remaining > 86400 ? undefined : remaining <= 3600 ? "font-semibold text-destructive" : "text-amber-700"}
            >
              {date(expiresAt * 1000)} · {expiry}
            </time>
          )}
          {onSignOut && (
            <Button variant="ghost" size="sm" onClick={onSignOut}>
              {t("lease.signOut")}
            </Button>
          )}
        </header>
        {error && (
          <p role="alert" className="text-destructive">
            {error}
          </p>
        )}
        {message && <p role="status">{message}</p>}
        {tenant ? (
          <>
            <p className="text-xs text-muted-foreground">{t("lease.root")}</p>
            <details>
              <summary className="cursor-pointer">{t("lease.tools")}</summary>
              <div className="flex flex-wrap items-center gap-2 py-2">
                <Button
                  size="sm"
                  disabled={busy}
                  onClick={() =>
                    void perform(async () => {
                      setBackupDone(false);
                      const r = await apiFetch(base + "/backups", { method: "POST" });
                      if (!r.ok) throw new Error(t("lease.backupFailed"));
                      const url = URL.createObjectURL(await r.blob());
                      const a = document.createElement("a");
                      a.href = url;
                      a.download = "workspace-" + leaseId + ".zip";
                      a.click();
                      setTimeout(() => URL.revokeObjectURL(url), 1000);
                      setBackupDone(true);
                      reload((n) => n + 1);
                    })
                  }
                >
                  {t("lease.export")}
                </Button>
                {backupDone && <span role="status">{t("lease.downloaded")}</span>}
                {backups.restorable && backups.backups.length > 0 && (
                  <>
                    <select
                      aria-label={t("lease.snapshot")}
                      value={snapshot}
                      onChange={(e) => setSnapshot(e.target.value)}
                    >
                      <option value="">—</option>
                      {backups.backups.map((id) => (
                        <option key={id}>{id}</option>
                      ))}
                    </select>
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={!snapshot || busy}
                      onClick={() =>
                        void perform(async () => {
                          if (!window.confirm(t("lease.restoreConfirm"))) return;
                          await jsonFetch(base + "/backups/" + encodeURIComponent(snapshot) + "/restore", {
                            method: "POST",
                          });
                          location.reload();
                        })
                      }
                    >
                      {t("lease.restore")}
                    </Button>
                  </>
                )}
                <Button size="sm" variant="outline" aria-expanded={chat} onClick={() => setChat(!chat)}>
                  {t("lease.chat")}
                </Button>
              </div>
              {chat && <WorkspaceChat zh={i18n.language.startsWith("zh")} />}
            </details>
          </>
        ) : (
          <>
            {view && (
              <>
                <p>
                  {t("lease.version")} {view.version} · {view.shareCount} Share ·{" "}
                  {t(view.cleaning ? "lease.cleaning" : view.fenced ? "lease.fenced" : "lease.active")} ·{" "}
                  {t("lease.pending", { count: view.pendingEvents || 0 })}
                </p>
                {view.waitingOnShares && <p role="status">{t("lease.awaitingShares")}</p>}
                <p>
                  {t(view.connectivity.registered ? "lease.registered" : "lease.offline")}
                  {view.connectivity.lastHeartbeatMs != null && (
                    <>
                      {" "}
                      · {t("lease.heartbeat")} {date(view.connectivity.lastHeartbeatMs)}
                    </>
                  )}
                </p>
                {view.hardware && (
                  <p>
                    {view.hardware.arch} · {view.hardware.cpu ?? "—"} vCPU · {t("lease.memory")}{" "}
                    {gib(view.hardware.memoryBytes)} · {t("lease.disk")} {gib(view.hardware.diskBytes)}
                  </p>
                )}
                <details>
                  <summary>{t("lease.health")}</summary>
                  <table>
                    <thead>
                      <tr>
                        <th>App</th>
                        <th>Provider</th>
                        <th>{t("lease.status")}</th>
                        <th>{t("lease.failures")}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {view.providers.map((p) => (
                        <tr key={p.app + p.providerId}>
                          <td>{p.app}</td>
                          <td>{p.providerId}</td>
                          <td>{p.status}</td>
                          <td>
                            {p.failures} / {p.requests}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </details>
                <details>
                  <summary>{t("lease.audit")}</summary>
                  {view.audit.map((e) => (
                    <p key={e.sequence}>
                      {date(e.time)} · HTTP {e.status ?? "—"}
                    </p>
                  ))}
                </details>
              </>
            )}
            {!authorized ? (
              <form
                className="flex flex-wrap items-end gap-2"
                onSubmit={(e) => {
                  e.preventDefault();
                  void perform(async () => {
                    const next = await jsonFetch<Proof>("/web-api/auth/password/login", post({ password }));
                    setPassword("");
                    if (!alive.current) return;
                    if (!next.accessToken || !next.refreshToken || !(Date.parse(next.expiresAt) > Date.now()))
                      throw new Error(t("lease.ownerProof"));
                    setWorkspaceOwnerProof(next.accessToken);
                    try {
                      await verify();
                    } catch (cause) {
                      setWorkspaceOwnerProof(null);
                      throw cause;
                    }
                    if (alive.current) {
                      setProof(next);
                      setAuthorized(true);
                    }
                  });
                }}
              >
                <label>
                  {t("lease.ownerPassword")}{" "}
                  <Input
                    className="h-12"
                    type="password"
                    autoComplete="current-password"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                  />
                </label>
                <Button disabled={!password || busy}>{t("lease.unlock")}</Button>
              </form>
            ) : (
              <>
                <div className="flex flex-wrap gap-2">
                  <Button
                    variant="outline"
                    disabled={!view?.maintenance?.terminalAvailable}
                    aria-expanded={terminal}
                    onClick={() => setTerminal(!terminal)}
                  >
                    {t("lease.terminal")}
                  </Button>
                  {(["restart", "upgrade", "rollback"] as const).map((action) => (
                    <Button
                      key={action}
                      variant="outline"
                      disabled={busy || !!job}
                      onClick={() =>
                        void perform(async () => {
                          if (!window.confirm(t("lease.confirm_" + action))) return;
                          const v = await jsonFetch<{ taskId?: string }>(
                            "/web-api/invoke/" + commands[action],
                            post({}),
                          );
                          setJob({
                            action,
                            id: action === "upgrade" ? v.taskId || "" : instance.current,
                            start: Date.now(),
                          });
                          setMessage(t("lease.running", { action: t("lease." + action) }));
                        })
                      }
                    >
                      {t("lease." + action)}
                    </Button>
                  ))}
                </div>
                {view?.maintenance && (
                  <details>
                    <summary>{t("lease.settings")}</summary>
                    <form
                      className="grid gap-2 py-2"
                      onSubmit={(e) => {
                        e.preventDefault();
                        const data = new FormData(e.currentTarget);
                        void perform(async () => {
                          if (!window.confirm(t("lease.confirm_settings"))) return;
                          const saved = await jsonFetch<Maintenance>(
                            base + "/maintenance",
                            post({
                              terminalEnabled: data.has("terminal"),
                              upgradePolicy: {
                                autoUpgradeEnabled: data.has("automatic"),
                                delegateUpgradeToRouterOwner: data.has("delegate"),
                                autoUpgradeCheckIntervalMinutes: Number(data.get("interval")),
                              },
                            }),
                          );
                          setView((v) => v && { ...v, maintenance: saved });
                          if (!saved.terminalAvailable) setTerminal(false);
                          setMessage(t("lease.done"));
                        });
                      }}
                    >
                      <label>
                        <input
                          name="terminal"
                          type="checkbox"
                          defaultChecked={view.maintenance.terminalEnabled}
                        />{" "}
                        {t("lease.enableTerminal")}
                      </label>
                      {view.maintenance.terminalAvailable !== view.maintenance.terminalEnabled && (
                        <p>{t("lease.terminalOverride")}</p>
                      )}
                      <label>
                        <input
                          name="automatic"
                          type="checkbox"
                          defaultChecked={view.maintenance.upgradePolicy.autoUpgradeEnabled}
                        />{" "}
                        {t("lease.automatic")}
                      </label>
                      <label>
                        <input
                          name="delegate"
                          type="checkbox"
                          defaultChecked={view.maintenance.upgradePolicy.delegateUpgradeToRouterOwner}
                        />{" "}
                        {t("lease.delegate")}
                      </label>
                      <label>
                        {t("lease.interval")}{" "}
                        <Input
                          className="h-12"
                          name="interval"
                          type="number"
                          required
                          min={5}
                          max={1440}
                          defaultValue={view.maintenance.upgradePolicy.autoUpgradeCheckIntervalMinutes}
                        />
                      </label>
                      <Button className="justify-self-start" disabled={busy}>
                        {t("lease.save")}
                      </Button>
                    </form>
                  </details>
                )}
                {terminal && view?.maintenance?.terminalAvailable && (
                  <div className="h-[65dvh] min-h-72 overflow-hidden" aria-label={t("lease.terminal")}>
                    <Suspense fallback={t("common.loading")}>
                      <TerminalPage bare />
                    </Suspense>
                  </div>
                )}
              </>
            )}
          </>
        )}
      </section>
      {tenant && children}
    </main>
  );
}
