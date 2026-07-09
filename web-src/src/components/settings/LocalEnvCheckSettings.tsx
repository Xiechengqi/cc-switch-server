import { useCallback, useEffect, useMemo, useState } from "react";
import {
  ArrowUpCircle,
  CheckCircle2,
  AlertCircle,
  ChevronDown,
  Copy,
  Download,
  Loader2,
  RefreshCw,
  Stethoscope,
  Terminal,
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
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { APP_ICON_MAP } from "@/config/appConfig";
import { settingsApi } from "@/lib/api";
import type { ToolInstallation, ToolInstallationReport } from "@/lib/api/settings";
import { cn } from "@/lib/utils";
import { extractErrorMessage } from "@/utils/errorUtils";
import { isWindows } from "@/lib/platform";
import { isUpdateAvailable } from "@/lib/version";
import { ToolInstallRow } from "@/components/settings/ToolInstallRow";
import { ToolUpgradeConfirmDialog } from "@/components/settings/ToolUpgradeConfirmDialog";
import {
  ENV_BADGE_CONFIG,
  getToolVersionsCache,
  mergeToolVersions,
  ONE_CLICK_INSTALL_COMMANDS,
  TOOL_APP_IDS,
  TOOL_DISPLAY_NAMES,
  TOOL_NAMES,
  TOOL_VERSIONS_CACHE_TTL_MS,
  toolDisplayName,
  updateToolVersionsCache,
  type ToolLifecycleAction,
  type ToolName,
  type ToolVersion,
  type WslShellPreference,
  WSL_SHELL_FLAG_OPTIONS,
  WSL_SHELL_OPTIONS,
} from "@/components/settings/localEnvCheckShared";

export function LocalEnvCheckSettings() {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [toolVersions, setToolVersions] = useState<ToolVersion[]>(
    () => getToolVersionsCache()?.data ?? [],
  );
  const [isLoadingTools, setIsLoadingTools] = useState(
    () => getToolVersionsCache() === null,
  );
  const [toolActions, setToolActions] = useState<
    Partial<Record<ToolName, ToolLifecycleAction>>
  >({});
  const [batchAction, setBatchAction] = useState<ToolLifecycleAction | null>(
    null,
  );
  const [wslShellByTool, setWslShellByTool] = useState<
    Record<string, WslShellPreference>
  >({});
  const [loadingTools, setLoadingTools] = useState<Record<string, boolean>>({});
  const [toolDiagnostics, setToolDiagnostics] = useState<
    Partial<Record<ToolName, ToolInstallation[]>>
  >({});
  const [isDiagnosingAll, setIsDiagnosingAll] = useState(false);
  const [pendingUpgrade, setPendingUpgrade] = useState<{
    toolNames: ToolName[];
    plans: ToolInstallationReport[];
  } | null>(null);
  const [preflightTools, setPreflightTools] = useState<Set<ToolName>>(
    () => new Set(),
  );

  const toolVersionByName = useMemo(
    () => new Map(toolVersions.map((tool) => [tool.name, tool])),
    [toolVersions],
  );

  const updatableToolNames = useMemo(
    () =>
      TOOL_NAMES.filter((toolName) => {
        const tool = toolVersionByName.get(toolName);
        return isUpdateAvailable(tool?.version, tool?.latest_version);
      }),
    [toolVersionByName],
  );

  const subtitle = useMemo(() => {
    if (isLoadingTools) {
      return t("common.loading");
    }
    if (updatableToolNames.length > 0) {
      return t("settings.updateAllTools", { count: updatableToolNames.length });
    }
    const installedCount = TOOL_NAMES.filter((name) =>
      Boolean(toolVersionByName.get(name)?.version),
    ).length;
    return t("settings.localEnvCheckSummary", {
      installed: installedCount,
      total: TOOL_NAMES.length,
    });
  }, [isLoadingTools, t, toolVersionByName, updatableToolNames.length]);

  const refreshToolVersions = useCallback(
    async (
      toolNames: ToolName[],
      wslOverrides?: Record<string, WslShellPreference>,
    ): Promise<ToolVersion[]> => {
      if (toolNames.length === 0) return [];

      setLoadingTools((prev) => {
        const next = { ...prev };
        for (const name of toolNames) next[name] = true;
        return next;
      });

      try {
        const updated = await settingsApi.getToolVersions(
          toolNames,
          wslOverrides,
        );
        setToolVersions((prev) => mergeToolVersions(prev, updated));
        updateToolVersionsCache((current) => ({
          data: mergeToolVersions(current?.data ?? [], updated),
          at: current?.at ?? 0,
        }));
        return updated;
      } catch (error) {
        console.error("[LocalEnvCheckSettings] Failed to refresh tools", error);
        return [];
      } finally {
        setLoadingTools((prev) => {
          const next = { ...prev };
          for (const name of toolNames) next[name] = false;
          return next;
        });
      }
    },
    [],
  );

  const loadAllToolVersions = useCallback(
    async (options?: { force?: boolean }) => {
      const force = options?.force ?? false;
      const cache = getToolVersionsCache();
      if (
        !force &&
        cache &&
        Date.now() - cache.at < TOOL_VERSIONS_CACHE_TTL_MS
      ) {
        setToolVersions(cache.data);
        setIsLoadingTools(false);
        return;
      }
      setIsLoadingTools(true);
      try {
        await Promise.all(
          TOOL_NAMES.map((toolName) =>
            refreshToolVersions([toolName], wslShellByTool),
          ),
        );
      } finally {
        updateToolVersionsCache((current) =>
          current ? { ...current, at: Date.now() } : current,
        );
        setIsLoadingTools(false);
      }
    },
    [wslShellByTool, refreshToolVersions],
  );

  const handleToolShellChange = async (toolName: ToolName, value: string) => {
    const wslShell = value === "auto" ? null : value;
    const nextPref: WslShellPreference = {
      ...(wslShellByTool[toolName] ?? {}),
      wslShell,
    };
    setWslShellByTool((prev) => ({ ...prev, [toolName]: nextPref }));
    await refreshToolVersions([toolName], { [toolName]: nextPref });
  };

  const handleToolShellFlagChange = async (
    toolName: ToolName,
    value: string,
  ) => {
    const wslShellFlag = value === "auto" ? null : value;
    const nextPref: WslShellPreference = {
      ...(wslShellByTool[toolName] ?? {}),
      wslShellFlag,
    };
    setWslShellByTool((prev) => ({ ...prev, [toolName]: nextPref }));
    await refreshToolVersions([toolName], { [toolName]: nextPref });
  };

  useEffect(() => {
    if (!isWindows()) {
      void loadAllToolVersions();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleCopyInstallCommands = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(ONE_CLICK_INSTALL_COMMANDS);
      toast.success(t("settings.installCommandsCopied"), { closeButton: true });
    } catch (error) {
      console.error(
        "[LocalEnvCheckSettings] Failed to copy install commands",
        error,
      );
      toast.error(t("settings.installCommandsCopyFailed"));
    }
  }, [t]);

  const diagnoseToolSilently = useCallback(async (toolName: ToolName) => {
    try {
      const [report] = await settingsApi.probeToolInstallations([toolName]);
      setToolDiagnostics((prev) => {
        if (report?.is_conflict) {
          return { ...prev, [toolName]: report.installs };
        }
        if (!(toolName in prev)) return prev;
        const next = { ...prev };
        delete next[toolName];
        return next;
      });
    } catch (error) {
      console.error(
        `[LocalEnvCheckSettings] Auto-diagnose failed for ${toolName}`,
        error,
      );
    }
  }, []);

  const handleDiagnoseAll = useCallback(async () => {
    setIsDiagnosingAll(true);
    try {
      const reports = await settingsApi.probeToolInstallations([...TOOL_NAMES]);
      const next: Partial<Record<ToolName, ToolInstallation[]>> = {};
      let conflicts = 0;
      for (const report of reports) {
        if (report.is_conflict) {
          next[report.tool as ToolName] = report.installs;
          conflicts += 1;
        }
      }
      setToolDiagnostics(next);
      if (conflicts === 0) {
        toast.info(t("settings.toolDiagnoseNoConflict"), { closeButton: true });
      }
    } catch (error) {
      console.error("[LocalEnvCheckSettings] Diagnose all failed", error);
      toast.error(t("settings.toolDiagnoseFailed"), {
        description: extractErrorMessage(error) || undefined,
        closeButton: true,
      });
    } finally {
      setIsDiagnosingAll(false);
    }
  }, [t]);

  const executeRun = useCallback(
    async (toolNames: ToolName[], action: ToolLifecycleAction) => {
      const isBatch = toolNames.length > 1;
      if (isBatch) {
        setBatchAction(action);
      }

      const failures: {
        toolName: ToolName;
        detail: string;
        soft: boolean;
        kind?: "notRunnable" | "versionUnchanged";
      }[] = [];
      let succeeded = 0;

      for (const toolName of toolNames) {
        setToolActions((prev) => ({ ...prev, [toolName]: action }));
        try {
          const previousTool = toolVersionByName.get(toolName);
          const previousVersion = previousTool?.version ?? null;
          const previousLatestVersion = previousTool?.latest_version ?? null;

          await settingsApi.runToolLifecycleAction(
            [toolName],
            action,
            wslShellByTool,
          );
          const refreshed = await refreshToolVersions(
            [toolName],
            wslShellByTool,
          );
          const tool = refreshed.find((entry) => entry.name === toolName);
          if (tool?.version) {
            const latestVersion = tool.latest_version ?? previousLatestVersion;
            const versionUnchangedAfterUpdate =
              action === "update" &&
              Boolean(previousVersion) &&
              tool.version === previousVersion &&
              isUpdateAvailable(tool.version, latestVersion);

            if (versionUnchangedAfterUpdate) {
              failures.push({
                toolName,
                detail: t("settings.toolActionVersionUnchanged", {
                  version: tool.version,
                  latest: latestVersion ?? t("common.unknown"),
                }),
                soft: true,
                kind: "versionUnchanged",
              });
              void diagnoseToolSilently(toolName);
            } else {
              succeeded += 1;
              if (action === "update") {
                void diagnoseToolSilently(toolName);
              }
            }
          } else {
            const detail = tool?.error?.trim() || t("settings.toolNotRunnable");
            failures.push({
              toolName,
              detail,
              soft: true,
              kind: "notRunnable",
            });
            void diagnoseToolSilently(toolName);
          }
        } catch (error) {
          console.error(
            `[LocalEnvCheckSettings] Failed to run tool action for ${toolName}`,
            error,
          );
          failures.push({
            toolName,
            detail: extractErrorMessage(error) || String(error),
            soft: false,
          });
        } finally {
          setToolActions((prev) => {
            const next = { ...prev };
            delete next[toolName];
            return next;
          });
        }
      }

      if (isBatch) {
        setBatchAction(null);
      }

      const actionLabel =
        action === "install"
          ? t("settings.toolInstall")
          : t("settings.toolUpdate");

      if (failures.length === 0) {
        toast.success(
          t("settings.toolActionDone", {
            count: succeeded,
            action: actionLabel,
          }),
          { closeButton: true },
        );
        return;
      }

      const lastLine = (text: string) => {
        const lines = text.trim().split("\n").filter(Boolean);
        return lines[lines.length - 1] ?? text;
      };
      const failureDescription = isBatch
        ? failures
            .map(
              (f) => `${TOOL_DISPLAY_NAMES[f.toolName]}: ${lastLine(f.detail)}`,
            )
            .join("\n")
        : failures[0]?.detail;

      const hardFailures = failures.filter((f) => !f.soft);
      const allSoftVersionUnchanged =
        failures.length > 0 &&
        failures.every((f) => f.soft && f.kind === "versionUnchanged");

      if (succeeded === 0 && hardFailures.length === 0) {
        toast.warning(
          allSoftVersionUnchanged
            ? t("settings.toolActionVersionUnchangedTitle")
            : t("settings.toolActionInstalledNotRunnable"),
          {
            description: failureDescription || undefined,
            closeButton: true,
          },
        );
      } else if (succeeded === 0) {
        toast.error(t("settings.toolActionFailed"), {
          description: failureDescription || undefined,
          closeButton: true,
        });
      } else {
        toast.warning(
          t("settings.toolActionPartial", {
            succeeded,
            failed: failures.length,
            action: actionLabel,
          }),
          { description: failureDescription || undefined, closeButton: true },
        );
      }
    },
    [diagnoseToolSilently, refreshToolVersions, t, toolVersionByName, wslShellByTool],
  );

  const handleRunToolAction = useCallback(
    async (toolNames: ToolName[], action: ToolLifecycleAction) => {
      if (toolNames.length === 0) return;
      if (
        toolNames.some(
          (name) => preflightTools.has(name) || toolActions[name] !== undefined,
        )
      ) {
        return;
      }
      setPreflightTools((prev) => {
        const next = new Set(prev);
        toolNames.forEach((name) => next.add(name));
        return next;
      });
      try {
        if (action === "install") {
          await executeRun(toolNames, action);
          return;
        }
        let reports: ToolInstallationReport[];
        try {
          reports = await settingsApi.probeToolInstallations(toolNames);
        } catch (error) {
          console.error(
            "[LocalEnvCheckSettings] probeToolInstallations failed",
            error,
          );
          await executeRun(toolNames, action);
          return;
        }
        const needConfirm = reports.filter((r) => r.needs_confirmation);
        if (needConfirm.length === 0) {
          await executeRun(toolNames, action);
          return;
        }
        setPendingUpgrade({ toolNames, plans: needConfirm });
      } finally {
        setPreflightTools((prev) => {
          const next = new Set(prev);
          toolNames.forEach((name) => next.delete(name));
          return next;
        });
      }
    },
    [executeRun, preflightTools, toolActions],
  );

  const handleConfirmUpgrade = useCallback(() => {
    if (pendingUpgrade) {
      void executeRun(pendingUpgrade.toolNames, "update");
    }
    setPendingUpgrade(null);
  }, [pendingUpgrade, executeRun]);

  const isAnyBusy =
    Boolean(batchAction) ||
    Object.keys(toolActions).length > 0 ||
    preflightTools.size > 0;

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
                  <Terminal className="h-4 w-4 text-emerald-500" />
                </div>
                <div className="min-w-0 space-y-1">
                  <p className="text-sm font-medium leading-none">
                    {t("settings.localEnvCheck")}
                  </p>
                  <p className="truncate text-xs text-muted-foreground">
                    {subtitle}
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
                size="icon"
                className="h-9 w-9"
                disabled={isLoadingTools || isAnyBusy || isDiagnosingAll}
                onClick={() => void handleDiagnoseAll()}
                title={t("settings.toolDiagnose")}
              >
                {isDiagnosingAll ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  <Stethoscope className="h-4 w-4" />
                )}
              </Button>
              <Button
                type="button"
                variant="outline"
                size="icon"
                className="h-9 w-9"
                disabled={isLoadingTools || isAnyBusy}
                onClick={() => void loadAllToolVersions({ force: true })}
                title={t("common.refresh")}
              >
                <RefreshCw
                  className={cn(
                    "h-4 w-4",
                    isLoadingTools && "animate-spin",
                  )}
                />
              </Button>
              <Button
                type="button"
                size="sm"
                className="h-9"
                onClick={() => void handleRunToolAction(updatableToolNames, "update")}
                disabled={
                  isLoadingTools || isAnyBusy || updatableToolNames.length === 0
                }
              >
                {batchAction === "update" ? (
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                ) : (
                  <ArrowUpCircle className="mr-2 h-4 w-4" />
                )}
                {t("settings.updateAllTools", {
                  count: updatableToolNames.length,
                })}
              </Button>
            </div>
          </div>

          <CollapsibleContent>
            <div className="space-y-4 border-t border-border/50 px-4 pb-4 pt-3">
              <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
                {TOOL_NAMES.map((toolName) => {
                  const tool = toolVersionByName.get(toolName);
                  const appConfig = APP_ICON_MAP[TOOL_APP_IDS[toolName]];
                  const displayName = TOOL_DISPLAY_NAMES[toolName];
                  const isToolVersionLoading =
                    Boolean(loadingTools[toolName]) ||
                    (isLoadingTools && !toolVersionByName.has(toolName));
                  const isOutdated = isUpdateAvailable(
                    tool?.version,
                    tool?.latest_version,
                  );
                  const installedButBroken = Boolean(tool?.installed_but_broken);
                  const action: ToolLifecycleAction | null =
                    isToolVersionLoading || installedButBroken
                      ? null
                      : !tool?.version
                        ? "install"
                        : isOutdated
                          ? "update"
                          : null;
                  const runningAction = toolActions[toolName];
                  const title =
                    tool?.version || tool?.error || t("common.unknown");
                  const conflicts = toolDiagnostics[toolName];

                  return (
                    <div
                      key={toolName}
                      className="flex min-h-[150px] flex-col gap-3 rounded-xl border border-border bg-gradient-to-br from-card/80 to-card/40 p-4 shadow-sm transition-colors hover:border-primary/30"
                    >
                      <div className="flex items-start justify-between gap-3">
                        <div className="flex min-w-0 items-center gap-2">
                          <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-background/80 text-muted-foreground">
                            {appConfig?.icon ?? <Terminal className="h-4 w-4" />}
                          </span>
                          <div className="min-w-0">
                            <div className="truncate text-sm font-medium">
                              {displayName}
                            </div>
                            {tool?.env_type && ENV_BADGE_CONFIG[tool.env_type] && (
                              <span
                                className={`mt-1 inline-flex w-fit text-[9px] px-1.5 py-0.5 rounded-full border ${ENV_BADGE_CONFIG[tool.env_type].className}`}
                              >
                                {t(ENV_BADGE_CONFIG[tool.env_type].labelKey)}
                                {tool.wsl_distro ? ` · ${tool.wsl_distro}` : ""}
                              </span>
                            )}
                          </div>
                        </div>
                        {isToolVersionLoading ? (
                          <Loader2 className="mt-1 h-4 w-4 animate-spin text-muted-foreground" />
                        ) : tool?.version ? (
                          isOutdated ? (
                            <span className="mt-1 shrink-0 rounded-full border border-yellow-500/20 bg-yellow-500/10 px-1.5 py-0.5 text-[10px] text-yellow-600 dark:text-yellow-400">
                              {t("settings.updateAvailableShort")}
                            </span>
                          ) : (
                            <CheckCircle2 className="mt-1 h-4 w-4 shrink-0 text-green-500" />
                          )
                        ) : (
                          <AlertCircle className="mt-1 h-4 w-4 shrink-0 text-yellow-500" />
                        )}
                      </div>

                      <div className="space-y-1.5 text-xs">
                        <div className="flex items-center justify-between gap-3">
                          <span className="text-muted-foreground">
                            {t("settings.currentVersion")}
                          </span>
                          <span
                            className="min-w-0 truncate font-mono text-foreground"
                            title={title}
                          >
                            {isToolVersionLoading
                              ? t("common.loading")
                              : tool?.version
                                ? tool.version
                                : installedButBroken
                                  ? t("settings.installedNotRunnable")
                                  : t("common.notInstalled")}
                          </span>
                        </div>
                        <div className="flex items-center justify-between gap-3">
                          <span className="text-muted-foreground">
                            {t("settings.latestVersion")}
                          </span>
                          <span className="min-w-0 truncate font-mono text-foreground">
                            {isToolVersionLoading
                              ? t("common.loading")
                              : tool?.latest_version || t("common.unknown")}
                          </span>
                        </div>
                        {!isToolVersionLoading && !tool?.version && tool?.error && (
                          <div className="truncate text-[11px] text-muted-foreground">
                            {tool.error}
                          </div>
                        )}
                      </div>

                      {tool?.env_type === "wsl" && (
                        <div className="flex flex-wrap gap-2">
                          <Select
                            value={wslShellByTool[toolName]?.wslShell || "auto"}
                            onValueChange={(v) => void handleToolShellChange(toolName, v)}
                            disabled={isToolVersionLoading || isAnyBusy}
                          >
                            <SelectTrigger className="h-7 w-[82px] text-xs">
                              <SelectValue />
                            </SelectTrigger>
                            <SelectContent>
                              <SelectItem value="auto">{t("common.auto")}</SelectItem>
                              {WSL_SHELL_OPTIONS.map((shell) => (
                                <SelectItem key={shell} value={shell}>
                                  {shell}
                                </SelectItem>
                              ))}
                            </SelectContent>
                          </Select>
                          <Select
                            value={wslShellByTool[toolName]?.wslShellFlag || "auto"}
                            onValueChange={(v) =>
                              void handleToolShellFlagChange(toolName, v)
                            }
                            disabled={isToolVersionLoading || isAnyBusy}
                          >
                            <SelectTrigger className="h-7 w-[82px] text-xs">
                              <SelectValue />
                            </SelectTrigger>
                            <SelectContent>
                              <SelectItem value="auto">{t("common.auto")}</SelectItem>
                              {WSL_SHELL_FLAG_OPTIONS.map((flag) => (
                                <SelectItem key={flag} value={flag}>
                                  {flag}
                                </SelectItem>
                              ))}
                            </SelectContent>
                          </Select>
                        </div>
                      )}

                      {conflicts && conflicts.length > 0 && (
                        <div className="space-y-1.5 rounded-lg border border-yellow-500/20 bg-yellow-500/5 p-2.5">
                          <div className="text-[11px] font-medium text-yellow-600 dark:text-yellow-400">
                            {t("settings.toolConflictTitle")}
                          </div>
                          <p className="text-[10px] leading-snug text-muted-foreground">
                            {t("settings.toolConflictHint")}
                          </p>
                          <ul className="space-y-1.5">
                            {conflicts.map((inst) => (
                              <li key={inst.path}>
                                <ToolInstallRow inst={inst} />
                              </li>
                            ))}
                          </ul>
                        </div>
                      )}

                      <div className="mt-auto flex items-center justify-end">
                        {isToolVersionLoading ? (
                          <span className="text-xs text-muted-foreground">
                            {t("common.loading")}
                          </span>
                        ) : installedButBroken ? (
                          <span className="text-xs text-yellow-600 dark:text-yellow-400">
                            {t("settings.toolCheckEnv")}
                          </span>
                        ) : action ? (
                          <Button
                            size="sm"
                            variant={action === "install" ? "outline" : "default"}
                            className="h-7 gap-1.5 text-xs"
                            onClick={() => void handleRunToolAction([toolName], action)}
                            disabled={isToolVersionLoading || isAnyBusy}
                          >
                            {runningAction ? (
                              <Loader2 className="h-3.5 w-3.5 animate-spin" />
                            ) : action === "install" ? (
                              <Download className="h-3.5 w-3.5" />
                            ) : (
                              <ArrowUpCircle className="h-3.5 w-3.5" />
                            )}
                            {action === "install"
                              ? t("settings.toolInstall")
                              : t("settings.toolUpdate")}
                          </Button>
                        ) : (
                          <span className="text-xs text-muted-foreground">
                            {t("settings.toolReady")}
                          </span>
                        )}
                      </div>
                    </div>
                  );
                })}
              </div>

              <div className="space-y-3 rounded-lg border border-border/60 bg-muted/20 p-4">
                <div className="flex items-center justify-between gap-2">
                  <p className="text-sm font-medium">
                    {t("settings.manualInstallCommands")}
                  </p>
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() => void handleCopyInstallCommands()}
                    className="h-7 gap-1.5 text-xs"
                  >
                    <Copy className="h-3.5 w-3.5" />
                    {t("common.copy")}
                  </Button>
                </div>
                <p className="text-xs text-muted-foreground">
                  {t("settings.oneClickInstallHint")}
                </p>
                <pre className="overflow-x-auto rounded-lg border border-border/60 bg-background/80 px-3 py-2.5 font-mono text-xs">
                  {ONE_CLICK_INSTALL_COMMANDS}
                </pre>
              </div>
            </div>
          </CollapsibleContent>
        </div>
      </Collapsible>

      <ToolUpgradeConfirmDialog
        isOpen={pendingUpgrade !== null}
        plans={pendingUpgrade?.plans ?? []}
        displayName={toolDisplayName}
        onConfirm={handleConfirmUpgrade}
        onCancel={() => setPendingUpgrade(null)}
      />
    </>
  );
}
