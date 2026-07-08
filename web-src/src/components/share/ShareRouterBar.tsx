import { useTranslation } from "react-i18next";

interface ShareRouterBarProps {
  proxyRunning: boolean;
  proxyAddress?: string | null;
  proxyPort?: number | null;
  hasShare: boolean;
  readOnly?: boolean;
  mode?: "share-page" | "settings";
  onCreate?: () => void;
}

export function ShareRouterBar({
  proxyRunning,
  proxyAddress,
  proxyPort,
  hasShare,
  readOnly = false,
  mode = "share-page",
}: ShareRouterBarProps) {
  const { t } = useTranslation();

  if (mode === "share-page" && (readOnly || proxyRunning)) {
    return null;
  }

  return (
    <div className="rounded-xl border border-border-default/70 bg-card/80 px-4 py-3">
      {mode === "share-page" ? (
        <div className="text-sm text-muted-foreground">
          {hasShare
            ? t("share.routerLockedAfterCreate", {
                defaultValue: "路由节点已绑定。",
              })
            : t("share.createDescription")}
        </div>
      ) : (
        <div className="text-sm font-medium">
          {t("settings.share.proxyStatus.title", {
            defaultValue: "本地路由状态",
          })}
        </div>
      )}

      {proxyRunning ? (
        <div className="mt-3 text-xs text-emerald-600 dark:text-emerald-400">
          {t("settings.share.proxyStatus.running", {
            defaultValue: "本地路由运行中：{{address}}:{{port}}",
            address: proxyAddress || "127.0.0.1",
            port: proxyPort || 53000,
          })}
        </div>
      ) : (
        <div className="mt-3 text-xs text-amber-600 dark:text-amber-400">
          {t("share.proxyCompactWarning", {
            address: proxyAddress || "127.0.0.1",
            port: proxyPort || 53000,
          })}
        </div>
      )}
    </div>
  );
}
