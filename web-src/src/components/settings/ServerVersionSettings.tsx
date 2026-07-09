import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  CheckCircle2,
  ChevronDown,
  Github,
  Globe,
  Loader2,
  Package,
  RefreshCw,
  Rocket,
  RotateCcw,
  X,
  XCircle,
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
import { Badge } from "@/components/ui/badge";
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

function formatUpgradeLogTime(at?: string): string {
  if (!at) return "";
  const date = new Date(at);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleTimeString();
}

function normalizeUpgradeLogLevel(level?: UpgradeLogLevel): string {
  return (level || "info").toLowerCase();
}

function upgradeLogLevelClass(level?: UpgradeLogLevel): string {
  switch (normalizeUpgradeLogLevel(level)) {
    case "success":
      return "text-emerald-600 dark:text-emerald-300";
    case "warn":
      return "text-amber-600 dark:text-amber-300";
    case "error":
      return "text-red-600 dark:text-red-300";
    case "progress":
      return "text-sky-600 dark:text-sky-300";
    default:
      return "text-foreground/80";
  }
}

function upgradeLogBadgeClass(level?: UpgradeLogLevel): string {
  switch (normalizeUpgradeLogLevel(level)) {
    case "success":
      return "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300";
    case "warn":
      return "border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300";
    case "error":
      return "border-red-500/30 bg-red-500/10 text-red-700 dark:text-red-300";
    case "progress":
      return "border-sky-500/30 bg-sky-500/10 text-sky-700 dark:text-sky-300";
    default:
      return "border-border bg-muted/60 text-muted-foreground";
  }
}

function computeUpgradeProgress(
  logs: UpgradeLogEntry[],
  outcome: UpgradeOutcome,
  running: boolean,
): number {
  if (outcome === "success") return 100;
  if (logs.length === 0) {
    return running ? 4 : 0;
  }
  const latest = logs[logs.length - 1];
  if (typeof latest.progress === "number") {
    return Math.max(0, Math.min(100, latest.progress));
  }
  if (latest.step && latest.totalSteps) {
    return Math.round((latest.step / latest.totalSteps) * 100);
  }
  return running ? 4 : 0;
}

function UpgradeLogRow({ entry }: { entry: UpgradeLogEntry }) {
  const level = normalizeUpgradeLogLevel(entry.level);
  const time = formatUpgradeLogTime(entry.at);
  const stepLabel =
    entry.step && entry.totalSteps
      ? `${entry.step}/${entry.totalSteps}`
      : null;

  return (
    <div className="group flex items-start gap-2 rounded-md px-2 py-1.5 transition-colors hover:bg-muted/40">
      <span className="w-16 shrink-0 pt-0.5 font-mono text-[10px] tabular-nums text-muted-foreground">
        {time || "--:--:--"}
      </span>
      <Badge
        variant="outline"
        className={cn(
          "h-5 shrink-0 px-1.5 py-0 text-[10px] font-semibold uppercase tracking-wide",
          upgradeLogBadgeClass(entry.level),
        )}
      >
        {level}
      </Badge>
      {stepLabel ? (
        <span className="shrink-0 pt-0.5 font-mono text-[10px] text-muted-foreground">
          {stepLabel}
        </span>
      ) : null}
      <p className={cn("min-w-0 flex-1 break-all text-xs leading-5", upgradeLogLevelClass(entry.level))}>
        {entry.message || ""}
      </p>
    </div>
  );
}

function formatBytes(bytes?: number | null): string {
  if (!bytes) return "--";
  const mib = bytes / 1024 / 1024;
  return mib >= 1 ? `${mib.toFixed(1)} MiB` : `${(bytes / 1024).toFixed(1)} KiB`;
}

function formatCommitLabel(
  commitId?: string | null,
  commitShort?: string | null,
): string {
  return commitShort?.trim() || commitId?.trim() || "--";
}

function buildVersionCardSubtitle(
  info: AdminVersionInfo | null,
  t: (key: string, options?: Record<string, string>) => string,
): string {
  if (!info?.commitId) {
    return "--";
  }
  const current = formatCommitLabel(info.commitId, info.commitShort);
  const latest = formatCommitLabel(
    info.latest?.commitId,
    info.latest?.commitShort,
  );

  if (info.latest?.commitId && info.latest.updateAvailable && latest !== "--") {
    return t("settings.serverVersion.versionCardUpdateAvailable", {
      current,
      latest,
      defaultValue: "当前版本 {{current}} -> 最新版本 {{latest}}",
    });
  }
  return t("settings.serverVersion.versionCardUpToDate", {
    current,
    defaultValue: "已是最新版本 {{current}}",
  });
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
  const streamRef = useRef<EventSource | null>(null);
  const streamFinishedRef = useRef(false);
  const streamReconnectAttemptsRef = useRef(0);
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

  const upgradeRunning = busy === "upgrade" || upgradeOutcome === "running";

  const upgradeProgress = useMemo(
    () => computeUpgradeProgress(upgradeLogs, upgradeOutcome, upgradeRunning),
    [upgradeLogs, upgradeOutcome, upgradeRunning],
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

  const upgradeStatusMessage = useMemo(() => {
    if (upgradeRunning) {
      return latestStepLabel || t("settings.serverVersion.upgradeRunning");
    }
    if (upgradeOutcome === "success") {
      return t("settings.serverVersion.upgradeSucceeded");
    }
    if (upgradeOutcome === "failed") {
      const lastError = [...upgradeLogs]
        .reverse()
        .find((entry) => normalizeUpgradeLogLevel(entry.level) === "error");
      return lastError?.message || t("settings.serverVersion.upgradeFailed");
    }
    return t("settings.serverVersion.waitingLogs");
  }, [latestStepLabel, t, upgradeLogs, upgradeOutcome, upgradeRunning]);

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
      streamFinishedRef.current = false;
      streamReconnectAttemptsRef.current = 0;

      const connect = () => {
        const token = readWebSessionToken();
        const params = new URLSearchParams({ taskId });
        if (token) params.set("accessToken", token);
        const source = new EventSource(
          `/web-api/admin/upgrade/stream?${params}`,
        );
        streamRef.current = source;

        source.addEventListener("log", (event) => {
          streamReconnectAttemptsRef.current = 0;
          appendUpgradeLog(parseUpgradeLogEntry((event as MessageEvent).data));
        });

        source.addEventListener("done", (event) => {
          if (streamFinishedRef.current) return;
          streamFinishedRef.current = true;
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
                toast.success(
                  t("settings.serverVersion.upgradePendingRestart"),
                );
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
          if (streamFinishedRef.current) return;
          if (source.readyState === EventSource.CONNECTING) return;

          if (
            source.readyState === EventSource.CLOSED &&
            streamReconnectAttemptsRef.current < 5
          ) {
            closeUpgradeStream();
            streamReconnectAttemptsRef.current += 1;
            window.setTimeout(() => {
              if (!streamFinishedRef.current) {
                connect();
              }
            }, 400 * streamReconnectAttemptsRef.current);
            return;
          }

          if (!streamFinishedRef.current) {
            streamFinishedRef.current = true;
            closeUpgradeStream();
            appendUpgradeLog({
              level: "error",
              message: t("settings.serverVersion.streamDisconnected"),
            });
            setUpgradeOutcome("failed");
            setBusy(null);
            toast.error(t("settings.serverVersion.upgradeFailed"));
          }
        };
      };

      connect();
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

  const showUpdateCompareSubtitle = Boolean(
    info?.latest?.commitId && info?.latest?.updateAvailable,
  );

  const cardSubtitle = useMemo(() => {
    if (loading && !info) {
      return t("common.loading");
    }
    return buildVersionCardSubtitle(info, t);
  }, [info, loading, t]);

  const upgradeStatusVisual = useMemo(() => {
    if (upgradeRunning) {
      return {
        Icon: Loader2,
        spin: true,
        ring: "bg-sky-500/10 text-sky-600 dark:text-sky-400",
        bar: "bg-sky-500",
      };
    }
    if (upgradeOutcome === "success") {
      return {
        Icon: CheckCircle2,
        spin: false,
        ring: "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400",
        bar: "bg-emerald-500",
      };
    }
    if (upgradeOutcome === "failed") {
      return {
        Icon: XCircle,
        spin: false,
        ring: "bg-red-500/10 text-red-600 dark:text-red-400",
        bar: "bg-red-500",
      };
    }
    return {
      Icon: Package,
      spin: false,
      ring: "bg-muted text-muted-foreground",
      bar: "bg-muted-foreground/40",
    };
  }, [upgradeOutcome, upgradeRunning]);

  const UpgradeStatusIcon = upgradeStatusVisual.Icon;

  const handleCheckUpdate = useCallback(async () => {
    if (busy || checkingUpdate) return;
    setCheckingUpdate(true);
    try {
      const adminInfo = await loadAdminVersionInfo();
      setInfo(adminInfo);
      setUsingBuildInfoFallback(false);
      writeAdminVersionInfoCache(adminInfo);

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
        <DialogContent className="relative max-w-2xl gap-0 overflow-hidden p-0">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="absolute right-3 top-3 z-10 h-8 w-8 rounded-full"
            disabled={busy === "upgrade"}
            onClick={() => setUpgradeLogOpen(false)}
            aria-label={t("common.close")}
          >
            <X className="h-4 w-4" />
          </Button>

          <div className="border-b bg-muted/20 px-6 py-5 pr-14">
            <DialogHeader className="space-y-4 text-left">
              <div className="flex items-start gap-4">
                <div
                  className={cn(
                    "flex h-11 w-11 shrink-0 items-center justify-center rounded-xl ring-1 ring-border/60",
                    upgradeStatusVisual.ring,
                  )}
                >
                  <UpgradeStatusIcon
                    className={cn(
                      "h-5 w-5",
                      upgradeStatusVisual.spin && "animate-spin",
                    )}
                  />
                </div>
                <div className="min-w-0 flex-1 space-y-1">
                  <DialogTitle className="text-base">
                    {t("settings.serverVersion.upgradeLogTitle")}
                  </DialogTitle>
                  <DialogDescription className="text-sm leading-6">
                    {upgradeStatusMessage}
                  </DialogDescription>
                </div>
                <div className="shrink-0 text-right">
                  <div className="font-mono text-2xl font-semibold tabular-nums leading-none">
                    {upgradeProgress}%
                  </div>
                  {latestStepLabel ? (
                    <Badge variant="outline" className="mt-2 text-[10px]">
                      {latestStepLabel}
                    </Badge>
                  ) : null}
                </div>
              </div>

              <div className="space-y-2">
                <div className="h-2 overflow-hidden rounded-full bg-muted">
                  <div
                    className={cn(
                      "h-full rounded-full transition-all duration-500 ease-out",
                      upgradeStatusVisual.bar,
                      upgradeRunning && upgradeProgress < 8
                        ? "animate-pulse"
                        : "",
                    )}
                    style={{ width: `${Math.max(upgradeProgress, upgradeRunning ? 4 : 0)}%` }}
                  />
                </div>
              </div>
            </DialogHeader>
          </div>

          <ScrollArea className="h-[min(24rem,52vh)]">
            <div className="space-y-0.5 p-3">
              {upgradeLogs.length > 0 ? (
                upgradeLogs.map((entry, index) => (
                  <UpgradeLogRow
                    key={`${index}-${entry.at || entry.message || index}`}
                    entry={entry}
                  />
                ))
              ) : (
                <div className="flex min-h-40 items-center justify-center px-4 text-sm text-muted-foreground">
                  {upgradeRunning ? (
                    <span className="inline-flex items-center gap-2">
                      <Loader2 className="h-4 w-4 animate-spin" />
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

          <div className="flex justify-end border-t bg-muted/10 px-6 py-3">
            <Button
              type="button"
              variant="outline"
              disabled={busy === "upgrade"}
              onClick={() => setUpgradeLogOpen(false)}
            >
              {t("common.close")}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
