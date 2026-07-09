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
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { cn } from "@/lib/utils";
import { settingsApi } from "@/lib/api";
import { readToken } from "@/lib/runtime";
import {
  loadAdminVersionInfo,
  loadBuildInfo,
  restartServerService,
  startServerUpgrade,
  type AdminVersionInfo,
} from "@/lib/server-legacy-api";

const SERVER_OFFICIAL_WEBSITE = "https://tokenswitch.cc";
const SERVER_GITHUB_URL = "https://github.com/Xiechengqi/cc-switch-server";

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

async function pollHealthAndReload(maxAttempts = 60) {
  for (let attempt = 0; attempt < maxAttempts; attempt += 1) {
    await new Promise((resolve) => window.setTimeout(resolve, 1000));
    try {
      const response = await fetch("/health", { cache: "no-store" });
      if (response.ok) {
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
  const [upgradeLogOpen, setUpgradeLogOpen] = useState(false);
  const [upgradeLogs, setUpgradeLogs] = useState<string[]>([]);
  const [usingBuildInfoFallback, setUsingBuildInfoFallback] = useState(false);
  const streamRef = useRef<EventSource | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const adminInfo = await loadAdminVersionInfo();
      setInfo(adminInfo);
      setUsingBuildInfoFallback(false);
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
    void refresh();
    return () => {
      streamRef.current?.close();
      streamRef.current = null;
    };
  }, [refresh]);

  const versionDetails = useMemo(
    () => (info ? formatVersionDetails(info) : ""),
    [info],
  );

  const closeUpgradeStream = useCallback(() => {
    streamRef.current?.close();
    streamRef.current = null;
  }, []);

  const streamUpgrade = useCallback(
    (taskId: string) => {
      closeUpgradeStream();
      const token = readToken();
      const params = new URLSearchParams({ taskId });
      if (token) params.set("accessToken", token);
      const source = new EventSource(`/api/admin/upgrade/stream?${params}`);
      streamRef.current = source;

      source.addEventListener("log", (event) => {
        try {
          const data = JSON.parse((event as MessageEvent).data) as {
            at?: string;
            level?: string;
            message?: string;
          };
          const line = `${data.at || ""} ${data.level || "info"} ${data.message || ""}`.trim();
          setUpgradeLogs((prev) => [...prev, line]);
        } catch {
          setUpgradeLogs((prev) => [...prev, (event as MessageEvent).data]);
        }
      });

      source.addEventListener("done", (event) => {
        setUpgradeLogs((prev) => [...prev, `done ${(event as MessageEvent).data}`]);
        closeUpgradeStream();
        setBusy(null);
        try {
          const payload = JSON.parse((event as MessageEvent).data) as {
            status?: string;
            restartPending?: boolean;
          };
          if (payload.status === "success" && payload.restartPending) {
            toast.success(t("settings.serverVersion.upgradePendingRestart"));
            void refresh();
          } else if (payload.status === "failed") {
            toast.error(t("settings.serverVersion.upgradeFailed"));
          }
        } catch {
          void refresh();
        }
      });

      source.onerror = () => {
        setUpgradeLogs((prev) => [
          ...prev,
          t("settings.serverVersion.streamDisconnected"),
        ]);
        closeUpgradeStream();
        setBusy(null);
      };
    },
    [closeUpgradeStream, refresh, t],
  );

  const handleUpgrade = useCallback(
    async (restartAfter: boolean) => {
      setUpgradeConfirmOpen(false);
      setBusy("upgrade");
      setUpgradeLogs([]);
      setUpgradeLogOpen(true);
      try {
        const { taskId } = await startServerUpgrade({ restartAfter });
        streamUpgrade(taskId);
        if (restartAfter) {
          void pollHealthAndReload();
        }
      } catch (reason) {
        toast.error(reason instanceof Error ? reason.message : String(reason));
        setBusy(null);
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

  const restartPending = info?.restartPending ?? false;
  const upgradeDisabled =
    busy !== null || loading || info?.upgradeCapable === false;

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
                  <p className="truncate text-xs text-muted-foreground">
                    {loading
                      ? t("common.loading")
                      : info?.versionLine || info?.version || "--"}
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
                size="icon"
                className="h-9 w-9"
                disabled={loading || busy !== null}
                onClick={() => void refresh()}
                title={t("common.refresh")}
              >
                <RefreshCw className="h-4 w-4" />
              </Button>
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
            <div className="border-t border-border/50 px-4 pb-4 pt-3">
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

      <Dialog open={upgradeLogOpen} onOpenChange={setUpgradeLogOpen}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>
              {t("settings.serverVersion.upgradeLogTitle")}
            </DialogTitle>
          </DialogHeader>
          <div className="max-h-96 overflow-y-auto rounded-lg border bg-slate-950 p-4 font-mono text-xs text-slate-100">
            {upgradeLogs.length > 0 ? (
              <div className="space-y-2">
                {upgradeLogs.map((line, index) => (
                  <div key={`${index}-${line}`}>{line}</div>
                ))}
              </div>
            ) : (
              <div className="text-slate-400">
                {t("settings.serverVersion.waitingLogs")}
              </div>
            )}
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
