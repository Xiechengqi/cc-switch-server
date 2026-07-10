import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  CheckCircle2,
  ChevronDown,
  Clock3,
  Cpu,
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
import {
  abortableDelay,
  consumeAuthenticatedSse,
  isAbortError,
} from "@/lib/sse";
import {
  loadAdminVersionInfo,
  loadBuildInfo,
  loadRuntimeVersionInfo,
  loadUpgradeStatus,
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

type UpgradeOutcome = "running" | "restarting" | "success" | "failed" | null;

type UpgradeDonePayload = {
  status?: string;
  restartPending?: boolean;
};

function upgradeLogsIndicateRestartScheduled(logs: UpgradeLogEntry[]): boolean {
  return logs.some((entry) => {
    if (normalizeUpgradeLogLevel(entry.level) === "error") return false;
    const message = (entry.message ?? "").toLowerCase();
    return (
      message.includes("restart helper scheduled") ||
      message.includes("restart scheduled") ||
      message.includes("process will restart")
    );
  });
}

function commitsMatch(left?: string | null, right?: string | null): boolean {
  const normalizedLeft = left?.trim().toLowerCase() ?? "";
  const normalizedRight = right?.trim().toLowerCase() ?? "";
  if (!normalizedLeft || !normalizedRight) return false;
  const prefixLength = Math.min(normalizedLeft.length, normalizedRight.length, 12);
  return (
    prefixLength >= 7 &&
    normalizedLeft.slice(0, prefixLength) ===
      normalizedRight.slice(0, prefixLength)
  );
}

function upgradeLogKey(entry: UpgradeLogEntry): string {
  return [
    entry.taskId ?? "",
    entry.at ?? "",
    entry.step ?? "",
    entry.level ?? "",
    entry.progress ?? "",
    entry.message ?? "",
  ].join("\u0000");
}

function buildUpgradeStreamParams(taskId: string): string {
  return new URLSearchParams({ taskId }).toString();
}

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

function formatProcessUptime(totalSeconds: number): string {
  const seconds = Math.max(0, Math.floor(totalSeconds));
  const hours = Math.floor(seconds / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  const remainingSeconds = seconds % 60;
  return [hours, minutes, remainingSeconds]
    .map((value) => value.toString().padStart(2, "0"))
    .join(":");
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

function upgradeLogsIndicateHardFailure(logs: UpgradeLogEntry[]): boolean {
  return logs.some((entry) => {
    if (normalizeUpgradeLogLevel(entry.level) !== "error") {
      return false;
    }
    const message = (entry.message ?? "").toLowerCase();
    return (
      !message.includes("stream disconnected") &&
      !message.includes("日志流已断开")
    );
  });
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
  active: boolean,
): number {
  if (outcome === "success") return 100;
  if (outcome === "restarting") {
    const base =
      logs.length > 0 ? computeUpgradeProgress(logs, "running", true) : 90;
    return Math.max(base, 95);
  }
  if (logs.length === 0) {
    return active ? 4 : 0;
  }
  const latest = logs[logs.length - 1];
  if (typeof latest.progress === "number") {
    return Math.max(0, Math.min(100, latest.progress));
  }
  if (latest.step && latest.totalSteps) {
    return Math.round((latest.step / latest.totalSteps) * 100);
  }
  return active ? 4 : 0;
}

function UpgradeLogRow({ entry }: { entry: UpgradeLogEntry }) {
  const level = normalizeUpgradeLogLevel(entry.level);
  const time = formatUpgradeLogTime(entry.at);
  const stepLabel =
    entry.step && entry.totalSteps ? `${entry.step}/${entry.totalSteps}` : null;

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
      <p
        className={cn(
          "min-w-0 flex-1 break-all text-xs leading-5",
          upgradeLogLevelClass(entry.level),
        )}
      >
        {entry.message || ""}
      </p>
    </div>
  );
}

function formatBytes(bytes?: number | null): string {
  if (!bytes) return "--";
  const mib = bytes / 1024 / 1024;
  return mib >= 1
    ? `${mib.toFixed(1)} MiB`
    : `${(bytes / 1024).toFixed(1)} KiB`;
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

async function pollReplacementAndReload(
  previousProcessId: number,
  maxAttempts = 60,
): Promise<boolean> {
  for (let attempt = 0; attempt < maxAttempts; attempt += 1) {
    await new Promise((resolve) => window.setTimeout(resolve, 1000));
    try {
      const runtime = await loadRuntimeVersionInfo();
      if (
        runtime.processId > 0 &&
        previousProcessId > 0 &&
        runtime.processId !== previousProcessId
      ) {
        window.location.reload();
        return true;
      }
    } catch {
      // service may be restarting
    }
  }
  return false;
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
  const [runtimeTickMs, setRuntimeTickMs] = useState(() => Date.now());
  const streamRef = useRef<AbortController | null>(null);
  const streamFinishedRef = useRef(false);
  const streamReconnectAttemptsRef = useRef(0);
  const restartWaitAnnouncedRef = useRef(false);
  const upgradeLogsRef = useRef<UpgradeLogEntry[]>([]);
  const upgradeLogKeysRef = useRef(new Set<string>());
  const logEndRef = useRef<HTMLDivElement | null>(null);
  const uptimeAnchorRef = useRef({ uptimeSecs: 0, capturedAtMs: Date.now() });

  const applyVersionInfo = useCallback((next: AdminVersionInfo) => {
    const capturedAtMs = Date.now();
    uptimeAnchorRef.current = {
      uptimeSecs: next.uptimeSecs,
      capturedAtMs,
    };
    setRuntimeTickMs(capturedAtMs);
    setInfo(next);
  }, []);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const adminInfo = await loadAdminVersionInfo();
      applyVersionInfo(adminInfo);
      setUsingBuildInfoFallback(false);
      writeAdminVersionInfoCache(adminInfo);
    } catch {
      try {
        const build = await loadBuildInfo();
        applyVersionInfo({
          ...build,
          binaryPath: "",
          rollbackPath: "",
          rollbackAvailable: false,
          processId: 0,
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
  }, [applyVersionInfo]);

  useEffect(() => {
    const cached = readAdminVersionInfoCache();
    if (cached) {
      applyVersionInfo(cached);
      setLoading(false);
      void refresh();
    } else {
      void refresh();
    }
    return () => {
      streamRef.current?.abort();
      streamRef.current = null;
    };
  }, [applyVersionInfo, refresh]);

  useEffect(() => {
    if (!info) return;
    const timer = window.setInterval(() => setRuntimeTickMs(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [info]);

  useEffect(() => {
    logEndRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [upgradeLogs, upgradeOutcome]);

  const versionDetails = useMemo(
    () => (info ? formatVersionDetails(info) : ""),
    [info],
  );

  const processUptimeSecs = info
    ? uptimeAnchorRef.current.uptimeSecs +
      Math.max(
        0,
        Math.floor((runtimeTickMs - uptimeAnchorRef.current.capturedAtMs) / 1000),
      )
    : 0;

  const upgradeActive =
    busy === "upgrade" ||
    upgradeOutcome === "running" ||
    upgradeOutcome === "restarting";
  const upgradeRunning = busy === "upgrade" || upgradeOutcome === "running";

  const upgradeProgress = useMemo(
    () => computeUpgradeProgress(upgradeLogs, upgradeOutcome, upgradeActive),
    [upgradeLogs, upgradeOutcome, upgradeActive],
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
    if (upgradeOutcome === "restarting") {
      return t("settings.serverVersion.upgradeRestarting");
    }
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
    streamRef.current?.abort();
    streamRef.current = null;
  }, []);

  const appendUpgradeLog = useCallback((entry: UpgradeLogEntry) => {
    const key = upgradeLogKey(entry);
    if (upgradeLogKeysRef.current.has(key)) return;
    upgradeLogKeysRef.current.add(key);
    setUpgradeLogs((prev) => {
      const next = [...prev, entry];
      upgradeLogsRef.current = next;
      return next;
    });
  }, []);

  const applyUpgradeDone = useCallback(
    (payload: UpgradeDonePayload) => {
      if (streamFinishedRef.current) return;
      streamFinishedRef.current = true;
      closeUpgradeStream();
      setBusy(null);
      if (payload.status === "success") {
        setUpgradeOutcome("success");
        if (payload.restartPending) {
          toast.success(t("settings.serverVersion.upgradePendingRestart"));
          void refresh();
        } else {
          toast.success(t("settings.serverVersion.upgradeSucceeded"));
          const previousProcessId = info?.processId ?? 0;
          void pollReplacementAndReload(previousProcessId).then((replaced) => {
            if (!replaced) {
              toast.error(t("settings.serverVersion.restartNotObserved"));
              void refresh();
            }
          });
        }
        return;
      }
      if (payload.status === "failed") {
        setUpgradeOutcome("failed");
        toast.error(t("settings.serverVersion.upgradeFailed"));
        return;
      }
      void refresh();
    },
    [closeUpgradeStream, info?.processId, refresh, t],
  );

  const markUpgradeRestarting = useCallback(() => {
    if (streamFinishedRef.current || restartWaitAnnouncedRef.current) return;
    restartWaitAnnouncedRef.current = true;
    setBusy(null);
    setUpgradeOutcome("restarting");
    toast.success(t("settings.serverVersion.upgradeRestarting"));
  }, [t]);

  const resolveUpgradeAfterStreamLoss = useCallback(
    async (taskId: string, signal: AbortSignal) => {
      while (!signal.aborted) {
        const logs = upgradeLogsRef.current;
        try {
          const status = await loadUpgradeStatus(taskId);
          if (status.logs.length > 0) {
            setUpgradeLogs(status.logs);
            upgradeLogsRef.current = status.logs;
            upgradeLogKeysRef.current = new Set(status.logs.map(upgradeLogKey));
          }
          if (status.status === "success") {
            if (status.targetCommitId) {
              const build = await loadBuildInfo();
              if (!commitsMatch(build.commitId, status.targetCommitId)) {
                appendUpgradeLog({
                  taskId,
                  step: 7,
                  totalSteps: 7,
                  level: "error",
                  message: `replacement commit mismatch: expected ${status.targetCommitId}, got ${build.commitId}`,
                  progress: null,
                  at: new Date().toISOString(),
                });
                applyUpgradeDone({ status: "failed" });
                return;
              }
            }
            applyUpgradeDone({
              status: "success",
              restartPending: status.restartPending,
            });
            return;
          }
          if (status.status === "failed") {
            applyUpgradeDone({ status: "failed" });
            return;
          }
        } catch {
          if (upgradeLogsIndicateHardFailure(logs)) {
            applyUpgradeDone({ status: "failed" });
            return;
          }
          if (upgradeLogsIndicateRestartScheduled(logs)) {
            markUpgradeRestarting();
          }
        }
        await abortableDelay(2000, signal).catch(() => undefined);
      }
    },
    [appendUpgradeLog, applyUpgradeDone, markUpgradeRestarting],
  );

  const streamUpgrade = useCallback(
    (taskId: string) => {
      closeUpgradeStream();
      streamFinishedRef.current = false;
      streamReconnectAttemptsRef.current = 0;

      const controller = new AbortController();
      streamRef.current = controller;
      void (async () => {
        while (
          !controller.signal.aborted &&
          !streamFinishedRef.current &&
          streamReconnectAttemptsRef.current < 5
        ) {
          try {
            await consumeAuthenticatedSse(
              `/web-api/admin/upgrade/stream?${buildUpgradeStreamParams(taskId)}`,
              {
                signal: controller.signal,
                onEvent: (event) => {
                  if (event.event === "log") {
                    appendUpgradeLog(parseUpgradeLogEntry(event.data));
                  } else if (event.event === "done") {
                    try {
                      applyUpgradeDone(
                        JSON.parse(event.data) as UpgradeDonePayload,
                      );
                    } catch {
                      applyUpgradeDone({ status: "success" });
                    }
                  }
                },
              },
            );
            if (streamFinishedRef.current) return;
          } catch (error) {
            if (controller.signal.aborted || isAbortError(error)) return;
          }
          streamReconnectAttemptsRef.current += 1;
          await abortableDelay(
            400 * streamReconnectAttemptsRef.current,
            controller.signal,
          ).catch(() => undefined);
        }
        if (!controller.signal.aborted && !streamFinishedRef.current) {
          toast.warning(t("settings.serverVersion.streamDisconnected"));
          await resolveUpgradeAfterStreamLoss(taskId, controller.signal);
        }
      })();
    },
    [
      appendUpgradeLog,
      applyUpgradeDone,
      closeUpgradeStream,
      resolveUpgradeAfterStreamLoss,
      t,
    ],
  );

  const handleUpgrade = useCallback(
    async (restartAfter: boolean) => {
      setUpgradeConfirmOpen(false);
      setBusy("upgrade");
      setUpgradeLogs([]);
      upgradeLogsRef.current = [];
      upgradeLogKeysRef.current.clear();
      restartWaitAnnouncedRef.current = false;
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
    const previousProcessId = info?.processId ?? 0;
    if (previousProcessId <= 0) {
      toast.error(t("settings.serverVersion.restartNotObserved"));
      return;
    }
    setBusy("restart");
    try {
      await restartServerService();
      toast.success(t("settings.serverVersion.restartScheduled"));
      const replaced = await pollReplacementAndReload(previousProcessId);
      if (!replaced) {
        toast.error(t("settings.serverVersion.restartNotObserved"));
        setBusy(null);
        void refresh();
      }
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : String(reason));
      setBusy(null);
    }
  }, [info?.processId, refresh, t]);

  const handleRollback = useCallback(async () => {
    setRollbackConfirmOpen(false);
    const previousProcessId = info?.processId ?? 0;
    if (previousProcessId <= 0) {
      toast.error(t("settings.serverVersion.restartNotObserved"));
      return;
    }
    setBusy("rollback");
    try {
      await rollbackServerService();
      toast.success(t("settings.serverVersion.rollbackScheduled"));
      const replaced = await pollReplacementAndReload(previousProcessId);
      if (!replaced) {
        toast.error(t("settings.serverVersion.restartNotObserved"));
        setBusy(null);
        void refresh();
      }
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : String(reason));
      setBusy(null);
    }
  }, [info?.processId, refresh, t]);

  const restartPending = info?.restartPending ?? false;
  const rollbackAvailable = info?.rollbackAvailable ?? false;
  const upgradeDisabled =
    busy !== null || loading || restartPending || info?.upgradeCapable === false;

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
    if (upgradeOutcome === "restarting") {
      return {
        Icon: Loader2,
        spin: true,
        ring: "bg-amber-500/10 text-amber-600 dark:text-amber-400",
        bar: "bg-amber-500",
      };
    }
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
      applyVersionInfo(adminInfo);
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
  }, [applyVersionInfo, busy, checkingUpdate, t]);

  return (
    <>
      <Collapsible open={open} onOpenChange={setOpen}>
        <div className="rounded-xl border border-border bg-card/50 transition-colors hover:bg-muted/50">
          <div className="flex flex-col gap-3 p-4 sm:flex-row sm:items-center sm:justify-between sm:gap-4">
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

            <div className="grid w-full grid-cols-2 gap-2 sm:flex sm:w-auto sm:shrink-0 sm:flex-wrap sm:items-center sm:justify-end">
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-9 gap-1.5 text-xs"
                onClick={() =>
                  void settingsApi.openExternal(SERVER_OFFICIAL_WEBSITE)
                }
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
              <Button
                type="button"
                variant={restartPending ? "default" : "outline"}
                size="sm"
                className="h-9"
                disabled={busy !== null || !info?.processId}
                onClick={() => setRestartConfirmOpen(true)}
              >
                {busy === "restart" ? (
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                ) : (
                  <RotateCcw className="mr-2 h-4 w-4" />
                )}
                {restartPending
                  ? t("settings.serverVersion.pendingRestart")
                  : t("settings.serverVersion.restart")}
              </Button>
            </div>
          </div>

          <CollapsibleContent>
            <div className="border-t border-border/50 px-4 pb-4 pt-3 space-y-3">
              <div className="flex flex-wrap items-center gap-x-5 gap-y-2 text-xs text-muted-foreground">
                <span className="inline-flex items-center gap-1.5">
                  <Cpu className="h-3.5 w-3.5" />
                  {t("settings.serverVersion.processId")}
                  <code className="font-mono text-foreground">
                    {info?.processId || "--"}
                  </code>
                </span>
                <span className="inline-flex items-center gap-1.5">
                  <Clock3 className="h-3.5 w-3.5" />
                  {t("settings.serverVersion.processUptime")}
                  <code className="font-mono tabular-nums text-foreground">
                    {info ? formatProcessUptime(processUptimeSecs) : "--"}
                  </code>
                </span>
              </div>
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
        <DialogContent
          zIndex="alert"
          className="max-w-2xl gap-0 overflow-hidden p-0"
        >
          <div className="relative">
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
                        upgradeActive && upgradeProgress < 8
                          ? "animate-pulse"
                          : "",
                      )}
                      style={{
                        width: `${Math.max(upgradeProgress, upgradeActive ? 4 : 0)}%`,
                      }}
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
                    {upgradeActive ? (
                      <span className="inline-flex items-center gap-2">
                        <Loader2 className="h-4 w-4 animate-spin" />
                        {upgradeOutcome === "restarting"
                          ? t("settings.serverVersion.upgradeRestarting")
                          : t("settings.serverVersion.waitingLogs")}
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
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
