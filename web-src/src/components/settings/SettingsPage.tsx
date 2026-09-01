import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { motion } from "framer-motion";
import {
  Loader2,
  LogOut,
  Save,
  FolderSearch,
  ScrollText,
  HardDriveDownload,
  FlaskConical,
  Gauge,
  ShieldCheck,
} from "lucide-react";
import { toast } from "sonner";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { cn } from "@/lib/utils";
import { PAGE_SHELL_PADDING_X } from "@/lib/layout";
import { stableStringify } from "@/lib/stableStringify";
import { LanguageSettings } from "@/components/settings/LanguageSettings";
import { ThemeSettings } from "@/components/settings/ThemeSettings";
import { BackupListSection } from "@/components/settings/BackupListSection";
import { ModelTestConfigPanel } from "@/components/usage/ModelTestConfigPanel";
import { ProviderRuntimeDefaultsPanel } from "@/components/settings/ProviderRuntimeDefaultsPanel";
import { UsageDashboard } from "@/components/usage/UsageDashboard";
import { LogConfigPanel } from "@/components/settings/LogConfigPanel";
import { ApiManagementPanel } from "@/components/settings/ApiManagementPanel";
import { AuthCenterPanel } from "@/components/settings/AuthCenterPanel";
import { ServerSecuritySettings } from "@/components/settings/ServerSecuritySettings";
import { ServerUpgradePolicySettings } from "@/components/settings/ServerUpgradePolicySettings";
import { ServerVersionSettings } from "@/components/settings/ServerVersionSettings";
import { ServerConfigDirSettings } from "@/components/settings/ServerConfigDirSettings";
import {
  ShareSettingsTab,
  type ShareSettingsSaveState,
} from "@/components/settings/ShareSettingsTab";
import { useSettings } from "@/hooks/useSettings";
import { useTranslation } from "react-i18next";
import type { SettingsFormState } from "@/hooks/useSettings";

export type SettingsTab =
  "general" | "proxy" | "auth" | "share" | "advanced" | "usage";

interface SettingsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  defaultTab?: SettingsTab | "router" | "diagnostics" | "tunnel" | "backup";
  onSignOut?: (options?: { clearPasswordCache?: boolean }) => void;
}

export interface SettingsPageHandle {
  requestClose: () => void;
}

type PendingNavigation =
  | { kind: "close" }
  | { kind: "tab"; tab: string };

export const SettingsPage = forwardRef<
  SettingsPageHandle,
  SettingsDialogProps
>(function SettingsPage(
  { open, onOpenChange, defaultTab = "general", onSignOut },
  ref,
) {
  const { t } = useTranslation();
  const {
    settings,
    isLoading,
    isSaving,
    configDir,
    updateSettings,
    saveSettings,
    autoSaveSettings,
    resetSettings,
  } = useSettings();

  const [shareSaveState, setShareSaveState] =
    useState<ShareSettingsSaveState | null>(null);

  const [activeTab, setActiveTab] = useState<string>("general");
  const [pendingNavigation, setPendingNavigation] =
    useState<PendingNavigation | null>(null);
  const tabScrollContainerRef = useRef<HTMLDivElement>(null);
  const advancedBaselineRef = useRef<string | null>(null);
  const advancedFingerprint = useMemo(
    () => stableStringify(settings),
    [settings],
  );
  const advancedDirty =
    activeTab === "advanced" &&
    advancedBaselineRef.current !== null &&
    advancedFingerprint !== advancedBaselineRef.current;

  useEffect(() => {
    if (open) {
      const normalizedTab =
        defaultTab === "proxy" || defaultTab === "tunnel" || defaultTab === "backup"
          ? "advanced"
          : defaultTab === "router" || defaultTab === "diagnostics"
            ? "share"
            : defaultTab;
      setActiveTab(normalizedTab);
      advancedBaselineRef.current = null;
      setPendingNavigation(null);
    }
  }, [open, defaultTab]);

  useEffect(() => {
    if (
      open &&
      activeTab === "advanced" &&
      settings &&
      !isLoading &&
      advancedBaselineRef.current === null
    ) {
      advancedBaselineRef.current = advancedFingerprint;
    }
  }, [activeTab, advancedFingerprint, isLoading, open, settings]);

  useEffect(() => {
    if (!open || !advancedDirty) return;
    const handleBeforeUnload = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", handleBeforeUnload);
    return () => window.removeEventListener("beforeunload", handleBeforeUnload);
  }, [advancedDirty, open]);

  useLayoutEffect(() => {
    if (tabScrollContainerRef.current) {
      tabScrollContainerRef.current.scrollTop = 0;
    }
  }, [activeTab]);

  const closeAfterSave = useCallback(() => {
    onOpenChange(false);
  }, [onOpenChange]);

  const navigateWithoutGuard = useCallback(
    (navigation: PendingNavigation) => {
      advancedBaselineRef.current = null;
      setPendingNavigation(null);
      if (navigation.kind === "tab") {
        setActiveTab(navigation.tab);
        return;
      }
      onOpenChange(false);
    },
    [onOpenChange],
  );

  const requestNavigation = useCallback(
    (navigation: PendingNavigation) => {
      if (advancedDirty) {
        setPendingNavigation(navigation);
        return;
      }
      navigateWithoutGuard(navigation);
    },
    [advancedDirty, navigateWithoutGuard],
  );

  const requestClose = useCallback(
    () => requestNavigation({ kind: "close" }),
    [requestNavigation],
  );

  useImperativeHandle(ref, () => ({ requestClose }), [requestClose]);

  const handleTabChange = useCallback(
    (nextTab: string) => {
      if (nextTab === activeTab) return;
      if (activeTab === "advanced") {
        requestNavigation({ kind: "tab", tab: nextTab });
        return;
      }
      if (nextTab === "advanced") {
        advancedBaselineRef.current = advancedFingerprint;
      }
      setActiveTab(nextTab);
    },
    [activeTab, advancedFingerprint, requestNavigation],
  );

  const discardPendingChanges = useCallback(() => {
    if (!pendingNavigation) return;
    resetSettings();
    navigateWithoutGuard(pendingNavigation);
  }, [navigateWithoutGuard, pendingNavigation, resetSettings]);

  const handleSave = useCallback(async () => {
    try {
      const result = await saveSettings(undefined, { silent: false });
      if (!result) return;
      advancedBaselineRef.current = advancedFingerprint;
      closeAfterSave();
    } catch (error) {
      console.error("[SettingsPage] Failed to save settings", error);
    }
  }, [advancedFingerprint, closeAfterSave, saveSettings]);

  // 通用设置即时保存（无需手动点击）
  // 返回保存是否成功：需要在保存成功后追加动作的调用方（如统一会话历史
  // 关闭后的备份还原）据此短路，其余调用方可忽略返回值。
  const handleAutoSave = useCallback(
    async (updates: Partial<SettingsFormState>): Promise<boolean> => {
      if (!settings) return false;
      // 乐观更新前捕获旧值：autoSaveSettings 发送的是全量表单状态，后端按
      // diff 触发副作用（如统一会话开关的 live 重写与历史迁移）。保存失败
      // 不回滚的话，失败的变更会滞留在表单里，被之后任意一次无关保存原样
      // 重放，绕过确认弹窗。
      const previousValues = Object.fromEntries(
        Object.keys(updates).map((key) => [
          key,
          settings[key as keyof SettingsFormState],
        ]),
      ) as Partial<SettingsFormState>;
      updateSettings(updates);
      try {
        await autoSaveSettings(updates);
        return true;
      } catch (error) {
        console.error("[SettingsPage] Failed to autosave settings", error);
        updateSettings(previousValues);
        toast.error(
          t("settings.saveFailedGeneric", {
            defaultValue: "保存失败，请重试",
          }),
        );
        return false;
      }
    },
    [autoSaveSettings, settings, t, updateSettings],
  );

  const isBusy = useMemo(() => isLoading && !settings, [isLoading, settings]);

  return (
    <div
      className={cn(
        "flex flex-col h-full overflow-hidden",
        PAGE_SHELL_PADDING_X,
      )}
    >
      {isBusy ? (
        <div className="flex flex-1 items-center justify-center">
          <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" />
        </div>
      ) : (
        <Tabs
          value={activeTab}
          onValueChange={handleTabChange}
          className="flex flex-col h-full"
        >
          <TabsList
            className={cn("grid w-full mb-6 glass rounded-lg", "grid-cols-5")}
          >
            <TabsTrigger value="general">
              {t("settings.tabGeneral")}
            </TabsTrigger>
            <TabsTrigger value="auth">
              {t("settings.tabAuth", { defaultValue: "认证" })}
            </TabsTrigger>
            <TabsTrigger value="share">
              {t("settings.tabShare", { defaultValue: "分享" })}
            </TabsTrigger>
            <TabsTrigger value="advanced">
              {t("settings.tabAdvanced")}
            </TabsTrigger>
            <TabsTrigger value="usage">{t("usage.title")}</TabsTrigger>
          </TabsList>

          <div className="flex-1 min-h-0 flex flex-col">
            <div
              ref={tabScrollContainerRef}
              className="flex-1 overflow-y-auto overflow-x-hidden pr-2"
            >
              <TabsContent value="general" className="space-y-6 mt-0">
                {settings ? (
                  <motion.div
                    initial={{ opacity: 0, y: 10 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={{ duration: 0.3 }}
                    className="space-y-6"
                  >
                    <LanguageSettings
                      value={settings.language}
                      onChange={(lang) => handleAutoSave({ language: lang })}
                    />
                    <ThemeSettings />
                    <ServerSecuritySettings />
                    <ServerVersionSettings />
                    <ServerUpgradePolicySettings />
                  </motion.div>
                ) : (
                  <p className="text-sm text-muted-foreground">
                    {t("settings.loadFailed", {
                      defaultValue: "无法加载设置，请刷新页面后重试。",
                    })}
                  </p>
                )}
              </TabsContent>

              <TabsContent value="auth" className="space-y-6 mt-0 pb-4">
                <motion.div
                  initial={{ opacity: 0, y: 10 }}
                  animate={{ opacity: 1, y: 0 }}
                  transition={{ duration: 0.3 }}
                  className="space-y-6"
                >
                  <AuthCenterPanel serverMode />
                </motion.div>
              </TabsContent>

              <TabsContent value="share" className="space-y-6 mt-0 pb-4">
                <motion.div
                  initial={{ opacity: 0, y: 10 }}
                  animate={{ opacity: 1, y: 0 }}
                  transition={{ duration: 0.3 }}
                >
                  <ShareSettingsTab onSaveStateChange={setShareSaveState} />
                </motion.div>
              </TabsContent>

              <TabsContent value="advanced" className="space-y-6 mt-0 pb-4">
                {settings ? (
                  <motion.div
                    initial={{ opacity: 0, y: 10 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={{ duration: 0.3 }}
                    className="space-y-4"
                  >
                    <Accordion
                      type="multiple"
                      defaultValue={[]}
                      className="w-full space-y-4"
                    >
                      <AccordionItem
                        value="providerRequestDefaults"
                        className="rounded-xl glass-card overflow-hidden"
                      >
                          <AccordionTrigger className="px-6 py-4 hover:no-underline hover:bg-muted/50 data-[state=open]:bg-muted/50">
                            <div className="flex min-w-0 flex-1 items-center gap-3">
                              <Gauge className="h-5 w-5 shrink-0 text-sky-500" />
                              <div className="min-w-0 space-y-1 text-left">
                                <h3 className="text-sm font-medium leading-none">
                                  {t("settings.advanced.providerDefaults.title", {
                                    defaultValue: "供应商请求默认值",
                                  })}
                                </h3>
                                <p className="text-xs font-normal text-muted-foreground">
                                  {t(
                                    "settings.advanced.providerDefaults.description",
                                    {
                                      defaultValue:
                                        "设置供应商请求、首字节和流空闲超时的 Server 默认值",
                                    },
                                  )}
                                </p>
                              </div>
                            </div>
                          </AccordionTrigger>
                          <AccordionContent className="px-6 pb-6 pt-4 border-t border-border/50">
                            <ProviderRuntimeDefaultsPanel />
                          </AccordionContent>
                      </AccordionItem>

                      <AccordionItem
                        value="directory"
                        className="rounded-xl glass-card overflow-hidden"
                      >
                        <AccordionTrigger className="px-6 py-4 hover:no-underline hover:bg-muted/50 data-[state=open]:bg-muted/50">
                          <div className="flex min-w-0 flex-1 items-center gap-3">
                            <FolderSearch className="h-5 w-5 shrink-0 text-primary" />
                            <div className="min-w-0 space-y-1 text-left">
                              <h3 className="text-sm font-medium leading-none">
                                {t("settings.serverConfigDir.title", {
                                  defaultValue: "Server 配置目录",
                                })}
                              </h3>
                              <p className="text-xs font-normal text-muted-foreground">
                                {t("settings.serverConfigDir.description", {
                                  defaultValue:
                                    "持久化数据目录（监听地址由启动参数配置）",
                                })}
                              </p>
                            </div>
                          </div>
                        </AccordionTrigger>
                        <AccordionContent className="px-6 pb-6 pt-4 border-t border-border/50">
                          <ServerConfigDirSettings
                            configDir={configDir}
                          />
                        </AccordionContent>
                      </AccordionItem>

                      <AccordionItem
                        value="backup"
                        className="rounded-xl glass-card overflow-hidden"
                      >
                        <AccordionTrigger className="px-6 py-4 hover:no-underline hover:bg-muted/50 data-[state=open]:bg-muted/50">
                          <div className="flex min-w-0 flex-1 items-center gap-3">
                            <HardDriveDownload className="h-5 w-5 shrink-0 text-amber-500" />
                            <div className="min-w-0 space-y-1 text-left">
                              <h3 className="text-sm font-medium leading-none">
                                {t("settings.advanced.backup.title", {
                                  defaultValue: "Backup & Restore",
                                })}
                              </h3>
                              <p className="text-xs font-normal text-muted-foreground">
                                {t("settings.advanced.backup.description", {
                                  defaultValue:
                                    "Manage state snapshots for this installation; migrate hosts by copying the complete stopped data directory",
                                })}
                              </p>
                            </div>
                          </div>
                        </AccordionTrigger>
                        <AccordionContent className="px-6 pb-6 pt-4 border-t border-border/50">
                          <BackupListSection
                            backupIntervalHours={settings.backupIntervalHours}
                            backupRetainCount={settings.backupRetainCount}
                            onSettingsChange={(updates) =>
                              handleAutoSave(updates)
                            }
                          />
                        </AccordionContent>
                      </AccordionItem>

                      <AccordionItem
                        value="test"
                        className="rounded-xl glass-card overflow-hidden"
                      >
                        <AccordionTrigger className="px-6 py-4 hover:no-underline hover:bg-muted/50 data-[state=open]:bg-muted/50">
                          <div className="flex min-w-0 flex-1 items-center gap-3">
                            <FlaskConical className="h-5 w-5 shrink-0 text-emerald-500" />
                            <div className="min-w-0 space-y-1 text-left">
                              <h3 className="text-sm font-medium leading-none">
                                {t("settings.advanced.modelTest.title")}
                              </h3>
                              <p className="text-xs font-normal text-muted-foreground">
                                {t("settings.advanced.modelTest.description")}
                              </p>
                            </div>
                          </div>
                        </AccordionTrigger>
                        <AccordionContent className="px-6 pb-6 pt-4 border-t border-border/50">
                          <ModelTestConfigPanel />
                        </AccordionContent>
                      </AccordionItem>

                      <AccordionItem
                        value="logConfig"
                        className="rounded-xl glass-card overflow-hidden"
                      >
                        <AccordionTrigger className="px-6 py-4 hover:no-underline hover:bg-muted/50 data-[state=open]:bg-muted/50">
                          <div className="flex min-w-0 flex-1 items-center gap-3">
                            <ScrollText className="h-5 w-5 shrink-0 text-cyan-500" />
                            <div className="min-w-0 space-y-1 text-left">
                              <h3 className="text-sm font-medium leading-none">
                                {t("settings.advanced.logConfig.title")}
                              </h3>
                              <p className="text-xs font-normal text-muted-foreground">
                                {t("settings.advanced.logConfig.description")}
                              </p>
                            </div>
                          </div>
                        </AccordionTrigger>
                        <AccordionContent className="px-6 pb-6 pt-4 border-t border-border/50">
                          <LogConfigPanel />
                        </AccordionContent>
                      </AccordionItem>

                      <AccordionItem
                        value="apiManagement"
                        className="rounded-xl glass-card overflow-hidden"
                      >
                        <AccordionTrigger className="px-6 py-4 hover:no-underline hover:bg-muted/50 data-[state=open]:bg-muted/50">
                          <div className="flex min-w-0 flex-1 items-center gap-3">
                            <ShieldCheck className="h-5 w-5 shrink-0 text-amber-500" />
                            <div className="min-w-0 space-y-1 text-left">
                              <h3 className="text-sm font-medium leading-none">
                                {t("settings.advanced.apiManagement.title")}
                              </h3>
                              <p className="text-xs font-normal text-muted-foreground">
                                {t(
                                  "settings.advanced.apiManagement.description",
                                )}
                              </p>
                            </div>
                          </div>
                        </AccordionTrigger>
                        <AccordionContent className="px-6 pb-6 pt-4 border-t border-border/50">
                          <ApiManagementPanel />
                        </AccordionContent>
                      </AccordionItem>
                    </Accordion>
                  </motion.div>
                ) : (
                  <p className="text-sm text-muted-foreground">
                    {t("settings.loadFailed", {
                      defaultValue: "无法加载设置，请刷新页面后重试。",
                    })}
                  </p>
                )}
              </TabsContent>

              <TabsContent value="usage" className="mt-0">
                <UsageDashboard />
              </TabsContent>
            </div>

            {activeTab === "general" && onSignOut ? (
              <div
                className="flex-shrink-0 pt-4 border-t border-border-default"
                style={{ backgroundColor: "hsl(var(--background))" }}
              >
                <div className="flex items-center justify-end gap-3">
                  <Button
                    type="button"
                    variant="destructive"
                    onClick={() => onSignOut()}
                  >
                    <LogOut className="mr-2 h-4 w-4" />
                    {t("settings.serverSecurity.signOut", {
                      defaultValue: "登出",
                    })}
                  </Button>
                </div>
              </div>
            ) : null}
            {activeTab === "share" && shareSaveState ? (
              <div
                className="flex-shrink-0 pt-4 border-t border-border-default"
                style={{ backgroundColor: "hsl(var(--background))" }}
              >
                <div className="flex items-center justify-end gap-3">
                  <Button
                    onClick={() => void shareSaveState.save()}
                    disabled={
                      !shareSaveState.canSave || shareSaveState.isSaving
                    }
                  >
                    {shareSaveState.isSaving ? (
                      <span className="inline-flex items-center gap-2">
                        <Loader2 className="h-4 w-4 animate-spin" />
                        {t("settings.saving")}
                      </span>
                    ) : (
                      <>
                        <Save className="mr-2 h-4 w-4" />
                        {t("common.save")}
                      </>
                    )}
                  </Button>
                </div>
              </div>
            ) : null}
            {activeTab === "advanced" && settings && (
              <div
                className="flex-shrink-0 pt-4 border-t border-border-default"
                style={{ backgroundColor: "hsl(var(--background))" }}
              >
                <div className="flex items-center justify-end gap-3">
                  <Button
                    onClick={handleSave}
                    disabled={isSaving || !advancedDirty}
                  >
                    {isSaving ? (
                      <span className="inline-flex items-center gap-2">
                        <Loader2 className="h-4 w-4 animate-spin" />
                        {t("settings.saving")}
                      </span>
                    ) : (
                      <>
                        <Save className="mr-2 h-4 w-4" />
                        {t("common.save")}
                      </>
                    )}
                  </Button>
                </div>
              </div>
            )}
          </div>
        </Tabs>
      )}

      <ConfirmDialog
        isOpen={pendingNavigation !== null}
        title={t("settings.unsavedChanges.title")}
        message={t("settings.unsavedChanges.message")}
        confirmText={t("settings.unsavedChanges.discard")}
        cancelText={t("settings.unsavedChanges.keepEditing")}
        onConfirm={discardPendingChanges}
        onCancel={() => setPendingNavigation(null)}
      />
    </div>
  );
});
