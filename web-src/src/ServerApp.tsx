import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion } from "framer-motion";
import {
  ArrowLeft,
  Settings,
  Share2,
  ShieldCheck,
  Terminal,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  SettingsPage,
  type SettingsPageHandle,
  type SettingsTab,
} from "@/components/settings/SettingsPage";
import { SharePage } from "@/components/share/SharePage";
import { useOauthQuotaRefreshBridge } from "@/hooks/useOauthQuotaRefreshBridge";
import { useProviderHealthRefreshBridge } from "@/hooks/useProviderHealthRefreshBridge";
import { isRemoteWebMode } from "@/lib/api/auth";
import {
  APP_VIEWPORT_PADDING_Y,
  PAGE_HEADER_CONTENT_GAP,
  PAGE_SHELL_CLASS,
  PAGE_SHELL_PADDING_X,
} from "@/lib/layout";
import { clearRouterSessionTokens } from "@/lib/routerAuth";
import { writeToken } from "@/lib/runtime";
import {
  resolveInitialServerView,
  SERVER_VIEW_STORAGE_KEY,
  type ServerView,
} from "@/lib/serverView";
import { cn } from "@/lib/utils";
import { ProviderBundlesPage } from "@/server/providers/bundles/ProviderBundlesPage";

const TerminalPage = lazy(() => import("@/components/terminal/TerminalPage"));

type View = ServerView;

interface ServerAppProps {
  onSignOut?: (options?: { clearPasswordCache?: boolean }) => void;
  enableWebTerminal?: boolean;
}

export default function ServerApp({
  onSignOut,
  enableWebTerminal = true,
}: ServerAppProps = {}) {
  const { t } = useTranslation();
  useOauthQuotaRefreshBridge();
  useProviderHealthRefreshBridge();
  const [currentView, setCurrentView] = useState<View>(() =>
    resolveInitialServerView(
      enableWebTerminal,
      typeof window === "undefined" ? "" : window.location.search,
      typeof window === "undefined"
        ? null
        : localStorage.getItem(SERVER_VIEW_STORAGE_KEY),
    ),
  );
  const [settingsDefaultTab, setSettingsDefaultTab] =
    useState<SettingsTab>("general");
  const settingsPageRef = useRef<SettingsPageHandle>(null);

  useEffect(() => {
    localStorage.setItem(SERVER_VIEW_STORAGE_KEY, currentView);
  }, [currentView]);

  useEffect(() => {
    if (!enableWebTerminal && currentView === "terminal") {
      setCurrentView("providers");
    }
  }, [enableWebTerminal, currentView]);

  const handleSignOut = useCallback(
    (_options?: { clearPasswordCache?: boolean }) => {
      writeToken(null);
      if (isRemoteWebMode()) clearRouterSessionTokens();
      if (onSignOut) onSignOut();
      else window.location.reload();
    },
    [onSignOut],
  );

  const openSettings = useCallback((tab: SettingsTab) => {
    setSettingsDefaultTab(tab);
    setCurrentView("settings");
  }, []);

  const handleBack = useCallback(() => {
    if (currentView === "settings") {
      settingsPageRef.current?.requestClose();
      return;
    }
    setCurrentView("providers");
  }, [currentView]);

  const content = (() => {
    if (currentView === "settings") {
      return (
        <SettingsPage
          ref={settingsPageRef}
          open
          onOpenChange={() => setCurrentView("providers")}
          defaultTab={settingsDefaultTab}
          onSignOut={handleSignOut}
        />
      );
    }
    if (currentView === "shares") {
      return (
        <SharePage
          defaultApp="claude"
          onOpenShareSettings={() => openSettings("share")}
        />
      );
    }
    if (currentView === "terminal") {
      return (
        <Suspense
          fallback={
            <div
              className={cn(
                PAGE_SHELL_PADDING_X,
                "pt-4 text-sm text-muted-foreground",
              )}
            >
              {t("common.loading")}
            </div>
          }
        >
          <TerminalPage />
        </Suspense>
      );
    }
    return (
      <div
        className={cn(
          PAGE_SHELL_PADDING_X,
          "flex h-full min-h-0 flex-col overflow-y-auto",
        )}
      >
        <ProviderBundlesPage
          onOpenShareSettings={() => openSettings("share")}
          toolbarActions={
            <div className="flex shrink-0 items-center gap-1">
              <Button
                variant="ghost"
                size="icon"
                onClick={() => openSettings("auth")}
                title={t("settings.tabAuth", { defaultValue: "认证" })}
              >
                <ShieldCheck className="h-4 w-4" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                onClick={() => setCurrentView("shares")}
                title={t("share.title")}
              >
                <Share2 className="h-4 w-4" />
              </Button>
              {enableWebTerminal ? (
                <Button
                  variant="ghost"
                  size="icon"
                  onClick={() => setCurrentView("terminal")}
                  title={t("terminal.nav")}
                >
                  <Terminal className="h-4 w-4" />
                </Button>
              ) : null}
              <Button
                variant="ghost"
                size="icon"
                onClick={() => openSettings("general")}
                title={t("common.settings")}
              >
                <Settings className="h-4 w-4" />
              </Button>
            </div>
          }
        />
      </div>
    );
  })();

  return (
    <div
      className={cn(
        "flex h-screen flex-col overflow-hidden bg-background text-foreground selection:bg-primary/30",
        APP_VIEWPORT_PADDING_Y,
        PAGE_HEADER_CONTENT_GAP,
      )}
    >
      {currentView !== "providers" ? (
        <header className="sticky top-0 z-50 w-full shrink-0 bg-background/90 backdrop-blur-md">
          <div
            className={cn(
              PAGE_SHELL_CLASS,
              PAGE_SHELL_PADDING_X,
              "flex min-h-14 items-center gap-2",
            )}
          >
            <Button
              variant="outline"
              size="icon"
              onClick={handleBack}
              title={t("common.back")}
            >
              <ArrowLeft className="h-4 w-4" />
            </Button>
            <h1 className="truncate text-lg font-semibold">
              {currentView === "settings" && t("settings.title")}
              {currentView === "shares" && t("share.title")}
              {currentView === "terminal" && t("terminal.title")}
            </h1>
          </div>
        </header>
      ) : null}

      <main className="min-h-0 flex-1 overflow-hidden">
        <AnimatePresence mode="wait">
          <motion.div
            key={currentView}
            className={cn(PAGE_SHELL_CLASS, "h-full")}
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.16 }}
          >
            {content}
          </motion.div>
        </AnimatePresence>
      </main>
    </div>
  );
}
