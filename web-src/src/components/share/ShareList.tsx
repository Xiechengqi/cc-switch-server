import { useTranslation } from "react-i18next";
import type { ShareRecord, TunnelConfig, TunnelInfo } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { ShareCard, type ShareProviderSalePricing } from "./ShareCard";

interface ShareListProps {
  shares: ShareRecord[];
  tunnelStatusMap: Record<string, TunnelInfo | null | undefined>;
  tunnelConfig: TunnelConfig;
  tunnelConfigured: boolean;
  isLoading: boolean;
  error: string | null;
  pendingAction?: string | null;
  providerSalePricing?: ShareProviderSalePricing[];
  providerNameByKey?: Record<string, string>;
  providerAccountByKey?: Record<string, string>;
  readOnly?: boolean;
  hideRuntimeActions?: boolean;
  onRetry: () => void;
  onDelete?: (share: ShareRecord) => void;
  onEnable?: (share: ShareRecord) => void;
  onDisable?: (share: ShareRecord) => void;
  onResetUsage?: (share: ShareRecord) => Promise<void> | void;
}

export function ShareList({
  shares,
  tunnelStatusMap,
  tunnelConfig,
  tunnelConfigured,
  isLoading,
  error,
  pendingAction,
  providerSalePricing,
  providerNameByKey,
  providerAccountByKey,
  readOnly = false,
  hideRuntimeActions = false,
  onRetry,
  onDelete,
  onEnable,
  onDisable,
  onResetUsage,
}: ShareListProps) {
  const { t } = useTranslation();

  if (error) {
    return (
      <Card className="density-empty-state density-surface-card border-red-500/30 bg-red-500/5">
        <CardContent className="flex flex-col items-start gap-4 px-6 py-6">
          <div>
            <div className="text-base font-medium">
              {t("share.error.title")}
            </div>
            <div className="mt-1 text-sm text-muted-foreground">{error}</div>
          </div>
          <Button variant="outline" onClick={onRetry}>
            {t("common.retry")}
          </Button>
        </CardContent>
      </Card>
    );
  }

  if (isLoading) {
    return (
      <div className="share-list-skeleton grid gap-4">
        {Array.from({ length: 3 }).map((_, index) => (
          <div
            key={index}
            className="h-52 animate-pulse rounded-2xl border border-border-default bg-muted/30"
          />
        ))}
      </div>
    );
  }

  if (shares.length === 0) {
    return (
      <Card className="density-empty-state density-surface-card border-dashed border-border-default/80 bg-muted/15">
        <CardContent className="flex flex-col items-center gap-4 px-6 py-12 text-center">
          <div className="space-y-2">
            <h3 className="text-xl font-semibold">{t("share.empty")}</h3>
            <p className="max-w-xl text-sm text-muted-foreground">
              {readOnly
                ? t("share.emptyReadOnlyHint", {
                    defaultValue:
                      "分享在 Provider 编辑页的「远程分享」区域创建与管理。请打开对应 Provider 并启用分享。",
                  })
                : t("share.emptyDescription")}
            </p>
          </div>
        </CardContent>
      </Card>
    );
  }

  return (
    <div className="share-list-root grid gap-4">
      {readOnly ? (
        <p className="rounded-xl border border-border-default/70 bg-card/40 px-4 py-2.5 text-sm text-muted-foreground">
          {t("share.readOnlyOverviewHint", {
            defaultValue:
              "此为只读总览。修改分享设置请前往对应 Provider 的编辑页。",
          })}
        </p>
      ) : null}
      <div className="text-sm text-muted-foreground">
        {t("share.listCount", {
          defaultValue: "{{count}} 个 share",
          count: shares.length,
        })}
      </div>
      {shares.map((share) => (
        <ShareCard
          key={share.id}
          share={share}
          providerNameByKey={providerNameByKey}
          providerAccountByKey={providerAccountByKey}
          tunnelStatus={tunnelStatusMap[share.id]}
          tunnelConfig={tunnelConfig}
          tunnelConfigured={tunnelConfigured}
          pendingAction={pendingAction}
          providerSalePricing={providerSalePricing}
          readOnly={readOnly}
          hideRuntimeActions={hideRuntimeActions}
          onDelete={onDelete ?? (() => undefined)}
          onEnable={onEnable ?? (() => undefined)}
          onDisable={onDisable ?? (() => undefined)}
          onResetUsage={onResetUsage ?? (() => undefined)}
        />
      ))}
    </div>
  );
}
