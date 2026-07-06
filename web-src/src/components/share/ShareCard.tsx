import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ChevronDown,
  ChevronRight,
  Copy,
  Edit3,
  Play,
  Power,
  RotateCcw,
  Trash2,
} from "lucide-react";
import type {
  PublicMarket,
  ShareAccessByApp,
  ShareAppSettingsByApp,
  ShareRecord,
  TunnelConfig,
  TunnelInfo,
} from "@/lib/api";
import {
  sharePrimaryApp,
  sharePrimaryProviderId,
  shareSupportedApps,
} from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
import { cn } from "@/lib/utils";
import { useProviderHealth } from "@/lib/query/failover";
import { copyText } from "@/lib/clipboard";
import { toast } from "sonner";
import { formatShareRouterDisplay } from "@/utils/shareRouter";
import { EditShareDialog } from "./EditShareDialog";
import type { ProviderOption } from "./CreateShareDialog";
import { ShareDisplayStatusBadge } from "./ShareDisplayStatusBadge";
import { ShareRequestLogTable } from "./ShareRequestLogTable";
import {
  formatShareTokenUsage,
  formatUtcDateTime,
  getShareDisplayStatus,
  getShareTunnelRuntimeStatus,
  getShareUsageRatio,
  isPermanentExpiry,
  isShareActionAllowed,
  isUnlimitedParallelLimit,
  isUnlimitedTokenLimit,
  resolveShareTunnelInfo,
} from "@/utils/shareUtils";

export interface ShareProviderSalePricing {
  app: "claude" | "codex" | "gemini";
  label: string;
  providerName?: string;
  percent?: number;
}

interface ShareCardProps {
  share: ShareRecord;
  /**
   * P8：`${appType}:${providerId}` → provider 显示名，跨所有 app 维度。ShareCard
   * 在卡片摘要里渲染每个已绑 slot 的 provider 名；找不到时回退显示 provider id。
   */
  providerNameByKey?: Record<string, string>;
  providerAccountByKey?: Record<string, string>;
  tunnelStatus?: TunnelInfo | null;
  tunnelConfig: TunnelConfig;
  tunnelConfigured: boolean;
  pendingAction?: string | null;
  markets?: PublicMarket[];
  providerSalePricing?: ShareProviderSalePricing[];
  marketsLoading?: boolean;
  marketsError?: string | null;
  readOnly?: boolean;
  hideRuntimeActions?: boolean;
  subdomainReadOnly?: boolean;
  onRetryMarkets?: () => void;
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
  /**
   * 当前 app 下可绑定的 provider 列表（同 CreateShareDialog 的形态）。
   * 由 ShareList 透传，传给 EditShareDialog 的 Provider Select。
   */
  providersByAppForEdit?: Record<
    "claude" | "codex" | "gemini",
    ProviderOption[]
  >;
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
}

const EMPTY_MARKETS: PublicMarket[] = [];
const EMPTY_PROVIDER_SALE_PRICING: ShareProviderSalePricing[] = [];

export function ShareCard({
  share,
  providerNameByKey,
  providerAccountByKey,
  tunnelStatus,
  tunnelConfig,
  tunnelConfigured,
  pendingAction,
  markets = EMPTY_MARKETS,
  providerSalePricing = EMPTY_PROVIDER_SALE_PRICING,
  marketsLoading = false,
  marketsError = null,
  readOnly = false,
  hideRuntimeActions = false,
  subdomainReadOnly = false,
  onRetryMarkets,
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
  providersByAppForEdit,
  onUpdateProviderBinding,
  onRebindAtomic,
}: ShareCardProps) {
  const { t } = useTranslation();
  const [editDialogOpen, setEditDialogOpen] = useState(false);
  const [connectionExpanded, setConnectionExpanded] = useState(false);
  const [settingsExpanded, setSettingsExpanded] = useState(false);
  const ratio = getShareUsageRatio(share);
  // P8：多 app share。胸标里渲染每个已绑定 slot 的 chip + 健康色点。
  // primaryApp/primaryProvider 用于摘要标题、健康轮询等仍按"单值"逻辑的入口。
  const primaryAppType = sharePrimaryApp(share);
  const primaryProviderIdValue = sharePrimaryProviderId(share);
  // C-2：拉主 binding 的健康状态作为卡片首要状态指示。其它 slot 的健康由
  // EditDialog 内展开时再单独拉。
  const { data: boundProviderHealth } = useProviderHealth(
    primaryProviderIdValue ?? "",
    primaryAppType ?? "claude",
  );
  const isBusy = pendingAction === share.id;
  const tunnelDisplay = resolveShareTunnelInfo(share, tunnelConfig);
  const tunnelRuntimeStatus = getShareTunnelRuntimeStatus(share, tunnelStatus);
  const routerDisplay = formatShareRouterDisplay(tunnelConfig.domain);
  const displayStatus = getShareDisplayStatus(
    share,
    tunnelConfigured,
    tunnelStatus,
  );
  const usageMarkets = markets.filter(
    (market) => (market.marketKind ?? "usage") !== "share",
  );
  const marketEmailSet = new Set(
    usageMarkets.map((market) => market.email.toLowerCase()),
  );
  const currentMarketEmails = Array.from(
    new Set(
      (share.sharedWithEmails ?? [])
        .map((email) => email.trim().toLowerCase())
        .filter((email) => marketEmailSet.has(email)),
    ),
  ).sort();
  const currentMarketAccessMode = share.marketAccessMode ?? "selected";
  const shareToSummary = shareAccessSummary(share, marketEmailSet);

  const canDisable = isShareActionAllowed(
    share,
    "disable",
    tunnelConfigured,
    tunnelStatus,
  );
  const canEnable = isShareActionAllowed(
    share,
    "enable",
    tunnelConfigured,
    tunnelStatus,
  );
  const canDelete = share.status === "paused";

  const handleCopy = async (value: string, key: string) => {
    await copyText(value);
    toast.success(t(key));
  };

  return (
    <Card className="border-border-default/70 bg-card/90">
      <CardContent className="space-y-5 px-5 py-5">
        <div className="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
          <div className="space-y-2">
            <div className="flex flex-wrap items-center gap-2">
              <h3 className="text-lg font-semibold">{share.name}</h3>
              <ShareDisplayStatusBadge status={displayStatus} />
              {/* P8：每个已绑定 slot 渲染一个 Provider chip。主 slot 显示健康色点（C-2）；
                  其它 slot 只显示绑定名（健康在 EditDialog 内查看）。 */}
              {shareSupportedApps(share).map((app) => {
                const pid = share.bindings[app];
                if (!pid) return null;
                const name = providerNameByKey?.[`${app}:${pid}`] ?? pid;
                const account = providerAccountByKey?.[`${app}:${pid}`];
                const isPrimary = app === primaryAppType;
                return (
                  <Badge
                    key={app}
                    variant="outline"
                    className="max-w-full rounded-full px-2.5 py-1 text-[11px] font-medium border-sky-500/30 bg-sky-500/10 text-sky-700 dark:text-sky-300"
                    title={[
                      t("share.boundProviderHint", {
                        defaultValue:
                          "本 share 在该 app 上的请求强制走此 provider，不参与故障转移",
                      }),
                      account ? `${name} · ${account}` : name,
                    ].join("\n")}
                  >
                    {isPrimary ? (
                      <span
                        className={cn(
                          "mr-1 inline-block h-2 w-2 rounded-full",
                          boundProviderHealth
                            ? boundProviderHealth.is_healthy
                              ? "bg-emerald-500"
                              : "bg-red-500"
                            : "bg-muted-foreground/40",
                        )}
                        aria-label={
                          boundProviderHealth?.is_healthy
                            ? "provider-healthy"
                            : "provider-unhealthy-or-unknown"
                        }
                      />
                    ) : null}
                    <span className="mr-1 uppercase">{app}</span>
                    <span className="min-w-0 max-w-40 truncate">{name}</span>
                    {account ? (
                      <span className="ml-1 min-w-0 max-w-52 truncate text-sky-700/70 dark:text-sky-200/75">
                        · {account}
                      </span>
                    ) : null}
                  </Badge>
                );
              })}
            </div>
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-sm text-muted-foreground">
              <span>
                {t("share.requestsCount")}: {share.requestsCount}
              </span>
              <span>
                {t("share.tokensUsed")}: {formatShareTokenUsage(share)}
              </span>
              <span>
                {t("share.expiresAt")}:{" "}
                {isPermanentExpiry(share.expiresAt)
                  ? t("share.expiry.permanentLabel")
                  : formatUtcDateTime(share.expiresAt)}
              </span>
              <span>
                {t("share.lastUsedAt")}:{" "}
                {share.lastUsedAt
                  ? formatUtcDateTime(share.lastUsedAt)
                  : t("share.never")}
              </span>
            </div>
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-sm text-muted-foreground">
              <span>
                {t("share.id")}: {share.id}
              </span>
              <span>
                {t("share.createdAt")}: {formatUtcDateTime(share.createdAt)}
              </span>
              <span>
                {t("share.remotePort")}:{" "}
                {tunnelStatus?.remotePort
                  ? String(tunnelStatus.remotePort)
                  : "-"}
              </span>
              <span>
                {t("share.tunnelHealth")}:{" "}
                {t(`share.statuses.${tunnelRuntimeStatus}`)}
              </span>
              <span>
                {t("share.tunnel.region")}: {routerDisplay}
              </span>
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            {!readOnly && !hideRuntimeActions && canDisable ? (
              <Button
                variant="outline"
                size="sm"
                disabled={isBusy}
                onClick={() => onDisable(share)}
              >
                <Power className="h-4 w-4" />
                {t("share.disable")}
              </Button>
            ) : !readOnly && !hideRuntimeActions ? (
              <Button
                variant="outline"
                size="sm"
                disabled={isBusy || !canEnable}
                onClick={() => onEnable(share)}
              >
                <Play className="h-4 w-4" />
                {t("share.enable")}
              </Button>
            ) : null}
            {!readOnly && !hideRuntimeActions ? (
              <Button
                variant="outline"
                size="sm"
                disabled={isBusy}
                onClick={() => {
                  if (!window.confirm(t("share.confirmResetUsageMessage"))) {
                    return;
                  }
                  void onResetUsage(share);
                }}
              >
                <RotateCcw className="h-4 w-4" />
                {t("share.resetUsage")}
              </Button>
            ) : null}
            <Button
              variant="outline"
              size="sm"
              disabled={isBusy}
              onClick={() => setEditDialogOpen(true)}
            >
              <Edit3 className="h-4 w-4" />
              {t("share.edit")}
            </Button>
            {!readOnly && !hideRuntimeActions ? (
              <Button
                variant="outline"
                size="sm"
                disabled={isBusy || !canDelete}
                className="text-destructive hover:text-destructive"
                onClick={() => onDelete(share)}
              >
                <Trash2 className="h-4 w-4" />
                {t("share.delete")}
              </Button>
            ) : null}
          </div>
        </div>

        <section className="space-y-3 border-t border-border-default/70 pt-4">
          <CollapsibleSectionHeader
            title={t("share.connectInfo")}
            expanded={connectionExpanded}
            onToggle={() => setConnectionExpanded((prev) => !prev)}
          />

          {connectionExpanded ? (
            <div className="grid gap-2 lg:grid-cols-3">
              <ConnectInlineValue
                label={t("share.tunnelUrl")}
                value={tunnelDisplay.tunnelUrl}
                onCopy={() =>
                  void handleCopy(
                    tunnelDisplay.tunnelUrl,
                    "share.toast.copyUrl",
                  )
                }
              />
              <ConnectInlineValue
                label={t("share.subdomain")}
                value={tunnelDisplay.subdomain}
                onCopy={() =>
                  void handleCopy(
                    tunnelDisplay.subdomain,
                    "share.toast.copySubdomain",
                  )
                }
              />
            </div>
          ) : null}
        </section>

        <section className="space-y-4 border-t border-border-default/70 pt-4">
          <CollapsibleSectionHeader
            title={t("share.settings", { defaultValue: "设置项" })}
            expanded={settingsExpanded}
            onToggle={() => setSettingsExpanded((prev) => !prev)}
          />

          {settingsExpanded ? (
            <>
              {!isUnlimitedTokenLimit(share.tokenLimit) ? (
                <div className="h-2 rounded-full bg-muted">
                  <div
                    className="h-2 rounded-full bg-blue-500"
                    style={{ width: `${Math.max(4, ratio * 100)}%` }}
                  />
                </div>
              ) : null}

              <div className="grid gap-2 md:grid-cols-3">
                <SummaryLine
                  label={t("share.ownerEmail", {
                    defaultValue: "Owner Email",
                  })}
                  value={share.ownerEmail || "-"}
                />
                <SummaryLine
                  label={t("share.sharedWithEmails", {
                    defaultValue: "Share To",
                  })}
                  value={shareToSummary || "-"}
                />
                <SummaryLine
                  label={t("share.forSale")}
                  value={t(
                    `share.forSaleOptions.${share.forSale.toLowerCase()}`,
                  )}
                />
                <MarketSummary
                  markets={usageMarkets}
                  marketAccessMode={currentMarketAccessMode}
                  selectedMarketEmails={currentMarketEmails}
                />
                <SummaryLine
                  label={t("share.description")}
                  value={share.description || "-"}
                />
                <SummaryLine
                  label={t("share.tokenLimit")}
                  value={
                    isUnlimitedTokenLimit(share.tokenLimit)
                      ? t("share.unlimited")
                      : String(share.tokenLimit)
                  }
                />
                <SummaryLine
                  label={t("share.expiresAt")}
                  value={
                    isPermanentExpiry(share.expiresAt)
                      ? t("share.expiry.permanentLabel")
                      : formatUtcDateTime(share.expiresAt)
                  }
                />
                <SummaryLine
                  label={t("share.parallelLimit")}
                  value={
                    isUnlimitedParallelLimit(share.parallelLimit)
                      ? t("share.unlimited")
                      : String(share.parallelLimit)
                  }
                />
              </div>
            </>
          ) : null}
        </section>

        {!readOnly && !hideRuntimeActions ? (
          <ShareRequestLogTable shareId={share.id} />
        ) : null}
      </CardContent>

      <EditShareDialog
        open={editDialogOpen}
        onOpenChange={setEditDialogOpen}
        share={share}
        markets={markets}
        providerSalePricing={providerSalePricing}
        marketsLoading={marketsLoading}
        marketsError={marketsError}
        readOnly={readOnly}
        subdomainReadOnly={subdomainReadOnly}
        onRetryMarkets={onRetryMarkets}
        isBusy={isBusy}
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
        providersByApp={
          providersByAppForEdit ?? { claude: [], codex: [], gemini: [] }
        }
        providerNameByKey={providerNameByKey}
        onUpdateProviderBinding={onUpdateProviderBinding}
        onRebindAtomic={onRebindAtomic}
      />
    </Card>
  );
}

function CollapsibleSectionHeader({
  title,
  expanded,
  onToggle,
}: {
  title: string;
  expanded: boolean;
  onToggle: () => void;
}) {
  const Icon = expanded ? ChevronDown : ChevronRight;
  return (
    <button
      type="button"
      aria-expanded={expanded}
      className="flex w-full items-center justify-between gap-3 rounded-md px-0 py-0 text-left text-sm font-semibold text-foreground"
      onClick={onToggle}
    >
      <span>{title}</span>
      <Icon className="h-4 w-4 text-muted-foreground" />
    </button>
  );
}

function MarketSummary({
  markets,
  marketAccessMode,
  selectedMarketEmails,
}: {
  markets: PublicMarket[];
  marketAccessMode: "selected" | "all";
  selectedMarketEmails: string[];
}) {
  const { t } = useTranslation();
  const marketByEmail = new Map(
    markets.map((market) => [market.email.toLowerCase(), market]),
  );

  return (
    <div className="min-w-0 rounded-md border border-border-default/70 bg-muted/10 px-3 py-2">
      <div className="text-xs text-muted-foreground">
        {t("share.market.title", { defaultValue: "Market" })}
      </div>
      <div className="mt-2">
        {marketAccessMode === "all" ? (
          <div className="text-sm text-muted-foreground">
            {t("share.market.allSelected", {
              defaultValue: "已选中所有 Market",
            })}
          </div>
        ) : selectedMarketEmails.length ? (
          <div className="flex flex-wrap gap-2">
            {selectedMarketEmails.map((email) => {
              const market = marketByEmail.get(email);
              return (
                <Badge
                  key={email}
                  variant="secondary"
                  className="max-w-full gap-1 rounded-md px-2 py-1 text-xs"
                >
                  <span className="min-w-0 truncate">
                    {market?.displayName ?? email}
                  </span>
                </Badge>
              );
            })}
          </div>
        ) : (
          <div className="break-all text-sm">
            {t("share.market.default", {
              defaultValue: "默认，不授权 Market",
            })}
          </div>
        )}
      </div>
    </div>
  );
}

function shareAccessSummary(share: ShareRecord, marketEmailSet: Set<string>) {
  const accessByApp = share.accessByApp ?? {};
  const apps = shareSupportedApps(share);
  const accessApps =
    Object.keys(accessByApp).length > 0
      ? apps.length
        ? apps
        : Object.keys(accessByApp)
      : [];
  if (accessApps.length === 0) {
    return Array.from(
      new Set(
        (share.sharedWithEmails ?? [])
          .map((email) => email.trim().toLowerCase())
          .filter((email) => email && !marketEmailSet.has(email)),
      ),
    )
      .sort()
      .join(", ");
  }

  return accessApps
    .map((app) => {
      const access = (
        accessByApp as Record<string, { sharedWithEmails?: string[] }>
      )[app];
      const emails = Array.from(
        new Set(
          (access?.sharedWithEmails ?? [])
            .map((email) => email.trim().toLowerCase())
            .filter((email) => email && !marketEmailSet.has(email)),
        ),
      ).sort();
      if (emails.length === 0) return null;
      return `${shareAppLabel(app)}: ${emails.join(", ")}`;
    })
    .filter(Boolean)
    .join(" / ");
}

function shareAppLabel(app: string) {
  if (app === "claude") return "Claude";
  if (app === "codex") return "Codex";
  if (app === "gemini") return "Gemini";
  return app;
}

function ConnectInlineValue({
  label,
  value,
  displayValue,
  action,
  onCopy,
}: {
  label: string;
  value: string;
  displayValue?: string;
  action?: ReactNode;
  onCopy: () => void;
}) {
  return (
    <div className="min-w-0 rounded-md border border-border-default bg-background/60 px-3 py-2">
      <div className="text-xs text-muted-foreground">{label}</div>
      <div className="mt-2 flex items-center justify-between gap-2">
        <div className="min-w-0 flex-1">
          <code className="block min-w-0 break-all text-xs">
            {displayValue ?? (value || "-")}
          </code>
        </div>
        {action}
        <Button
          variant="ghost"
          size="icon"
          className="h-7 w-7 shrink-0"
          disabled={!value}
          onClick={onCopy}
        >
          <Copy className="h-3.5 w-3.5" />
        </Button>
      </div>
    </div>
  );
}

function SummaryLine({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0 rounded-md border border-border-default/70 bg-muted/10 px-3 py-2">
      <div className="text-xs text-muted-foreground">{label}</div>
      <div className="mt-1 break-all text-sm">{value}</div>
    </div>
  );
}
