import { useCallback, useEffect, useId, useState } from "react";
import {
  AlertTriangle,
  Database,
  Loader2,
  RefreshCw,
  RotateCcw,
  X,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { cn } from "@/lib/utils";
import { HttpResponseError } from "@/lib/runtime";
import { SERVER_STORAGE_SETTINGS_SECTION } from "@/lib/serverView";
import {
  serverStoreAuthorityKey,
  serverStorePhase,
  serverStoreShadowStale,
} from "@/lib/serverStore";
import {
  cancelServerStoreActivation,
  loadRuntimeVersionInfo,
  loadServerStoreAuthority,
  pollReplacementAndReload,
  requestServerStoreActivation,
  restartServerService,
  type ServerStoreAuthority,
} from "@/lib/server-legacy-api";

const p = "settings.serverStorage.";
const errorText = (reason: unknown) =>
  reason instanceof Error ? reason.message : String(reason);

/**
 * Which store the Server trusts, and the one-way switch to SQLite.
 *
 * The migration itself runs while the Server starts (it holds the data-dir
 * lock then), so "Migrate and restart" only records the request and restarts;
 * the page reloads once a new process answers, like Settings → Version.
 * Linkable as `?view=settings&section=server-storage`.
 */
export function ServerStorageSettings() {
  const { t, i18n } = useTranslation();
  const titleId = useId();
  const [store, setStore] = useState<ServerStoreAuthority | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState<"migrate" | "restart" | "cancel" | null>(
    null,
  );
  const [restarting, setRestarting] = useState(false);
  const [confirm, setConfirm] = useState<"migrate" | "restart" | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      setStore(await loadServerStoreAuthority());
      setLoadError(null);
    } catch (reason) {
      setLoadError(errorText(reason));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const formatTime = (ms: number | null | undefined) => {
    if (ms == null || !Number.isFinite(ms)) return "—";
    try {
      return new Date(ms).toLocaleString(i18n.language || undefined);
    } catch {
      return new Date(ms).toLocaleString();
    }
  };
  const formatCount = (value: number) => {
    try {
      return new Intl.NumberFormat(i18n.language || undefined).format(value);
    } catch {
      return String(value);
    }
  };

  /** The running process, so its replacement can be recognised. */
  const currentProcess = async () => {
    const runtime = await loadRuntimeVersionInfo();
    if (!runtime.processInstanceId) {
      throw new Error(t("settings.serverVersion.restartNotObserved"));
    }
    return {
      processId: runtime.processId,
      processInstanceId: runtime.processInstanceId,
    };
  };

  /** Reloads the page once a new process answers; otherwise says so. */
  const awaitReplacement = async (previous: {
    processId: number;
    processInstanceId: string;
  }) => {
    setRestarting(true);
    const replaced = await pollReplacementAndReload(previous);
    if (!replaced) {
      toast.error(t("settings.serverVersion.restartNotObserved"));
      setRestarting(false);
      setBusy(null);
      void refresh();
    }
  };

  const handleMigrate = async () => {
    setConfirm(null);
    setBusy("migrate");
    try {
      const previous = await currentProcess();
      const result = await requestServerStoreActivation({ restart: true });
      setStore((current) =>
        current
          ? { ...current, activationRequest: result.activationRequest }
          : current,
      );
      if (result.restartError || !result.restartOperationId) {
        // The request stands; it runs whenever the Server next starts.
        toast.error(
          result.restartError
            ? t(p + "restartFailed", { error: result.restartError })
            : t(p + "restartNotScheduled"),
        );
        setBusy(null);
        void refresh();
        return;
      }
      toast.success(t(p + "restarting"));
      await awaitReplacement(previous);
    } catch (reason) {
      if (reason instanceof HttpResponseError && reason.status === 409) {
        toast.info(t(p + "alreadyCommitted"));
      } else {
        toast.error(errorText(reason));
      }
      setBusy(null);
      void refresh();
    }
  };

  const handleRestart = async () => {
    setConfirm(null);
    setBusy("restart");
    try {
      const previous = await currentProcess();
      await restartServerService();
      toast.success(t("settings.serverVersion.restartScheduled"));
      await awaitReplacement(previous);
    } catch (reason) {
      toast.error(errorText(reason));
      setBusy(null);
    }
  };

  const handleCancel = async () => {
    setBusy("cancel");
    try {
      await cancelServerStoreActivation();
      setStore((current) =>
        current ? { ...current, activationRequest: null } : current,
      );
      toast.success(t(p + "cancelled"));
    } catch (reason) {
      toast.error(errorText(reason));
    } finally {
      setBusy(null);
      void refresh();
    }
  };

  const phase = store ? serverStorePhase(store) : null;
  const request = store?.activationRequest ?? null;
  const locked = busy !== null || restarting;
  const subtitle =
    loading && !store
      ? t("common.loading")
      : loadError && !store
        ? t(p + "loadFailed")
        : store
          ? t(serverStoreAuthorityKey(store.authority), {
              value: store.authority,
            })
          : "";
  const dotClass =
    phase === "committed"
      ? "bg-emerald-500"
      : phase === "failed"
        ? "bg-destructive"
        : phase
          ? "bg-amber-500"
          : "bg-muted-foreground/40";

  const spinnerOr = (action: typeof busy, Icon: typeof RotateCcw) =>
    busy === action ? (
      <Loader2 className="mr-2 h-4 w-4 animate-spin" />
    ) : (
      <Icon className="mr-2 h-4 w-4" />
    );

  return (
    <>
      <section
        id={SERVER_STORAGE_SETTINGS_SECTION}
        tabIndex={-1}
        aria-labelledby={titleId}
        className="scroll-mt-4 rounded-xl border border-border bg-card/50 outline-none transition-colors"
      >
        <div className="flex flex-col gap-3 p-4 sm:flex-row sm:items-center sm:justify-between sm:gap-4">
          <div className="flex min-w-0 flex-1 items-center gap-3">
            <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-background ring-1 ring-border">
              <Database className="h-4 w-4 text-sky-500" aria-hidden="true" />
            </div>
            <div className="min-w-0 space-y-1">
              <h3 id={titleId} className="text-sm font-medium leading-none">
                {t(p + "title")}
              </h3>
              <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
                <span
                  aria-hidden="true"
                  className={cn("h-1.5 w-1.5 shrink-0 rounded-full", dotClass)}
                />
                <span className="min-w-0 truncate">{subtitle}</span>
              </p>
            </div>
          </div>

          <div className="flex w-full flex-col gap-2 sm:w-auto sm:shrink-0 sm:flex-row sm:flex-wrap sm:items-center sm:justify-end">
            {loadError && !store ? (
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-9"
                disabled={loading}
                onClick={() => void refresh()}
              >
                {loading ? (
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                ) : (
                  <RefreshCw className="mr-2 h-4 w-4" />
                )}
                {t("common.retry")}
              </Button>
            ) : null}
            {phase === "pending" || phase === "failed" ? (
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-9"
                disabled={locked}
                onClick={() => void handleCancel()}
              >
                {spinnerOr("cancel", X)}
                {t(p + "cancel")}
              </Button>
            ) : null}
            {phase === "pending" || phase === "migrating" ? (
              <Button
                type="button"
                size="sm"
                className="h-9"
                disabled={locked}
                onClick={() => setConfirm("restart")}
              >
                {spinnerOr("restart", RotateCcw)}
                {t(p + "restartNow")}
              </Button>
            ) : null}
            {phase === "available" || phase === "failed" ? (
              <Button
                type="button"
                size="sm"
                className="h-9"
                disabled={locked}
                onClick={() => setConfirm("migrate")}
              >
                {spinnerOr("migrate", Database)}
                {phase === "failed" ? t(p + "retry") : t(p + "migrate")}
              </Button>
            ) : null}
          </div>
        </div>

        {store ? (
          <div className="space-y-2 border-t border-border/50 px-4 pb-4 pt-3 text-xs leading-5 text-muted-foreground">
            {phase === "committed" ? (
              <dl className="grid gap-x-3 gap-y-1 sm:grid-cols-[auto_1fr]">
                <dt>{t(p + "committedAt")}</dt>
                <dd className="font-mono text-foreground">
                  {formatTime(store.committedAtMs)}
                </dd>
                {store.legacyBackupDir ? (
                  <>
                    <dt>{t(p + "backupDir")}</dt>
                    <dd className="break-all font-mono text-foreground">
                      {store.legacyBackupDir}
                    </dd>
                  </>
                ) : null}
              </dl>
            ) : (
              <>
                <p>{t(p + "why")}</p>
                <p>{t(p + "what")}</p>
                <p className="font-mono">
                  {t(p + "counts", {
                    providers: formatCount(store.counts.providers),
                    accounts: formatCount(store.counts.accounts),
                    shares: formatCount(store.counts.shares),
                    usageRecords: formatCount(store.counts.usageRecords),
                  })}
                </p>
                {serverStoreShadowStale(store) ? (
                  <p className="text-amber-600 dark:text-amber-400">
                    {t(p + "shadowStale")}
                  </p>
                ) : null}
                {store.shadowError ? (
                  <p className="break-words">
                    {t(p + "shadowError", { error: store.shadowError })}
                  </p>
                ) : null}
                {phase === "migrating" ? (
                  <p className="text-foreground" role="status">
                    {t(p + "migrating")}
                  </p>
                ) : null}
                {phase === "pending" && request ? (
                  <p className="text-foreground" role="status">
                    {t(p + "pending", {
                      time: formatTime(request.requestedAtMs),
                      by: request.requestedBy || "—",
                    })}
                  </p>
                ) : null}
                {phase === "failed" && request ? (
                  <div
                    className="space-y-1 rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-destructive"
                    role="alert"
                  >
                    <p className="flex items-start gap-1.5 font-medium">
                      <AlertTriangle
                        className="mt-0.5 h-3.5 w-3.5 shrink-0"
                        aria-hidden="true"
                      />
                      {t(p + "failed", {
                        time: formatTime(
                          request.failedAtMs ?? request.requestedAtMs,
                        ),
                      })}
                    </p>
                    {request.error ? (
                      <pre className="whitespace-pre-wrap break-words font-mono text-[11px] leading-relaxed">
                        {request.error}
                      </pre>
                    ) : null}
                  </div>
                ) : null}
              </>
            )}
            {restarting ? (
              <p
                className="inline-flex items-center gap-1.5 text-foreground"
                role="status"
              >
                <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
                {t(p + "waiting")}
              </p>
            ) : null}
          </div>
        ) : null}
      </section>

      <ConfirmDialog
        isOpen={confirm === "migrate"}
        variant="info"
        title={t(p + "confirmTitle")}
        message={t(p + "confirmMessage")}
        confirmText={t(p + "migrate")}
        onConfirm={() => handleMigrate()}
        onCancel={() => setConfirm(null)}
      />

      <ConfirmDialog
        isOpen={confirm === "restart"}
        variant="info"
        title={t("settings.serverVersion.restartConfirmTitle")}
        message={t(p + "restartConfirmMessage")}
        confirmText={t(p + "restartNow")}
        onConfirm={() => handleRestart()}
        onCancel={() => setConfirm(null)}
      />
    </>
  );
}
