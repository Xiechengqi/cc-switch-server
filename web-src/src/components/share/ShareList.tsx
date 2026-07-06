import { useTranslation } from "react-i18next";
import { Plus } from "lucide-react";
import type {
  PublicMarket,
  ShareAccessByApp,
  ShareAppSettingsByApp,
  ShareRecord,
  TunnelConfig,
  TunnelInfo,
} from "@/lib/api";
import type { ProviderOption } from "./CreateShareDialog";
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
  markets?: PublicMarket[];
  providerSalePricing?: ShareProviderSalePricing[];
  /**
   * 用于在 ShareCard 上展示绑定 provider 的名称。
   * 形如 `claude:p1` → "Provider A"；找不到时 ShareCard 退化显示 provider id。
   */
  providerNameByKey?: Record<string, string>;
  providerAccountByKey?: Record<string, string>;
  marketsLoading?: boolean;
  marketsError?: string | null;
  readOnly?: boolean;
  hideRuntimeActions?: boolean;
  subdomainReadOnly?: boolean;
  onRetryMarkets?: () => void;
  onRetry: () => void;
  onCreate: () => void;
  onDelete: (share: ShareRecord) => void;
  onEnable: (share: ShareRecord) => void;
  onDisable: (share: ShareRecord) => void;
  onResetUsage: (share: ShareRecord) => Promise<void> | void;
  onUpdateTokenLimit: (
    share: ShareRecord,
    tokenLimit: number,
  ) => Promise<void> | void;
  onUpdateParallelLimit: (
    share: ShareRecord,
    parallelLimit: number,
  ) => Promise<void> | void;
  onUpdateSubdomain: (
    share: ShareRecord,
    subdomain: string,
  ) => Promise<void> | void;
  onUpdateDescription: (
    share: ShareRecord,
    description: string,
  ) => Promise<void> | void;
  onUpdateForSale: (
    share: ShareRecord,
    forSale: "Yes" | "No" | "Free",
  ) => Promise<void> | void;
  onUpdateShareSalePricing: (
    share: ShareRecord,
    pricing: Record<string, number>,
  ) => Promise<void> | void;
  onUpdateExpiration: (
    share: ShareRecord,
    expiresAt: string,
  ) => Promise<void> | void;
  onUpdateOwnerEmail: (
    share: ShareRecord,
    ownerEmail: string,
  ) => Promise<void> | void;
  onTransferOwner: (
    share: ShareRecord,
    targetEmail: string,
  ) => Promise<void> | void;
  onUpdateAcl: (
    share: ShareRecord,
    sharedWithEmails: string[],
    marketAccessMode: "selected" | "all",
    accessByApp?: ShareAccessByApp,
    saleMarketKind?: "token" | "share",
    appSettings?: ShareAppSettingsByApp,
  ) => Promise<void> | void;
  onUpdateProviderBinding: (
    share: ShareRecord,
    appType: "claude" | "codex" | "gemini",
    providerId: string | null,
    options?: { dynamic?: boolean },
  ) => Promise<void> | void;
  onRebindAtomic?: (
    share: ShareRecord,
    appType: "claude" | "codex" | "gemini",
    newProviderId: string | null,
    options?: { dynamic?: boolean },
  ) => Promise<void> | void;
  /**
   * 全 app 维度的"可绑定 provider 列表"映射，key = appType。
   * EditShareDialog 取本 share 的 appType 那一份，并把"share 自己当前绑定的
   * provider"从 disabled 集合里挪出来——否则用户要换回原绑定时会被禁选。
   */
  providersByApp?: Partial<
    Record<
      "claude" | "codex" | "gemini",
      import("./CreateShareDialog").ProviderOption[]
    >
  >;
}

export function ShareList({
  shares,
  tunnelStatusMap,
  tunnelConfig,
  tunnelConfigured,
  isLoading,
  error,
  pendingAction,
  markets,
  providerSalePricing,
  providerNameByKey,
  providerAccountByKey,
  marketsLoading,
  marketsError,
  readOnly = false,
  hideRuntimeActions = false,
  subdomainReadOnly = false,
  onRetryMarkets,
  onRetry,
  onCreate,
  onDelete,
  onEnable,
  onDisable,
  onResetUsage,
  onUpdateTokenLimit,
  onUpdateParallelLimit,
  onUpdateSubdomain,
  onUpdateDescription,
  onUpdateForSale,
  onUpdateShareSalePricing,
  onUpdateExpiration,
  onUpdateOwnerEmail,
  onTransferOwner,
  onUpdateAcl,
  onUpdateProviderBinding,
  onRebindAtomic,
  providersByApp,
}: ShareListProps) {
  const { t } = useTranslation();

  if (error) {
    return (
      <Card className="border-red-500/30 bg-red-500/5">
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
      <div className="grid gap-4">
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
      <Card className="border-dashed border-border-default/80 bg-muted/15">
        <CardContent className="flex flex-col items-center gap-4 px-6 py-12 text-center">
          <div className="space-y-2">
            <h3 className="text-xl font-semibold">{t("share.empty")}</h3>
            <p className="max-w-xl text-sm text-muted-foreground">
              {t("share.emptyDescription")}
            </p>
          </div>
          {!readOnly ? (
            <Button onClick={onCreate}>{t("share.emptyCta")}</Button>
          ) : null}
        </CardContent>
      </Card>
    );
  }

  return (
    <div className="grid gap-4">
      {!readOnly ? (
        // 多 share 模式：列表头部常驻"新建"入口。原 ShareRouterBar 上的 Create
        // 按钮在 hasShare && proxyRunning 时会整体隐藏，导致用户创建第一个
        // share 后没法再加；toolbar 把入口提上来保证一直可见。
        <div className="flex items-center justify-between rounded-xl border border-border-default/70 bg-card/40 px-4 py-2.5">
          <div className="text-sm text-muted-foreground">
            {t("share.listCount", {
              defaultValue: "{{count}} 个 share",
              count: shares.length,
            })}
          </div>
          <Button onClick={onCreate} size="sm">
            <Plus className="mr-1 h-4 w-4" />
            {t("share.create")}
          </Button>
        </div>
      ) : null}
      {shares.map((share) => (
        <ShareCard
          key={share.id}
          share={share}
          // P8：多 app share，传 bindings 全集；ShareCard 自己渲染 0..3 个 provider chip。
          providerNameByKey={providerNameByKey}
          providerAccountByKey={providerAccountByKey}
          tunnelStatus={tunnelStatusMap[share.id]}
          tunnelConfig={tunnelConfig}
          tunnelConfigured={tunnelConfigured}
          pendingAction={pendingAction}
          markets={markets}
          providerSalePricing={providerSalePricing}
          marketsLoading={marketsLoading}
          marketsError={marketsError}
          readOnly={readOnly}
          hideRuntimeActions={hideRuntimeActions}
          subdomainReadOnly={subdomainReadOnly}
          onRetryMarkets={onRetryMarkets}
          onDelete={onDelete}
          onEnable={onEnable}
          onDisable={onDisable}
          onResetUsage={onResetUsage}
          onUpdateTokenLimit={onUpdateTokenLimit}
          onUpdateParallelLimit={onUpdateParallelLimit}
          onUpdateSubdomain={onUpdateSubdomain}
          onUpdateDescription={onUpdateDescription}
          onUpdateForSale={onUpdateForSale}
          onUpdateShareSalePricing={onUpdateShareSalePricing}
          onUpdateExpiration={onUpdateExpiration}
          onUpdateOwnerEmail={onUpdateOwnerEmail}
          onTransferOwner={onTransferOwner}
          onUpdateAcl={onUpdateAcl}
          onUpdateProviderBinding={onUpdateProviderBinding}
          onRebindAtomic={onRebindAtomic}
          providersByAppForEdit={(() => {
            // share 自己已绑定的 provider 在对应 slot 内不算 taken（让"换回原 provider"
            // 始终可选）；其他 share 占着的仍标灰。
            const result: Record<
              "claude" | "codex" | "gemini",
              ProviderOption[]
            > = { claude: [], codex: [], gemini: [] };
            (["claude", "codex", "gemini"] as const).forEach((app) => {
              const pool = providersByApp?.[app] ?? [];
              const myPid = share.bindings?.[app];
              result[app] = pool.map((p) =>
                myPid && p.id === myPid ? { ...p, disabled: false } : p,
              );
            });
            return result;
          })()}
        />
      ))}
    </div>
  );
}
