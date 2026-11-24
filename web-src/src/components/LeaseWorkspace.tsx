import { lazy, Suspense, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { apiFetch, jsonFetch } from "@/lib/runtime";
const TerminalPage = lazy(() => import("@/components/terminal/TerminalPage"));
import { WorkspaceChat } from "./WorkspaceChat";
import { setWorkspaceOwnerProof } from "@/lib/routerAuth";

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
  shareCount: number;
  version: string;
  hardware: {
    arch: string;
    cpu: number | null;
    memoryBytes: number | null;
    diskBytes: number | null;
  } | null;
  connectivity: { registered: boolean; lastHeartbeatMs: number | null };
  maintenance: Maintenance | null;
  providers: {
    app: string;
    providerId: string;
    status: string;
    requests: number;
    failures: number;
  }[];
  audit: { sequence: number; time: number; status: number | null }[];
};
export function LeaseWorkspace({
  role,
  children,
}: {
  role: "tenant" | "assistance";
  children?: React.ReactNode;
}) {
  const { i18n } = useTranslation();
  const zh = i18n.language.startsWith("zh");
  const label = (en: string, cn: string) => (zh ? cn : en);
  const [view, setView] = useState<View | null>(null);
  const [error, setError] = useState("");
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [backups, setBackups] = useState<string[]>([]);
  const [restorable, setRestorable] = useState(false);
  const [restoreId, setRestoreId] = useState("");
  const [password, setPassword] = useState("");
  const [owner, setOwner] = useState(false);
  const [terminal, setTerminal] = useState(false);
  const [chatOpen, setChatOpen] = useState(false);
  const [rev, setRev] = useState(0);
  useEffect(() => {
    const c = new AbortController();
    if (role === "assistance") {
      jsonFetch<View>("/web-api/market/workspace", { signal: c.signal })
        .then((v) => {
          if (!c.signal.aborted) setView(v);
        })
        .catch((e) => {
          if (!c.signal.aborted) setError(String(e));
        });
    } else {
      jsonFetch<{ backups: string[]; restorable: boolean }>(
        "/web-api/market/backups",
        { signal: c.signal },
      )
        .then((v) => {
          if (!c.signal.aborted) {
            setBackups(v.backups);
            setRestorable(v.restorable);
          }
        })
        .catch((e) => {
          if (!c.signal.aborted) setError(String(e));
        });
    }
    return () => c.abort();
  }, [role, rev]);
  useEffect(() => () => setWorkspaceOwnerProof(null), []);
  async function perform(fn: () => Promise<void>) {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await fn();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  const date = (n: number) => new Date(n).toLocaleString(i18n.language);
  const gib = (n: number | null) =>
    n == null
      ? "—"
      : (n / 2 ** 30).toLocaleString(i18n.language, {
          maximumFractionDigits: 1,
        }) + " GiB";
  return (
    <>
      <section
        className="panel"
        aria-label={label("Client rental", "Client 租约")}
      >
        <h2>
          {role === "tenant"
            ? label("Tenant workspace", "租户工作区")
            : label("Owner assistance · read only", "所有者协助 · 只读")}
        </h2>
        {error && <p role="alert">{error}</p>}
        {role === "tenant" ? (
          <>
            <p>
              {label(
                "The host owner has root access and can read data on disk. Owner assistance and maintenance are recorded.",
                "主机所有者拥有 root 权限，可以读取磁盘数据。卖家协助及运维操作会留痕。",
              )}
            </p>
            <button
              disabled={busy}
              onClick={() =>
                void perform(async () => {
                  const r = await apiFetch("/web-api/market/backups", {
                    method: "POST",
                  });
                  if (!r.ok) throw new Error(await r.text());
                  const url = URL.createObjectURL(await r.blob());
                  const a = document.createElement("a");
                  a.href = url;
                  a.download =
                    r.headers
                      .get("content-disposition")
                      ?.match(/filename=([a-z\d.-]+)/i)?.[1] || "workspace.zip";
                  a.click();
                  setTimeout(() => URL.revokeObjectURL(url), 1000);
                  setRev((v) => v + 1);
                })
              }
            >
              {label("Export workspace backup", "导出工作区备份")}
            </button>
            {restorable && backups.length > 0 && (
              <label>
                {label("Restore snapshot", "恢复快照")}{" "}
                <select
                  value={restoreId}
                  onChange={(e) => setRestoreId(e.target.value)}
                >
                  <option value="">—</option>
                  {backups.map((id) => (
                    <option key={id}>{id}</option>
                  ))}
                </select>
                <button
                  disabled={busy || !restoreId}
                  onClick={() =>
                    void perform(async () => {
                      if (
                        window.confirm(
                          label(
                            "Replace the current workspace with this snapshot?",
                            "用所选快照替换当前工作区？",
                          ),
                        )
                      ) {
                        await jsonFetch(
                          `/web-api/market/backups/${encodeURIComponent(restoreId)}/restore`,
                          { method: "POST" },
                        );
                        location.reload();
                      }
                    })
                  }
                >
                  {label("Restore", "恢复")}
                </button>
              </label>
            )}
            <button
              disabled={busy}
              onClick={() =>
                void perform(async () => {
                  const r = await jsonFetch<{ token: string }>(
                    "/web-api/market/token",
                    { method: "POST" },
                  );
                  setToken(r.token);
                })
              }
            >
              {label("Rotate workspace CLI token", "轮换工作区 CLI 令牌")}
            </button>
            <details onToggle={(e) => setChatOpen(e.currentTarget.open)}>
              <summary className="min-h-11 cursor-pointer py-2">
                {label("Workspace chat", "工作区聊天")}
              </summary>
              {chatOpen && <WorkspaceChat zh={zh} />}
            </details>
            {token && (
              <label>
                {label(
                  "Use this bearer token on loopback with the X-Workspace-Share header set to your Share ID.",
                  "通过回环地址调用时，请使用此 Bearer 令牌，并在 X-Workspace-Share 请求头中填写工作区的 Share ID。",
                )}{" "}
                <input
                  readOnly
                  value={token}
                  onFocus={(e) => e.target.select()}
                />
              </label>
            )}
          </>
        ) : (
          <>
            {view && (
              <>
                <p>
                  {label("Version", "版本")} {view.version} · {view.shareCount}{" "}
                  Share
                </p>
                <p>
                  {label("Router", "Router 连接")} ·{" "}
                  {view.connectivity.registered
                    ? label("Registered", "已注册")
                    : label("Unregistered", "未注册")}
                  {view.connectivity.lastHeartbeatMs != null && (
                    <>
                      {" "}
                      · {label("Last heartbeat", "最近心跳")}{" "}
                      {date(view.connectivity.lastHeartbeatMs)}
                    </>
                  )}
                </p>
                {view.hardware && (
                  <p>
                    {view.hardware.arch} · {view.hardware.cpu ?? "—"} vCPU ·{" "}
                    {label("Memory", "内存")} {gib(view.hardware.memoryBytes)} ·{" "}
                    {label("Disk", "数据盘")} {gib(view.hardware.diskBytes)}
                  </p>
                )}
                <table>
                  <caption>
                    {label("Provider health", "Provider 健康状况")}
                  </caption>
                  <thead>
                    <tr>
                      <th>App</th>
                      <th>Provider</th>
                      <th>{label("Status", "状态")}</th>
                      <th>{label("Failed / requests", "失败 / 请求数")}</th>
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
                <details>
                  <summary>
                    {label("Recent request audit", "近期请求审计")}
                  </summary>
                  {view.audit.map((e) => (
                    <p key={e.sequence}>
                      {date(e.time)} · HTTP {e.status ?? "—"}
                    </p>
                  ))}
                </details>
              </>
            )}
            {!owner ? (
              <form
                onSubmit={(e) => {
                  e.preventDefault();
                  void perform(async () => {
                    const r = await jsonFetch<{ accessToken: string }>(
                      "/web-api/auth/password/login",
                      {
                        method: "POST",
                        headers: { "Content-Type": "application/json" },
                        body: JSON.stringify({ password }),
                      },
                    );
                    setWorkspaceOwnerProof(r.accessToken);
                    setPassword("");
                    setOwner(true);
                  });
                }}
              >
                <label>
                  {label(
                    "Owner password for maintenance",
                    "运维需要所有者密码",
                  )}{" "}
                  <input
                    type="password"
                    autoComplete="current-password"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                  />
                </label>
                <button disabled={busy || !password}>
                  {label("Unlock maintenance", "解锁运维")}
                </button>
              </form>
            ) : (
              <div>
                <button
                  disabled={!view?.maintenance?.terminalAvailable}
                  onClick={() => setTerminal(!terminal)}
                >
                  {label("Owner terminal", "所有者终端")}
                </button>
                {view?.maintenance && (
                  <details>
                    <summary className="min-h-11 cursor-pointer py-2">
                      {label("Host settings", "主机设置")}
                    </summary>
                    <form
                      onSubmit={(e) => {
                        e.preventDefault();
                        const data = new FormData(e.currentTarget);
                        void perform(async () => {
                          const saved = await jsonFetch<Maintenance>(
                            "/web-api/market/maintenance",
                            {
                              method: "POST",
                              headers: { "Content-Type": "application/json" },
                              body: JSON.stringify({
                                terminalEnabled: data.has("terminal"),
                                upgradePolicy: {
                                  autoUpgradeEnabled: data.has("automatic"),
                                  delegateUpgradeToRouterOwner:
                                    data.has("delegate"),
                                  autoUpgradeCheckIntervalMinutes: Number(
                                    data.get("interval"),
                                  ),
                                },
                              }),
                            },
                          );
                          setView(
                            (previous) =>
                              previous && { ...previous, maintenance: saved },
                          );
                          if (!saved.terminalAvailable) setTerminal(false);
                        });
                      }}
                    >
                      <label>
                        <input
                          name="terminal"
                          type="checkbox"
                          defaultChecked={view.maintenance.terminalEnabled}
                        />{" "}
                        {label("Enable owner terminal", "启用所有者终端")}
                      </label>
                      {view.maintenance.terminalEnabled !==
                        view.maintenance.terminalAvailable && (
                        <p>
                          {label(
                            "The service environment overrides the terminal setting.",
                            "服务环境变量已覆盖终端设置。",
                          )}
                        </p>
                      )}
                      <label>
                        <input
                          name="automatic"
                          type="checkbox"
                          defaultChecked={
                            view.maintenance.upgradePolicy.autoUpgradeEnabled
                          }
                        />{" "}
                        {label("Enable automatic upgrades", "启用自动升级")}
                      </label>
                      <label>
                        <input
                          name="delegate"
                          type="checkbox"
                          defaultChecked={
                            view.maintenance.upgradePolicy
                              .delegateUpgradeToRouterOwner
                          }
                        />{" "}
                        {label(
                          "Allow Router owner to trigger upgrades",
                          "允许 Router 所有者触发升级",
                        )}
                      </label>
                      <label>
                        {label(
                          "Upgrade check interval (minutes)",
                          "升级检查间隔（分钟）",
                        )}{" "}
                        <input
                          name="interval"
                          className="h-12"
                          type="number"
                          required
                          min={5}
                          max={1440}
                          defaultValue={
                            view.maintenance.upgradePolicy
                              .autoUpgradeCheckIntervalMinutes
                          }
                        />
                      </label>
                      <button disabled={busy}>
                        {label("Save host settings", "保存主机设置")}
                      </button>
                    </form>
                  </details>
                )}
                {(
                  [
                    "restart_server_service",
                    "start_admin_upgrade",
                    "rollback_server_service",
                  ] as const
                ).map((cmd, i) => (
                  <button
                    key={cmd}
                    disabled={busy}
                    onClick={() =>
                      void perform(async () => {
                        if (
                          window.confirm(
                            label(
                              "Run this maintenance action and notify the tenant?",
                              "执行此运维操作并通知租户？",
                            ),
                          )
                        )
                          await jsonFetch(`/web-api/invoke/${cmd}`, {
                            method: "POST",
                            headers: { "Content-Type": "application/json" },
                            body: "{}",
                          });
                      })
                    }
                  >
                    {
                      [
                        label("Restart", "重启"),
                        label("Upgrade", "升级"),
                        label("Rollback", "回滚"),
                      ][i]
                    }
                  </button>
                ))}
              </div>
            )}
            {terminal && owner && (
              <Suspense>
                <TerminalPage bare />
              </Suspense>
            )}
          </>
        )}
      </section>
      {role === "tenant" && children}
    </>
  );
}
