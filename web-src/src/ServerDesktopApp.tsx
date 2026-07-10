import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion } from "framer-motion";
import { toast } from "sonner";
import {
  ArrowLeft,
  BarChart2,
  Plus,
  Settings,
  Share2,
} from "lucide-react";
import type { Provider } from "@/types";
import type { AppId } from "@/lib/api";
import { useProvidersQuery, useSettingsQuery } from "@/lib/query";
import { useProxyStatus, useProxyTakeoverStatus } from "@/lib/query/proxy";
import { useProviderActions } from "@/hooks/useProviderActions";
import { useOauthQuotaRefreshBridge } from "@/hooks/useOauthQuotaRefreshBridge";
import { useLastValidValue } from "@/hooks/useLastValidValue";
import { extractErrorMessage } from "@/utils/errorUtils";
import { deepClone } from "@/utils/deepClone";
import { cn } from "@/lib/utils";
import { isRemoteWebMode } from "@/lib/api/auth";
import { clearRouterSessionTokens } from "@/lib/routerAuth";
import { writeCachedPassword, writeToken } from "@/lib/runtime";
import { PAGE_SHELL_CLASS } from "@/lib/layout";
import { ProviderList } from "@/components/providers/ProviderList";
import { AddProviderDialog } from "@/components/providers/AddProviderDialog";
import { EditProviderDialog } from "@/components/providers/EditProviderDialog";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { AppSwitcher } from "@/components/AppSwitcher";
import {
  SettingsPage,
  type SettingsTab,
} from "@/components/settings/SettingsPage";
import { SharePage } from "@/components/share/SharePage";
import { FailoverToggle } from "@/components/proxy/FailoverToggle";
import { Button } from "@/components/ui/button";
import type { VisibleApps } from "@/types";

type View = "providers" | "shares" | "settings";

const VIEW_STORAGE_KEY = "cc-switch-server-view";
const APP_STORAGE_KEY = "cc-switch-active-app";

function isServerApp(value: string | null): value is AppId {
  return value === "claude" || value === "codex" || value === "gemini";
}

function getInitialApp(): AppId {
  const stored = localStorage.getItem(APP_STORAGE_KEY);
  if (isServerApp(stored)) {
    return stored;
  }
  return "claude";
}

function getInitialView(): View {
  const stored = localStorage.getItem(VIEW_STORAGE_KEY);
  if (stored === "providers" || stored === "shares" || stored === "settings") {
    return stored;
  }
  return "providers";
}

interface ServerDesktopAppProps {
  onSignOut?: (options?: { clearPasswordCache?: boolean }) => void;
}

export default function ServerDesktopApp({ onSignOut }: ServerDesktopAppProps = {}) {
  const { t } = useTranslation();
  useOauthQuotaRefreshBridge();
  const [activeApp, setActiveApp] = useState<AppId>(getInitialApp);
  const [currentView, setCurrentView] = useState<View>(getInitialView);
  const [settingsDefaultTab, setSettingsDefaultTab] =
    useState<SettingsTab>("general");
  const [isAddOpen, setIsAddOpen] = useState(false);
  const [editingProvider, setEditingProvider] = useState<Provider | null>(null);
  const [confirmAction, setConfirmAction] = useState<{
    provider: Provider;
    action: "delete";
  } | null>(null);

  const effectiveEditingProvider = useLastValidValue(editingProvider);

  useEffect(() => {
    localStorage.setItem(VIEW_STORAGE_KEY, currentView);
  }, [currentView]);

  useEffect(() => {
    localStorage.setItem(APP_STORAGE_KEY, activeApp);
  }, [activeApp]);

  const visibleApps: VisibleApps = {
    claude: true,
    "claude-desktop": false,
    codex: true,
    gemini: true,
    opencode: false,
    openclaw: false,
    hermes: false,
  };

  const needsProviderData =
    currentView === "providers" || currentView === "shares";
  const needsProxyPolling = currentView === "providers";

  const { data: proxyStatus } = useProxyStatus({ enabled: needsProxyPolling });
  const { data: takeoverStatus } = useProxyTakeoverStatus({
    enabled: needsProxyPolling,
  });
  const isProxyRunning = proxyStatus?.running ?? false;

  const isCurrentAppTakeoverActive = takeoverStatus?.[activeApp] ?? false;
  const activeProviderId = useMemo(() => {
    const target = proxyStatus?.active_targets?.find(
      (item) => item.app_type === activeApp,
    );
    return target?.provider_id;
  }, [proxyStatus?.active_targets, activeApp]);

  const { data, isLoading } = useProvidersQuery(activeApp, {
    isProxyRunning,
    enabled: needsProviderData,
  });
  const providers = useMemo(() => data?.providers ?? {}, [data]);
  const currentProviderId = data?.currentProviderId ?? "";

  const { data: settingsData } = useSettingsQuery();

  const {
    addProvider,
    updateProvider,
    switchProvider,
    clearCurrentProvider,
    deleteProvider,
  } = useProviderActions(
    activeApp,
    isProxyRunning,
    isProxyRunning && isCurrentAppTakeoverActive,
  );

  const handleOpenWebsite = useCallback(
    async (url: string) => {
      try {
        window.open(url, "_blank", "noopener,noreferrer");
      } catch (error) {
        toast.error(
          extractErrorMessage(error) ||
            t("notifications.openLinkFailed", { defaultValue: "链接打开失败" }),
        );
      }
    },
    [t],
  );

  const handleDuplicateProvider = useCallback(
    async (provider: Provider) => {
      const newSortIndex =
        provider.sortIndex !== undefined ? provider.sortIndex + 1 : undefined;
      const duplicatedProvider: Omit<Provider, "id" | "createdAt"> & {
        providerKey?: string;
      } = {
        name: `${provider.name} copy`,
        settingsConfig: deepClone(provider.settingsConfig),
        websiteUrl: provider.websiteUrl,
        category: provider.category,
        sortIndex: newSortIndex,
        meta: provider.meta ? deepClone(provider.meta) : undefined,
        icon: provider.icon,
        iconColor: provider.iconColor,
      };
      await addProvider(duplicatedProvider);
    },
    [addProvider],
  );

  const handleEditProvider = useCallback(
    async ({
      provider,
      originalId,
    }: {
      provider: Provider;
      originalId?: string;
    }) => {
      await updateProvider(provider, originalId);
      setEditingProvider(null);
    },
    [updateProvider],
  );

  const handleSignOut = useCallback(
    (options?: { clearPasswordCache?: boolean }) => {
      writeToken(null);
      if (isRemoteWebMode()) {
        clearRouterSessionTokens();
      }
      if (options?.clearPasswordCache !== false) {
        writeCachedPassword(null);
      }
      if (onSignOut) {
        onSignOut();
      } else {
        window.location.reload();
      }
    },
    [onSignOut],
  );

  const handleConfirmAction = useCallback(async () => {
    if (!confirmAction) return;
    await deleteProvider(confirmAction.provider.id);
    setConfirmAction(null);
  }, [confirmAction, deleteProvider]);

  const openSettings = useCallback(
    (tab: SettingsTab) => {
      setSettingsDefaultTab(tab);
      setCurrentView("settings");
    },
    [],
  );

  const addActionButtonClass =
    "bg-orange-500 hover:bg-orange-600 dark:bg-orange-500 dark:hover:bg-orange-600 text-white shadow-lg shadow-orange-500/30 dark:shadow-orange-500/40 rounded-full w-8 h-8";

  const isProviderHome =
    currentView === "providers" &&
    editingProvider === null &&
    !isAddOpen;

  const content = (() => {
    switch (currentView) {
      case "settings":
        return (
          <SettingsPage
            open
            onOpenChange={() => setCurrentView("providers")}
            defaultTab={settingsDefaultTab}
            onSignOut={handleSignOut}
          />
        );
      case "shares":
        if (
          activeApp !== "claude" &&
          activeApp !== "codex" &&
          activeApp !== "gemini"
        ) {
          return (
            <div className="px-6 pt-6 text-sm text-muted-foreground">
              {t("share.unsupportedApp", {
                defaultValue:
                  "{{app}} 暂不支持 share；请切换到 Claude / Codex / Gemini tab 后再创建 share。",
                app: activeApp,
              })}
            </div>
          );
        }
        return (
          <SharePage
            defaultApp={activeApp}
            onOpenShareSettings={() => {
              setSettingsDefaultTab("share");
              setCurrentView("settings");
            }}
          />
        );
      default:
        return (
          <div className="px-6 flex flex-col flex-1 min-h-0 overflow-hidden">
            <div className="flex-1 overflow-y-auto overflow-x-hidden pb-12 px-1">
              <AnimatePresence mode="wait">
                <motion.div
                  key={activeApp}
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1 }}
                  exit={{ opacity: 0 }}
                  transition={{ duration: 0.15 }}
                  className="space-y-4"
                >
                  <ProviderList
                    providers={providers}
                    currentProviderId={currentProviderId}
                    appId={activeApp}
                    isLoading={isLoading}
                    isProxyRunning={isProxyRunning}
                    isProxyTakeover={
                      isProxyRunning && isCurrentAppTakeoverActive
                    }
                    activeProviderId={activeProviderId}
                    onSwitch={switchProvider}
                    onClearCurrent={clearCurrentProvider}
                    onEdit={(provider) => setEditingProvider(provider)}
                    onDelete={(provider) =>
                      setConfirmAction({ provider, action: "delete" })
                    }
                    onDuplicate={handleDuplicateProvider}
                    onOpenWebsite={handleOpenWebsite}
                    onCreate={() => setIsAddOpen(true)}
                  />
                </motion.div>
              </AnimatePresence>
            </div>
          </div>
        );
    }
  })();

  return (
    <div className="flex flex-col h-screen overflow-hidden bg-background text-foreground selection:bg-primary/30 pb-4">
      <header className="sticky top-0 z-50 w-full bg-background/80 backdrop-blur-md">
        <div
          className={cn(
            PAGE_SHELL_CLASS,
            "flex h-14 items-center justify-between gap-2 px-6",
          )}
        >
          <div className="flex min-w-0 items-center gap-2">
            {currentView !== "providers" ? (
              <div className="flex items-center gap-2">
                <Button
                  variant="outline"
                  size="icon"
                  onClick={() => setCurrentView("providers")}
                  className="rounded-lg"
                >
                  <ArrowLeft className="w-4 h-4" />
                </Button>
                <h1 className="text-lg font-semibold truncate">
                  {currentView === "settings" && t("settings.title")}
                  {currentView === "shares" && t("share.title")}
                </h1>
              </div>
            ) : (
              <div className="flex items-center gap-2">
                <a
                  href="https://tokenswitch.org"
                  target="_blank"
                  rel="noreferrer"
                  className={cn(
                    "text-xl font-semibold transition-colors",
                    isProxyRunning && isCurrentAppTakeoverActive
                      ? "text-emerald-500 hover:text-emerald-600"
                      : "text-blue-500 hover:text-blue-600",
                  )}
                >
                  CC Switch Server
                </a>
                <Button
                  variant="ghost"
                  size="icon"
                  onClick={() => openSettings("general")}
                  title={t("common.settings")}
                  className="hover:bg-black/5 dark:hover:bg-white/5"
                >
                  <Settings className="w-4 h-4" />
                </Button>
                {isProviderHome && (
                  <>
                    <Button
                      variant="ghost"
                      size="icon"
                      onClick={() => openSettings("usage")}
                      title={t("usage.title", { defaultValue: "使用统计" })}
                      className="hover:bg-black/5 dark:hover:bg-white/5"
                    >
                      <BarChart2 className="w-4 h-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      onClick={() => setCurrentView("shares")}
                      title={t("share.title")}
                      className="hover:bg-black/5 dark:hover:bg-white/5"
                    >
                      <Share2 className="w-4 h-4" />
                    </Button>
                  </>
                )}
              </div>
            )}
          </div>

          {isProviderHome && (
            <div className="flex min-w-0 flex-1 items-center justify-end gap-1.5">
              {settingsData?.enableFailoverToggle !== false && (
                <div className="flex shrink-0 items-center gap-1.5">
                  <FailoverToggle activeApp={activeApp} />
                </div>
              )}
              <div className="flex min-w-0 flex-1 items-center">
                <div className="ml-auto flex shrink-0 items-center gap-1.5">
                  <AppSwitcher
                    activeApp={activeApp}
                    onSwitch={setActiveApp}
                    visibleApps={visibleApps}
                  />
                  <Button
                    onClick={() => setIsAddOpen(true)}
                    size="icon"
                    className={cn("ml-1", addActionButtonClass)}
                    title={t("providers.addProvider", {
                      defaultValue: "添加供应商",
                    })}
                  >
                    <Plus className="h-5 w-5" />
                  </Button>
                </div>
              </div>
            </div>
          )}
        </div>
      </header>

      <main className="flex-1 min-h-0 overflow-hidden">
        <AnimatePresence mode="wait">
          <motion.div
            key={currentView}
            className={cn(PAGE_SHELL_CLASS, "h-full")}
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.2 }}
          >
            {content}
          </motion.div>
        </AnimatePresence>
      </main>

      <AddProviderDialog
        open={isAddOpen}
        onOpenChange={setIsAddOpen}
        appId={activeApp}
        onSubmit={addProvider}
      />

      <EditProviderDialog
        open={Boolean(editingProvider)}
        provider={effectiveEditingProvider}
        appId={activeApp}
        isProxyTakeover={isProxyRunning && isCurrentAppTakeoverActive}
        onOpenChange={(open) => {
          if (!open) setEditingProvider(null);
        }}
        onSubmit={handleEditProvider}
        onOpenShareSettings={() => {
          setEditingProvider(null);
          setSettingsDefaultTab("share");
          setCurrentView("settings");
        }}
      />

      <ConfirmDialog
        isOpen={confirmAction !== null}
        title={t("providers.deleteConfirmTitle", {
          defaultValue: "Delete provider?",
        })}
        message={t("providers.deleteConfirmMessage", {
          defaultValue: "This action cannot be undone.",
          name: confirmAction?.provider.name ?? "",
        })}
        confirmText={t("common.delete", { defaultValue: "Delete" })}
        variant="destructive"
        onConfirm={() => void handleConfirmAction()}
        onCancel={() => setConfirmAction(null)}
      />
    </div>
  );
}
