import { useTranslation } from "react-i18next";

interface ShareRouterBarProps {
  proxyRunning: boolean;
  proxyAddress?: string | null;
  proxyPort?: number | null;
  hasShare: boolean;
  readOnly?: boolean;
  /**
   * 已不再承载创建职责（创建入口在 ShareList toolbar）。
   * 保留 prop 是为 SharePage 已有调用方零改动；将来可清理。
   */
  onCreate?: () => void;
}

export function ShareRouterBar({
  proxyRunning,
  proxyAddress,
  proxyPort,
  hasShare,
  readOnly = false,
}: ShareRouterBarProps) {
  const { t } = useTranslation();

  // 多 share 模式下，ShareRouterBar 只是路由节点/代理状态的状态条，
  // 创建入口由 ShareList toolbar 承担。代理在跑就没什么可警告，整条隐藏。
  if (readOnly || proxyRunning) {
    return null;
  }

  return (
    <div className="rounded-xl border border-border-default/70 bg-card/80 px-4 py-3">
      <div className="text-sm text-muted-foreground">
        {hasShare
          ? t("share.routerLockedAfterCreate", {
              defaultValue: "路由节点已绑定。",
            })
          : t("share.createDescription")}
      </div>

      <div className="mt-3 text-xs text-amber-600 dark:text-amber-400">
        {t("share.proxyCompactWarning", {
          address: proxyAddress || "127.0.0.1",
          port: proxyPort || 53000,
        })}
      </div>
    </div>
  );
}
