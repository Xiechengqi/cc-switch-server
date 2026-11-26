import { useEffect, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { apiFetch, jsonFetch } from "@/lib/runtime";
import { WorkspaceChat } from "./WorkspaceChat";

const base = "/web-api/market";

export function LeaseWorkspace({
  leaseId = "",
  expiresAt,
  onSignOut,
  children,
}: {
  leaseId?: string;
  expiresAt?: number;
  onSignOut?: () => void;
  children?: ReactNode;
}) {
  const { t, i18n } = useTranslation();
  const [backups, setBackups] = useState<{
    backups: string[];
    restorable: boolean;
  }>({ backups: [], restorable: false });
  const [snapshot, setSnapshot] = useState("");
  const [backupDone, setBackupDone] = useState(false);
  const [chat, setChat] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [revision, reload] = useState(0);
  const [now, setNow] = useState(Date.now);
  const date = (ms: number) => new Date(ms).toLocaleString(i18n.language);
  useEffect(() => {
    const ctl = new AbortController();
    void jsonFetch<typeof backups>(base + "/backups", { signal: ctl.signal })
      .then((value) => {
        if (!ctl.signal.aborted) setBackups(value);
      })
      .catch((cause) => {
        if (!ctl.signal.aborted) setError(String(cause));
      });
    return () => ctl.abort();
  }, [leaseId, revision]);
  useEffect(() => {
    if (!expiresAt) return;
    const timer = setInterval(() => setNow(Date.now()), 15_000);
    return () => clearInterval(timer);
  }, [expiresAt]);
  async function perform(action: () => Promise<void>) {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await action();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }
  const remaining =
    expiresAt == null ? null : Math.max(0, expiresAt - Math.floor(now / 1000));
  const expiry =
    remaining == null
      ? ""
      : remaining > 86400
        ? t("lease.daysLeft", { count: Math.ceil(remaining / 86400) })
        : remaining > 3600
          ? t("lease.hoursLeft", { count: Math.ceil(remaining / 3600) })
          : t("lease.minutesLeft", { count: Math.ceil(remaining / 60) });
  return (
    <main className="flex h-dvh min-h-0 flex-col">
      <section
        aria-label={t("lease.title")}
        className="max-h-[45dvh] shrink-0 overflow-auto border-b px-3 py-1.5 text-sm"
      >
        <header className="flex flex-wrap items-center gap-3">
          <h1 className="font-semibold">{t("lease.tenant")}</h1>
          {expiresAt && (
            <time dateTime={new Date(expiresAt * 1000).toISOString()}>
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
        <details>
          <summary className="cursor-pointer">{t("lease.tools")}</summary>
          <div className="flex flex-wrap items-center gap-2 py-2">
            <Button
              size="sm"
              disabled={busy}
              onClick={() =>
                void perform(async () => {
                  setBackupDone(false);
                  const r = await apiFetch(base + "/backups", {
                    method: "POST",
                  });
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
                      await jsonFetch(
                        base +
                          "/backups/" +
                          encodeURIComponent(snapshot) +
                          "/restore",
                        {
                          method: "POST",
                        },
                      );
                      location.reload();
                    })
                  }
                >
                  {t("lease.restore")}
                </Button>
              </>
            )}
            <Button
              size="sm"
              variant="outline"
              aria-expanded={chat}
              onClick={() => setChat(!chat)}
            >
              {t("lease.chat")}
            </Button>
          </div>
          {chat && <WorkspaceChat zh={i18n.language.startsWith("zh")} />}
        </details>
      </section>
      {children}
    </main>
  );
}
