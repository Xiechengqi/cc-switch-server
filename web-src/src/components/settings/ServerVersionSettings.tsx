import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ChevronDown,
  Github,
  Globe,
  Loader2,
  Package,
  RefreshCw,
  Rocket,
  RotateCcw,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { cn } from "@/lib/utils";
import { settingsApi } from "@/lib/api";
import { getWebRuntimeContext, readWebSessionToken } from "@/lib/runtime";
import {
  loadAdminVersionInfo,
  loadBuildInfo,
  readAdminVersionInfoCache,
  restartServerService,
  rollbackServerService,
  startServerUpgrade,
  writeAdminVersionInfoCache,
  type AdminVersionInfo,
} from "@/lib/server-legacy-api";

const SERVER_OFFICIAL_WEBSITE = "https://tokenswitch.org";
const SERVER_GITHUB_URL = "https://github.com/Xiechengqi/cc-switch-server";

type UpgradeLogLevel = "info" | "success" | "warn" | "error" | string;

interface UpgradeLogEntry {
  taskId?: string;
  step?: number;
  totalSteps?: number;
  level?: UpgradeLogLevel;
  message?: string;
  progress?: number | null;
  at?: string;
}

type UpgradeOutcome = "running" | "success" | "failed" | null;

function formatVersionDetails(info: AdminVersionInfo): string {
  return [
    `${info.name} ${info.version}`,
    `commit id: ${info.commitId}`,
    `commit short: ${info.commitShort}`,
    `commit message: ${info.commitMessage}`,
    `commit time: ${info.commitTime}`,
    `build time: ${info.buildTime}`,
    `target: ${info.target}`,
    `profile: ${info.profile}`,
    `rustc: ${info.rustcVersion}`,
    `dirty: ${info.dirty}`,
  ].join("\n");
}

function parseUpgradeLogEntry(raw: string): UpgradeLogEntry {
  try {
    return JSON.parse(raw) as UpgradeLogEntry;
  } catch {
    return { message: raw };
  }
}

function formatUpgradeLogLine(entry: UpgradeLogEntry): string {
  const stepLabel =
    entry.step && entry.totalSteps
      ? `[${entry.step}/${entry.totalSteps}]`
      : "";
  const progressLabel =
    typeof entry.progress === "number" ? `${entry.progress}%` : "";
  const prefix = [stepLabel, progressLabel].filter(Boolean).join(" ");
  const time = entry.at
    ? new Date(entry.at).toLocaleTimeString()
    : "";
  const level = (entry.level || "info").toUpperCase();
  return [time, level, prefix, entry.message || ""].filter(Boolean).join(" ");
}

function upgradeLogLevelClass(level?: UpgradeLogLevel): string {
  switch ((level || "info").toLowerCase()) {
    case "success":
      return "text-emerald-300";
    case "warn":
      return "text-amber-300";
    case "error":
      return "text-red-300";
    default:
      return "text-slate-100";
  }
}

function computeUpgradeProgress(
  logs: UpgradeLogEntry[],
  outcome: UpgradeOutcome,
): number {
  if (outcome === "success") return 100;
  if (logs.length === 0) return 0;
  const latest = logs[logs.length - 1];
  if (typeof latest.progress === "number") {
    return Math.max(0, Math.min(100, latest.progress));
  }
  if (latest.step && latest.totalSteps) {
    return Math.round((latest.step / latest.totalSteps) * 100);
  }
  return 0;
}

function formatBytes(bytes?: number | null): string {
  if (!bytes) return "--";
  const mib = bytes / 1024 / 1024;
  return mib >= 1 ? `${mib.toFixed(1)} MiB` : `${(bytes / 1024).toFixed(1)} KiB`;
}

async function pollHealthAndReload(maxAttempts = 60) {
  for (let attempt = 0; attempt < maxAttempts; attempt += 1) {
    await new Promise((resolve) => window.setTimeout(resolve, 1000));
    try {
      const context = await getWebRuntimeContext(false);
      if (context.mode === "local-admin") {
        window.location.reload();
        return;
      }
    } catch {
      // service may be restarting
    }
  }
}

export function ServerVersionSettings() {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [info, setInfo] = useState<AdminVersionInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [upgradeConfirmOpen, setUpgradeConfirmOpen] = useState(false);
  const [restartConfirmOpen, setRestartConfirmOpen] = useState(false);
  const [rollbackConfirmOpen, setRollbackConfirmOpen] = useState(false);
  const [upgradeLogOpen, setUpgradeLogOpen] = useState(false);
  const [upgradeLogs, setUpgradeLogs] = useState<UpgradeLogEntry[]>([]);
  const [upgradeOutcome, setUpgradeOutcome] = useState<UpgradeOutcome>(null);
  const [usingBuildInfoFallback, setUsingBuildInfoFallback] = useState(false);
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [updateChecked, setUpdateChecked] = useState(false);
  const streamRef = useRef<EventSource | null>(null);
  const logEndRef = useRef<HTMLDivElement | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const adminInfo = await loadAdminVersionInfo();
      setInfo(adminInfo);
      setUsingBuildInfoFallback(false);
      writeAdminVersionInfoCache(adminInfo);
    } catch {
      try {
        const build = await loadBuildInfo();
        setInfo({
          ...build,
          binaryPath: "",
          rollbackPath: "",
          rollbackAvailable: false,
          uptimeSecs: 0,
          restartPending: false,
          upgradeCapable: false,
          service: {
            manager: "nohup",
            active: true,
          },
          latest: {
            binaryUrl: "",
            available: false,
            commitId: "",
            commitShort: "",
            updateAvailable: false,
          },
        });
        setUsingBuildInfoFallback(true);
      } catch (reason) {
        setInfo(null);
        setUsingBuildInfoFallback(false);
        toast.error(reason instanceof Error ? reason.message : String(reason));
      }
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    const cached = readAdminVersionInfoCache();
    if (cached) {
      setInfo(cached);
      setLoading(false);
    } else {
      void refresh();
    }
    return () => {
      streamRef.current?.close();
      streamRef.current = null;
    };
  }, [refresh]);

  useEffect(() => {
    logEndRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [upgradeLogs, upgradeOutcome]);

  const versionDetails = useMemo(
    () => (info ? formatVersionDetails(info) : ""),
    [info],
  );

  const upgradeProgress = useMemo(
    () => computeUpgradeProgress(upgradeLogs, upgradeOutcome),
    [upgradeLogs, upgradeOutcome],
  );

  const latestStepLabel = useMemo(() => {
    const latest = upgradeLogs[upgradeLogs.length - 1];
    if (!latest?.step || !latest.totalSteps) return null;
    return t("settings.serverVersion.upgradeStep", {
      step: latest.step,
      total: latest.totalSteps,
      defaultValue: "步骤 {{step}} / {{total}}",
    });
  }, [upgradeLogs, t]);

  const closeUpgradeStream = useCallback(() => {
    streamRef.current?.close();
    streamRef.current = null;
  }, []);

  const appendUpgradeLog = useCallback((entry: UpgradeLogEntry) => {
    setUpgradeLogs((prev) => [...prev, entry]);
  }, []);

  const streamUpgrade = useCallback(
    (taskId: string) => {
      closeUpgradeStream();
      const token = readWebSessionToken();
      const params = new URLSearchParams({ taskId });
      if (token) params.set("accessToken", token);
      const source = new EventSource(
        `/web-api/admin/upgrade/stream?${params}`,
      );
      streamRef.current = source;

      source.addEventListener("log", (event) => {
        appendUpgradeLog(parseUpgradeLogEntry((event as MessageEvent).data));
      });

      source.addEventListener("done", (event) => {
        appendUpgradeLog({
          level: "info",
          message: `done ${(event as MessageEvent).data}`,
        });
        closeUpgradeStream();
        setBusy(null);
        try {
          const payload = JSON.parse((event as MessageEvent).data) as {
            status?: string;
            restartPending?: boolean;
          };
          if (payload.status === "success") {
            setUpgradeOutcome("success");
            if (payload.restartPending) {
              toast.success(t("settings.serverVersion.upgradePendingRestart"));
              void refresh();
            } else {
              toast.success(t("settings.serverVersion.upgradeSucceeded"));
              void pollHealthAndReload();
            }
          } else if (payload.status === "failed") {
            setUpgradeOutcome("failed");
            toast.error(t("settings.serverVersion.upgradeFailed"));
          } else {
            void refresh();
          }
        } catch {
          void refresh();
        }
      });

      source.onerror = () => {
        appendUpgradeLog({
          level: "error",
          message: t("settings.serverVersion.streamDisconnected"),
        });
        closeUpgradeStream();
        setBusy(null);
        setUpgradeOutcome("failed");
      };
    },
    [appendUpgradeLog, closeUpgradeStream, refresh, t],
  );

  const handleUpgrade = useCallback(
    async (restartAfter: boolean) => {
      setUpgradeConfirmOpen(false);
      setBusy("upgrade");
      setUpgradeLogs([]);
      setUpgradeOutcome("running");
      setUpgradeLogOpen(true);
      try {
        const { taskId } = await startServerUpgrade({ restartAfter });
        streamUpgrade(taskId);
      } catch (reason) {
        toast.error(reason instanceof Error ? reason.message : String(reason));
        setBusy(null);
        setUpgradeOutcome("failed");
      }
    },
    [streamUpgrade],
  );

  const handleRestart = useCallback(async () => {
    setRestartConfirmOpen(false);
    setBusy("restart");
    try {
      await restartServerService();
      toast.success(t("settings.serverVersion.restartScheduled"));
      void pollHealthAndReload();
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : String(reason));
      setBusy(null);
    }
  }, [t]);

  const handleRollback = useCallback(async () => {
    setRollbackConfirmOpen(false);
    setBusy("rollback");
    try {
      await rollbackServerService();
      toast.success(t("settings.serverVersion.rollbackScheduled"));
      void pollHealthAndReload();
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : String(reason));
      setBusy(null);
    }
  }, [t]);

  const restartPending = info?.restartPending ?? false;
  const rollbackAvailable = info?.rollbackAvailable ?? false;
  const upgradeDisabled =
    busy !== null || loading || info?.upgradeCapable === false;

  const formatCommitLabel = (commitId?: string | null, commitShort?: string | null) =>
    commitShort?.trim() || commitId?.trim() || "--";

  const currentCommitLabel = formatCommitLabel(info?.commitId, info?.commitShort);
  const latestCommitLabel = formatCommitLabel(
    info?.latest?.commitId,
    info?.latest?.commitShort,
  );
  const showUpdateCompareSubtitle =
    updateChecked && Boolean(info?.latest?.updateAvailable);

  const cardSubtitle = useMemo(() => {
    if (loading) {
      return t("common.loading");
    }
    if (showUpdateCompareSubtitle) {
      return t("settings.serverVersion.checkUpdateAvailableShort", {
        current: currentCommitLabel,
        latest: latestCommitLabel,
      });
    }
    if (updateChecked) {
      return t("settings.serverVersion.currentVersionOnly", {
        current: currentCommitLabel,
        defaultValue: "当前 {{current}}",
      });
    }
    return info?.versionLine || info?.version || "--";
  }, [
    currentCommitLabel,
    info?.version,
    info?.versionLine,
    latestCommitLabel,
    loading,
    showUpdateCompareSubtitle,
    t,
    updateChecked,
  ]);

  const handleCheckUpdate = useCallback(async () => {
    if (busy || checkingUpdate) return;
    setCheckingUpdate(true);
    try {
      const adminInfo = await loadAdminVersionInfo();
      setInfo(adminInfo);
      setUsingBuildInfoFallback(false);
      writeAdminVersionInfoCache(adminInfo);
      setUpdateChecked(true);

      const { latest } = adminInfo;
      const currentCommit = formatCommitLabel(
        adminInfo.commitId,
        adminInfo.commitShort,
      );
      const latestCommit = formatCommitLabel(
        latest.commitId,
        latest.commitShort,
      );

      if (latest.error) {
        toast.error(
          t("settings.serverVersion.checkUpdateFailed", {
            error: latest.error,
            defaultValue: "检查更新失败：{{error}}",
          }),
        );
        return;
      }
      if (!latest.commitId) {
        toast.error(t("settings.checkUpdateFailed"));
        return;
      }
      if (!latest.updateAvailable) {
        toast.success(
          t("settings.serverVersion.checkUpdateUpToDate", {
            current: currentCommit,
            defaultValue: "当前已是最新版本（{{current}}）",
          }),
        );
        return;
      }
      if (!latest.available) {
        toast.error(t("settings.checkUpdateFailed"));
        return;
      }
      if (!adminInfo.upgradeCapable) {
        toast.info(
          t("settings.serverVersion.checkUpdateReachableButUnavailable", {
            current: currentCommit,
            latest: latestCommit,
            defaultValue:
              "检测到新版本（当前 {{current}}，最新 {{latest}}），但当前环境无法就地升级",
          }),
        );
        return;
      }
      toast.success(
        t("settings.serverVersion.checkUpdateAvailable", {
          current: currentCommit,
          latest: latestCommit,
          size: formatBytes(latest.contentLength),
          defaultValue:
            "当前版本是 {{current}}，最新版本是 {{latest}}，可点击「升级」安装",
        }),
      );
    } catch (reason) {
      toast.error(
        reason instanceof Error
          ? reason.message
          : t("settings.checkUpdateFailed"),
      );
    } finally {
      setCheckingUpdate(false);
    }
  }, [busy, checkingUpdate, t]);

  return (
    <>
      <Collapsible open={open} onOpenChange={setOpen}>
        <div className="rounded-xl border border-border bg-card/50 transition-colors hover:bg-muted/50">
          <div className="flex items-center justify-between gap-4 p-4">
            <CollapsibleTrigger asChild>
              <button
                type="button"
                className="flex min-w-0 flex-1 items-center gap-3 text-left"
              >
                <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-background ring-1 ring-border">
                  <Package className="h-4 w-4 text-sky-500" />
                </div>
                <div className="min-w-0 space-y-1">
                  <p className="text-sm font-medium leading-none">
                    {t("settings.serverVersion.title")}
                  </p>
                  <p
                    className={cn(
                      "truncate text-xs",
                      showUpdateCompareSubtitle
                        ? "font-medium text-primary"
                        : "text-muted-foreground",
                    )}
                  >
                    {cardSubtitle}
                  </p>
                </div>
                <ChevronDown
                  className={cn(
                    "h-4 w-4 shrink-0 text-muted-foreground transition-transform",
                    open && "rotate-180",
                  )}
                />
              </button>
            </CollapsibleTrigger>

            <div className="flex shrink-0 flex-wrap items-center justify-end gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-9 gap-1.5 text-xs"
                onClick={() => void settingsApi.openExternal(SERVER_OFFICIAL_WEBSITE)}
              >
                <Globe className="h-3.5 w-3.5" />
                {t("settings.officialWebsite")}
              </Button>
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-9 gap-1.5 text-xs"
                onClick={() => void settingsApi.openExternal(SERVER_GITHUB_URL)}
              >
                <Github className="h-3.5 w-3.5" />
                {t("settings.github")}
              </Button>
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-9 gap-1.5 text-xs"
                disabled={loading || busy !== null || checkingUpdate}
                onClick={() => void handleCheckUpdate()}
              >
                {checkingUpdate ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <RefreshCw className="h-3.5 w-3.5" />
                )}
                {checkingUpdate
                  ? t("settings.checking")
                  : t("settings.checkForUpdates")}
              </Button>
              {rollbackAvailable ? (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="h-9"
                  disabled={busy !== null}
                  onClick={() => setRollbackConfirmOpen(true)}
                >
                  {busy === "rollback" ? (
                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  ) : (
                    <RotateCcw className="mr-2 h-4 w-4" />
                  )}
                  {t("settings.serverVersion.rollback")}
                </Button>
              ) : null}
              {restartPending ? (
                <Button
                  type="button"
                  size="sm"
                  className="h-9"
                  disabled={busy !== null}
                  onClick={() => setRestartConfirmOpen(true)}
                >
                  {busy === "restart" ? (
                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  ) : (
                    <RotateCcw className="mr-2 h-4 w-4" />
                  )}
                  {t("settings.serverVersion.pendingRestart")}
                </Button>
              ) : (
                <Button
                  type="button"
                  size="sm"
                  className="h-9"
                  disabled={upgradeDisabled}
                  onClick={() => setUpgradeConfirmOpen(true)}
                >
                  {busy === "upgrade" ? (
                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  ) : (
                    <Rocket className="mr-2 h-4 w-4" />
                  )}
                  {t("settings.serverVersion.upgrade")}
                </Button>
              )}
            </div>
          </div>

          <CollapsibleContent>
            <div className="border-t border-border/50 px-4 pb-4 pt-3 space-y-3">
              <pre className="overflow-x-auto whitespace-pre-wrap break-words rounded-lg bg-muted/40 p-3 font-mono text-xs leading-relaxed text-foreground">
                {versionDetails || t("settings.serverVersion.empty")}
              </pre>
              {info && !info.upgradeCapable ? (
                <p className="mt-2 text-xs text-amber-600 dark:text-amber-400">
                  {usingBuildInfoFallback
                    ? t("settings.serverVersion.adminFallbackHint")
                    : t("settings.serverVersion.upgradeUnavailable")}
                </p>
              ) : null}
            </div>
          </CollapsibleContent>
        </div>
      </Collapsible>

      <ConfirmDialog
        isOpen={upgradeConfirmOpen}
        variant="info"
        title={t("settings.serverVersion.upgradeConfirmTitle")}
        message={t("settings.serverVersion.upgradeConfirmMessage")}
        confirmText={t("settings.serverVersion.upgrade")}
        checkboxLabel={t("settings.serverVersion.restartAfterUpgrade")}
        checkboxDefaultChecked
        onConfirm={(restartAfter) => void handleUpgrade(restartAfter)}
        onCancel={() => setUpgradeConfirmOpen(false)}
      />

      <ConfirmDialog
        isOpen={restartConfirmOpen}
        variant="info"
        title={t("settings.serverVersion.restartConfirmTitle")}
        message={t("settings.serverVersion.restartConfirmMessage")}
        confirmText={t("settings.serverVersion.restart")}
        onConfirm={() => void handleRestart()}
        onCancel={() => setRestartConfirmOpen(false)}
      />

      <ConfirmDialog
        isOpen={rollbackConfirmOpen}
        variant="destructive"
        title={t("settings.serverVersion.rollbackConfirmTitle")}
        message={t("settings.serverVersion.rollbackConfirmMessage")}
        confirmText={t("settings.serverVersion.rollback")}
        onConfirm={() => void handleRollback()}
        onCancel={() => setRollbackConfirmOpen(false)}
      />

      <Dialog
        open={upgradeLogOpen}
        onOpenChange={(nextOpen) => {
          if (!nextOpen && busy === "upgrade") return;
          setUpgradeLogOpen(nextOpen);
        }}
      >
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>
              {t("settings.serverVersion.upgradeLogTitle")}
            </DialogTitle>
            <DialogDescription>
              {t("settings.serverVersion.upgradeLogDesc")}
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-3">
            <div className="space-y-2">
              <div className="flex items-center justify-between gap-3 text-xs text-muted-foreground">
                <span>
                  {latestStepLabel ||
                    (busy === "upgrade"
                      ? t("settings.serverVersion.upgradeRunning")
                      : upgradeOutcome === "success"
                        ? t("settings.serverVersion.upgradeSucceeded")
                        : upgradeOutcome === "failed"
                          ? t("settings.serverVersion.upgradeFailed")
                          : t("settings.serverVersion.waitingLogs"))}
                </span>
                <span className="font-mono tabular-nums">{upgradeProgress}%</span>
              </div>
              <div className="h-2 overflow-hidden rounded-full bg-muted">
                <div
                  className={cn(
                    "h-full rounded-full transition-all duration-300",
                    upgradeOutcome === "failed"
                      ? "bg-red-500"
                      : upgradeOutcome === "success"
                        ? "bg-emerald-500"
                        : "bg-sky-500",
                  )}
                  style={{ width: `${upgradeProgress}%` }}
                />
              </div>
            </div>

            <ScrollArea className="h-96 rounded-lg border bg-slate-950">
              <div className="space-y-2 p-4 font-mono text-xs">
                {upgradeLogs.length > 0 ? (
                  upgradeLogs.map((entry, index) => (
                    <div
                      key={`${index}-${entry.at || entry.message || index}`}
                      className={upgradeLogLevelClass(entry.level)}
                    >
                      {formatUpgradeLogLine(entry)}
                    </div>
                  ))
                ) : (
                  <div className="text-slate-400">
                    {busy === "upgrade" ? (
                      <span className="inline-flex items-center gap-2">
                        <Loader2 className="h-3.5 w-3.5 animate-spin" />
                        {t("settings.serverVersion.waitingLogs")}
                      </span>
                    ) : (
                      t("settings.serverVersion.waitingLogs")
                    )}
                  </div>
                )}
                <div ref={logEndRef} />
              </div>
            </ScrollArea>
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
