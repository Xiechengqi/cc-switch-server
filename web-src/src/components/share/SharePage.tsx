import { useCallback, useEffect, useMemo, useState } from "react";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import {
  authApi,
  shareApi,
  type AppId,
  type ShareAccessByApp,
  type ShareAppSettingsByApp,
  type ShareRecord,
  type ShareTunnelStatus,
} from "@/lib/api";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { useSettingsQuery } from "@/lib/query";
import { useProxyStatus } from "@/lib/query/proxy";
import {
  useConfigureTunnelMutation,
  useClaimClientTunnelMutation,
  useClientTunnelQuery,
  useCreateShareMutation,
  useDeleteShareMutation,
  useShareMarketsQuery,
  useDisableShareMutation,
  useEnableShareMutation,
  useProvidersQuery,
  useResetShareUsageMutation,
  useUpdateShareAclMutation,
  useSharesQuery,
  useUpdateShareDescriptionMutation,
  useUpdateShareExpirationMutation,
  useUpdateShareForSaleMutation,
  useUpdateShareForSaleOfficialPricePercentMutation,
  useUpdateShareOwnerEmailMutation,
  useUpdateShareParallelLimitMutation,
  useUpdateShareSubdomainMutation,
  useUpdateShareProviderBindingMutation,
  useUpdateShareTokenLimitMutation,
  useTransferShareOwnerMutation,
  useStartClientTunnelMutation,
  useStopClientTunnelMutation,
} from "@/lib/query";
import { shareKeys } from "@/lib/query/share";
import { extractErrorMessage } from "@/utils/errorUtils";
import { copyText } from "@/lib/clipboard";
import {
  getTunnelConfigFromSettings,
  isTunnelConfigured,
} from "@/utils/shareUtils";
import { CreateShareDialog } from "./CreateShareDialog";
import { ShareList } from "./ShareList";
import { ShareRouterBar } from "./ShareRouterBar";
import {
  buildProviderOption,
  getProviderAccountLabel,
  SHARE_PROVIDER_AUTH_PROVIDERS,
  type ManagedAuthStatusByProvider,
} from "./providerOptions";
import type { Provider } from "@/types";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  clearRouterSessionTokens,
  getRouterSessionStatus,
  requestRouterEmailCodeWithIdentityRetry,
  verifyRouterEmailCode,
  type RouterSessionStatus,
} from "@/lib/routerAuth";
import {
  updateRouterShareSettings,
  type RouterShareSettingsPatch,
} from "@/lib/routerShare";

const SHARE_PROVIDER_APPS = [
  { app: "claude", label: "Claude" },
  { app: "codex", label: "Codex" },
  { app: "gemini", label: "Gemini" },
] as const;

interface SharePageProps {
  defaultApp?: AppId;
  shareScoped?: boolean;
  readOnly?: boolean;
}

export function SharePage({
  defaultApp,
  shareScoped = false,
  readOnly = false,
}: SharePageProps) {
  const { t } = useTranslation();
  const { data: shares = [], isLoading, error, refetch } = useSharesQuery();
  const { data: settings } = useSettingsQuery();
  const { data: proxyStatus } = useProxyStatus();
  const queryClient = useQueryClient();
  const {
    session: routerSession,
    loading: routerSessionLoading,
    refresh: refreshRouterSession,
  } = useRouterSession(shareScoped);
  const claudeProvidersQuery = useProvidersQuery("claude");
  const codexProvidersQuery = useProvidersQuery("codex");
  const geminiProvidersQuery = useProvidersQuery("gemini");
  const managedAuthStatusResults = useQueries({
    queries: SHARE_PROVIDER_AUTH_PROVIDERS.map((authProvider) => ({
      queryKey: ["managed-auth-status", authProvider],
      queryFn: () => authApi.authGetStatus(authProvider),
      staleTime: 30000,
    })),
  });
  const deepSeekAccountStatusResult = useQuery({
    queryKey: ["deepseek-account-status"],
    queryFn: () => authApi.deepseekAccountStatus(),
    staleTime: 30000,
  });
  const managedAuthStatusVersion = managedAuthStatusResults
    .map((result) => String(result.dataUpdatedAt))
    .concat(String(deepSeekAccountStatusResult.dataUpdatedAt))
    .join(":");
  const managedAuthStatuses = useMemo<ManagedAuthStatusByProvider>(() => {
    const result: ManagedAuthStatusByProvider = {};
    SHARE_PROVIDER_AUTH_PROVIDERS.forEach((authProvider, index) => {
      const status = managedAuthStatusResults[index]?.data;
      if (status) result[authProvider] = status;
    });
    if (deepSeekAccountStatusResult.data) {
      result.deepseek_account = deepSeekAccountStatusResult.data;
    }
    return result;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [managedAuthStatusVersion]);

  // C-1：保留近期 share-needs-rebind 事件以便在页面顶部常驻红条提示。
  // toast 弹一下就消失，对于"已经发生且未解决"的状态需要更显著的存在感。
  const [needsRebindMap, setNeedsRebindMap] = useState<
    Record<string, { reason: string; at: number }>
  >({});

  // share 路径在请求阶段发现绑定 provider 缺失 / app_type 不匹配时，后端会 emit
  // `share-needs-rebind`。前端 toast 提示用户去改绑，并 invalidate shares 查询
  // 让状态条立刻刷新（后端已把该 share 改成 paused）。
  useTauriEvent<{
    shareId: string;
    appType: string;
    reason: string;
    detail?: string | null;
  }>("share-needs-rebind", (payload) => {
    setNeedsRebindMap((prev) => ({
      ...prev,
      [payload.shareId]: { reason: payload.reason, at: Date.now() },
    }));
    toast.error(
      t("share.needsRebindTitle", {
        defaultValue: "Share 绑定的 provider 已失效",
      }),
      {
        description: t("share.needsRebindBody", {
          defaultValue:
            "share_id={{shareId}} app={{appType}} 原因={{reason}}。请在 Share 页面改绑或删除该 share。",
          shareId: payload.shareId,
          appType: payload.appType,
          reason: payload.reason,
        }),
      },
    );
    void queryClient.invalidateQueries({ queryKey: shareKeys.lists() });
  });
  const tunnelConfigured = useMemo(
    () => isTunnelConfigured(settings),
    [settings],
  );
  const tunnelConfig = useMemo(
    () => getTunnelConfigFromSettings(settings),
    [settings],
  );
  const [createOpen, setCreateOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<ShareRecord | null>(null);
  const [pendingActionShareId, setPendingActionShareId] = useState<
    string | null
  >(null);

  const createMutation = useCreateShareMutation();
  const deleteMutation = useDeleteShareMutation();
  const enableMutation = useEnableShareMutation();
  const disableMutation = useDisableShareMutation();
  const resetUsageMutation = useResetShareUsageMutation();
  const updateDescriptionMutation = useUpdateShareDescriptionMutation();
  const updateForSaleMutation = useUpdateShareForSaleMutation();
  const updateSharePricingMutation =
    useUpdateShareForSaleOfficialPricePercentMutation();
  const updateExpirationMutation = useUpdateShareExpirationMutation();
  const updateOwnerEmailMutation = useUpdateShareOwnerEmailMutation();
  const transferOwnerMutation = useTransferShareOwnerMutation();
  const updateAclMutation = useUpdateShareAclMutation();
  const updateParallelLimitMutation = useUpdateShareParallelLimitMutation();
  const updateSubdomainMutation = useUpdateShareSubdomainMutation();
  const updateProviderBindingMutation = useUpdateShareProviderBindingMutation();
  const updateTokenLimitMutation = useUpdateShareTokenLimitMutation();
  const configureTunnelMutation = useConfigureTunnelMutation();
  const clientTunnelQuery = useClientTunnelQuery(!shareScoped);
  const claimClientTunnelMutation = useClaimClientTunnelMutation();
  const startClientTunnelMutation = useStartClientTunnelMutation();
  const stopClientTunnelMutation = useStopClientTunnelMutation();
  const [clientOwnerEmailInput, setClientOwnerEmailInput] = useState("");
  const [clientSubdomainInput, setClientSubdomainInput] = useState("");
  const clientTunnel = clientTunnelQuery.data;

  useEffect(() => {
    if (clientTunnel?.config?.ownerEmail) {
      setClientOwnerEmailInput(clientTunnel.config.ownerEmail);
    }
  }, [clientTunnel?.config?.ownerEmail]);

  useEffect(() => {
    if (clientTunnel?.config?.subdomain) {
      setClientSubdomainInput(clientTunnel.config.subdomain);
    }
  }, [clientTunnel?.config?.subdomain]);
  const {
    data: markets = [],
    isLoading: marketsLoading,
    error: marketsError,
    refetch: refetchMarkets,
  } = useShareMarketsQuery();
  const providerQueries = useMemo(
    () => ({
      claude: claudeProvidersQuery.data,
      codex: codexProvidersQuery.data,
      gemini: geminiProvidersQuery.data,
    }),
    [
      claudeProvidersQuery.data,
      codexProvidersQuery.data,
      geminiProvidersQuery.data,
    ],
  );
  const providerSalePricing = useMemo(
    () =>
      SHARE_PROVIDER_APPS.map(({ app, label }) => {
        const data = providerQueries[app];
        const provider: Provider | undefined =
          data?.providers?.[data.currentProviderId];
        return {
          app,
          label,
          providerName: provider?.name,
          percent: provider?.meta?.forSaleOfficialPricePercent,
        };
      }),
    [providerQueries],
  );

  const tunnelQueries = useQueries({
    queries: shares.map((share) => ({
      queryKey: shareKeys.tunnelStatus(share.id),
      queryFn: () => shareApi.getTunnelStatus(share.id),
      enabled: share.status === "active",
      refetchInterval: share.status === "active" ? 8000 : false,
      refetchIntervalInBackground: true,
    })),
  });

  const tunnelRuntimeStatusMap = useMemo<
    Record<string, ShareTunnelStatus | null>
  >(
    () =>
      Object.fromEntries(
        shares.map((share, index) => [
          share.id,
          tunnelQueries[index]?.data ?? null,
        ]),
      ),
    [shares, tunnelQueries],
  );

  const tunnelStatusMap = useMemo(
    () =>
      Object.fromEntries(
        shares.map((share) => [
          share.id,
          tunnelRuntimeStatusMap[share.id]?.info ?? null,
        ]),
      ),
    [shares, tunnelRuntimeStatusMap],
  );

  const primaryShare = shares[0] ?? null;
  const routerSessionEmail = routerSession?.user?.email?.trim().toLowerCase();
  const primaryShareOwnerEmail = primaryShare?.ownerEmail?.trim().toLowerCase();
  const normalizedClientOwnerEmail = clientOwnerEmailInput.trim().toLowerCase();
  const clientTunnelSaving = claimClientTunnelMutation.isPending;

  const saveClientTunnel = useCallback(
    (ownerEmail: string = normalizedClientOwnerEmail) =>
      claimClientTunnelMutation.mutateAsync({
        ownerEmail,
        subdomain: clientSubdomainInput.trim(),
        enabled: true,
        autoStart: true,
      }),
    [
      claimClientTunnelMutation,
      clientSubdomainInput,
      normalizedClientOwnerEmail,
    ],
  );

  const handleSaveClientTunnel = useCallback(() => {
    if (!clientSubdomainInput.trim() || !normalizedClientOwnerEmail) return;
    void saveClientTunnel();
  }, [clientSubdomainInput, normalizedClientOwnerEmail, saveClientTunnel]);
  const canManageShareFromRouter =
    shareScoped &&
    Boolean(routerSession?.authenticated) &&
    Boolean(routerSessionEmail) &&
    routerSessionEmail === primaryShareOwnerEmail;
  const effectiveReadOnly =
    readOnly || (shareScoped && !canManageShareFromRouter);
  const writeSharePatch = async (
    share: ShareRecord,
    patch: RouterShareSettingsPatch,
  ) => {
    const result = await updateRouterShareSettings(share.id, patch);
    toast.success(
      result.appliedSynchronously
        ? t("share.routerEdit.applied", {
            defaultValue: "配置已应用",
          })
        : t("share.routerEdit.queued", {
            defaultValue: "配置修改已提交，等待桌面端同步",
          }),
    );
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: shareKeys.list() }),
      queryClient.invalidateQueries({ queryKey: shareKeys.detail(share.id) }),
    ]);
    await refetch();
  };

  const fixedBoundProviderIds = useMemo(() => {
    const takenProviderIds = new Set<string>();
    shares.forEach((share) => {
      if (share.status === "deleted") return;
      const dynamicApps = new Set(share.dynamicApps ?? []);
      (["claude", "codex", "gemini"] as const).forEach((app) => {
        if (dynamicApps.has(app)) return;
        const pid = share.bindings?.[app];
        if (pid) takenProviderIds.add(pid);
      });
    });
    return takenProviderIds;
  }, [shares]);

  // 构造 CreateShareDialog 用的 provider 选项：按 defaultApp 过滤，
  // 把"已被其他固定 share slot 绑定"的 provider 标灰禁选。share ↔ fixed provider
  // P8 多 app share：dialogProviderOptions 现在只用于 CreateShareDialog 默认聚焦的 slot
  // 兼容路径，真正的"哪些 provider 可选"由下面 providersByApp 全量提供。这里保留是为了
  // 给老的、按"当前 app 单 slot"语义渲染的入口（例如 ProxyToggle）继续提供候选。
  // 严格 1:1，需要前端在选择阶段提前阻断冲突。
  const dialogProviderOptions = useMemo(() => {
    if (!defaultApp) return [];
    const queryData =
      providerQueries[defaultApp as "claude" | "codex" | "gemini"];
    if (!queryData) return [];
    return Object.values(queryData.providers ?? {})
      .filter((provider): provider is Provider => Boolean(provider))
      .sort((a, b) =>
        a.id === queryData.currentProviderId
          ? -1
          : b.id === queryData.currentProviderId
            ? 1
            : 0,
      )
      .map((provider) => ({
        ...buildProviderOption(
          provider,
          fixedBoundProviderIds.has(provider.id),
          managedAuthStatuses,
        ),
      }));
  }, [defaultApp, providerQueries, fixedBoundProviderIds, managedAuthStatuses]);

  // ShareCard 上"绑定 provider"显示名的查找表，key = `{appType}:{providerId}`。
  // 由 SharePage 在 provider 查询完成后统一计算，避免 Card 自己持有 query 句柄。
  const providerNameByKey = useMemo(() => {
    const map: Record<string, string> = {};
    (["claude", "codex", "gemini"] as const).forEach((app) => {
      const data = providerQueries[app];
      if (!data) return;
      Object.values(data.providers ?? {}).forEach((provider) => {
        if (provider) {
          map[`${app}:${provider.id}`] = provider.name;
        }
      });
    });
    return map;
  }, [providerQueries]);

  const providerAccountByKey = useMemo(() => {
    const map: Record<string, string> = {};
    (["claude", "codex", "gemini"] as const).forEach((app) => {
      const data = providerQueries[app];
      if (!data) return;
      Object.values(data.providers ?? {}).forEach((provider) => {
        if (!provider) return;
        const account = getProviderAccountLabel(provider, managedAuthStatuses);
        if (account) {
          map[`${app}:${provider.id}`] = account;
        }
      });
    });
    return map;
  }, [providerQueries, managedAuthStatuses]);

  // P8 多 app share：每个 app slot 的可绑定 provider 列表。CreateShareDialog 和
  // EditShareDialog 都按 `providersByApp[app]` 取候选，ShareList 那一层再为每条 share
  // 当前 slot 已绑定的 provider 取消 disabled（让"保持原 provider"始终可选）。
  const providersByApp = useMemo(() => {
    const result: Partial<
      Record<"claude" | "codex" | "gemini", typeof dialogProviderOptions>
    > = {};
    (["claude", "codex", "gemini"] as const).forEach((app) => {
      const data = providerQueries[app];
      if (!data) {
        result[app] = [];
        return;
      }
      result[app] = Object.values(data.providers ?? {})
        .filter((provider): provider is Provider => Boolean(provider))
        .sort((a, b) =>
          a.id === data.currentProviderId
            ? -1
            : b.id === data.currentProviderId
              ? 1
              : 0,
        )
        .map((provider) =>
          buildProviderOption(
            provider,
            fixedBoundProviderIds.has(provider.id),
            managedAuthStatuses,
          ),
        );
    });
    return result;
  }, [
    providerQueries,
    fixedBoundProviderIds,
    dialogProviderOptions,
    managedAuthStatuses,
  ]);

  const handleCreate = async (
    params: Parameters<typeof createMutation.mutateAsync>[0],
    extras: {
      sharedWithEmails: string[];
      marketAccessMode: "selected" | "all";
      saleMarketKind?: "token" | "share";
      accessByApp?: ShareAccessByApp;
      appSettings?: ShareAppSettingsByApp;
    },
  ) => {
    const createParams =
      extras.saleMarketKind === "share"
        ? { ...params, saleMarketKind: "token" as const }
        : params;
    const created = await createMutation.mutateAsync(createParams);
    const hasPerAppAccess =
      !!extras.accessByApp && Object.keys(extras.accessByApp).length > 0;
    const hasPerAppSettings =
      !!extras.appSettings && Object.keys(extras.appSettings).length > 0;
    try {
      if (
        extras.saleMarketKind === "share" ||
        extras.marketAccessMode === "all" ||
        extras.sharedWithEmails.length > 0 ||
        hasPerAppAccess ||
        hasPerAppSettings
      ) {
        await updateAclMutation.mutateAsync({
          shareId: created.id,
          sharedWithEmails: extras.sharedWithEmails,
          marketAccessMode: extras.marketAccessMode,
          accessByApp: extras.accessByApp,
          appSettings: extras.appSettings,
          saleMarketKind: extras.saleMarketKind ?? "token",
        });
      }
    } catch (error) {
      try {
        await deleteMutation.mutateAsync(created.id);
      } catch (rollbackError) {
        throw new Error(
          `${extractErrorMessage(error)}；回滚删除失败：${extractErrorMessage(rollbackError)}`,
        );
      }
      throw error;
    }
    setCreateOpen(false);
    await runShareAction(created, () => enableMutation.mutateAsync(created.id));
  };

  const runShareAction = async (
    share: ShareRecord,
    action: () => Promise<unknown>,
  ) => {
    setPendingActionShareId(share.id);
    try {
      await action();
    } finally {
      setPendingActionShareId(null);
    }
  };

  return (
    <div className="px-6 py-4">
      <div className="mx-auto flex max-w-7xl flex-col gap-5 pb-10">
        {shareScoped ? (
          <ShareOwnerAuthBar
            share={primaryShare}
            session={routerSession}
            loading={routerSessionLoading}
            canManageShare={canManageShareFromRouter}
            onRefresh={refreshRouterSession}
          />
        ) : null}

        {/* C-1：share-needs-rebind 常驻横幅。任意 share 进入 needs-rebind 状态都
            列在这里，点击可定位到该 share 的卡片。 */}
        {Object.keys(needsRebindMap).length > 0 ? (
          <div className="rounded-xl border border-destructive/40 bg-destructive/10 px-4 py-3">
            <div className="text-sm font-medium text-destructive">
              {t("share.needsRebindBannerTitle", {
                defaultValue:
                  "以下 share 的绑定 provider 已失效，请改绑或删除：",
              })}
            </div>
            <ul className="mt-2 space-y-1 text-xs">
              {Object.entries(needsRebindMap).map(([id, info]) => {
                const share = shares.find((s) => s.id === id);
                return (
                  <li key={id} className="flex items-center justify-between">
                    <span>
                      <span className="font-mono">{share?.name ?? id}</span>
                      <span className="ml-2 text-muted-foreground">
                        {info.reason}
                      </span>
                    </span>
                    <button
                      type="button"
                      className="text-xs underline"
                      onClick={() =>
                        setNeedsRebindMap((prev) => {
                          const next = { ...prev };
                          delete next[id];
                          return next;
                        })
                      }
                    >
                      {t("share.needsRebindDismiss", {
                        defaultValue: "忽略",
                      })}
                    </button>
                  </li>
                );
              })}
            </ul>
          </div>
        ) : null}

        <ShareRouterBar
          proxyRunning={proxyStatus?.running ?? false}
          proxyAddress={proxyStatus?.address ?? null}
          proxyPort={proxyStatus?.port ?? null}
          hasShare={shares.length > 0}
          readOnly={effectiveReadOnly || shareScoped}
          onCreate={() => setCreateOpen(true)}
        />

        {!shareScoped ? (
          <div className="rounded-lg border bg-card px-4 py-3">
            <div className="flex flex-col gap-3 lg:flex-row lg:items-end lg:justify-between">
              <div className="grid flex-1 gap-3 md:grid-cols-[minmax(180px,1fr)_minmax(180px,1fr)_minmax(220px,1.4fr)]">
                <div>
                  <div className="text-xs font-medium text-muted-foreground">
                    Client Tunnel Owner
                  </div>
                  <Input
                    className="mt-1 h-8"
                    type="email"
                    value={clientOwnerEmailInput}
                    placeholder="owner@example.com"
                    disabled={clientTunnelQuery.isLoading || clientTunnelSaving}
                    onChange={(event) =>
                      setClientOwnerEmailInput(event.target.value)
                    }
                  />
                </div>
                <div>
                  <div className="text-xs font-medium text-muted-foreground">
                    Client Subdomain
                  </div>
                  <Input
                    className="mt-1 h-8"
                    value={clientSubdomainInput}
                    disabled={clientTunnelQuery.isLoading || clientTunnelSaving}
                    onChange={(event) =>
                      setClientSubdomainInput(event.target.value)
                    }
                  />
                </div>
                <div>
                  <div className="text-xs font-medium text-muted-foreground">
                    Client URL
                  </div>
                  <button
                    type="button"
                    className="mt-2 block max-w-full truncate text-left text-sm underline-offset-4 hover:underline"
                    disabled={!clientTunnel?.config?.tunnelUrl}
                    onClick={() => {
                      if (clientTunnel?.config?.tunnelUrl) {
                        void copyText(clientTunnel.config.tunnelUrl).then(() =>
                          toast.success("URL 已复制"),
                        );
                      }
                    }}
                  >
                    {clientTunnel?.config?.tunnelUrl ?? "-"}
                  </button>
                </div>
              </div>
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-xs text-muted-foreground">
                  {clientTunnel?.status?.info
                    ? "运行中"
                    : clientTunnel?.status?.lastError
                      ? `失败: ${clientTunnel.status.lastError}`
                      : "未运行"}
                </span>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={
                    !clientSubdomainInput.trim() ||
                    !normalizedClientOwnerEmail ||
                    clientTunnelSaving
                  }
                  onClick={handleSaveClientTunnel}
                >
                  保存
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={startClientTunnelMutation.isPending}
                  onClick={() => startClientTunnelMutation.mutate()}
                >
                  启动
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={
                    !clientTunnel?.status?.info ||
                    stopClientTunnelMutation.isPending
                  }
                  onClick={() => stopClientTunnelMutation.mutate()}
                >
                  停止
                </Button>
              </div>
            </div>
          </div>
        ) : null}

        <ShareList
          shares={shares}
          tunnelStatusMap={tunnelStatusMap}
          tunnelConfig={tunnelConfig}
          tunnelConfigured={tunnelConfigured}
          isLoading={isLoading}
          error={error ? extractErrorMessage(error) : null}
          pendingAction={pendingActionShareId}
          markets={markets}
          providerSalePricing={providerSalePricing}
          providerNameByKey={providerNameByKey}
          providerAccountByKey={providerAccountByKey}
          providersByApp={providersByApp}
          marketsLoading={marketsLoading}
          marketsError={marketsError ? extractErrorMessage(marketsError) : null}
          readOnly={effectiveReadOnly}
          hideRuntimeActions={shareScoped}
          subdomainReadOnly={shareScoped}
          onRetryMarkets={() => void refetchMarkets()}
          onRetry={() => void refetch()}
          onCreate={() => setCreateOpen(true)}
          onDelete={(share) => {
            if (!shareScoped) setDeleteTarget(share);
          }}
          onEnable={(share) =>
            void runShareAction(share, () =>
              enableMutation.mutateAsync(share.id),
            ).catch(() => undefined)
          }
          onDisable={(share) =>
            void runShareAction(share, () =>
              disableMutation.mutateAsync(share.id),
            ).catch(() => undefined)
          }
          onResetUsage={(share) =>
            runShareAction(share, () =>
              resetUsageMutation.mutateAsync(share.id),
            )
          }
          onUpdateSubdomain={(share, subdomain) =>
            runShareAction(share, () =>
              shareScoped
                ? Promise.reject(
                    new Error(
                      t("share.routerEdit.subdomainReadOnly", {
                        defaultValue: "Share URL 页面暂不支持修改 subdomain",
                      }),
                    ),
                  )
                : updateSubdomainMutation.mutateAsync({
                    shareId: share.id,
                    subdomain,
                  }),
            )
          }
          onUpdateDescription={(share, description) =>
            runShareAction(share, () =>
              shareScoped
                ? writeSharePatch(share, { description: description || null })
                : updateDescriptionMutation.mutateAsync({
                    shareId: share.id,
                    description,
                  }),
            )
          }
          onUpdateForSale={(share, forSale) =>
            runShareAction(share, () =>
              shareScoped
                ? writeSharePatch(share, { forSale })
                : updateForSaleMutation.mutateAsync({
                    shareId: share.id,
                    forSale,
                  }),
            )
          }
          onUpdateShareSalePricing={(share, pricing) =>
            runShareAction(share, () =>
              shareScoped
                ? writeSharePatch(share, {
                    forSaleOfficialPricePercentByApp: pricing,
                  })
                : updateSharePricingMutation.mutateAsync({
                    shareId: share.id,
                    pricing,
                  }),
            )
          }
          onUpdateExpiration={(share, expiresAt) =>
            runShareAction(share, () =>
              shareScoped
                ? writeSharePatch(share, { expiresAt })
                : updateExpirationMutation.mutateAsync({
                    shareId: share.id,
                    expiresAt,
                  }),
            )
          }
          onUpdateOwnerEmail={(share, ownerEmail) =>
            runShareAction(share, () =>
              shareScoped
                ? writeSharePatch(share, { ownerEmail })
                : updateOwnerEmailMutation.mutateAsync({
                    shareId: share.id,
                    ownerEmail,
                  }),
            )
          }
          onTransferOwner={(share, targetEmail) =>
            runShareAction(share, () =>
              shareScoped
                ? writeSharePatch(share, { ownerEmail: targetEmail })
                : transferOwnerMutation.mutateAsync({
                    shareId: share.id,
                    targetEmail,
                  }),
            )
          }
          onUpdateAcl={(
            share,
            sharedWithEmails,
            marketAccessMode,
            accessByApp,
            saleMarketKind,
            appSettings,
          ) =>
            runShareAction(share, () =>
              shareScoped
                ? writeSharePatch(share, {
                    sharedWithEmails,
                    marketAccessMode,
                    accessByApp,
                    appSettings,
                    saleMarketKind:
                      saleMarketKind ?? share.saleMarketKind ?? "token",
                  })
                : updateAclMutation.mutateAsync({
                    shareId: share.id,
                    sharedWithEmails,
                    marketAccessMode,
                    accessByApp,
                    appSettings,
                    saleMarketKind:
                      saleMarketKind ?? share.saleMarketKind ?? "token",
                  }),
            )
          }
          onUpdateProviderBinding={(share, appType, providerId, options) =>
            runShareAction(share, () =>
              shareScoped
                ? Promise.resolve()
                : updateProviderBindingMutation.mutateAsync({
                    shareId: share.id,
                    appType,
                    providerId,
                    dynamic: options?.dynamic,
                  }),
            )
          }
          onRebindAtomic={(share, appType, providerId, options) =>
            runShareAction(share, async () => {
              if (shareScoped) return;
              // A-3：active share 上一键改绑 = disable tunnel → update binding → enable tunnel。
              // 中间任一步失败会留下 share 在 paused/active 中间态——错误会
              // 通过 mutation toast 暴露，用户可在 UI 手动恢复。
              await disableMutation.mutateAsync(share.id);
              await updateProviderBindingMutation.mutateAsync({
                shareId: share.id,
                appType,
                providerId,
                dynamic: options?.dynamic,
              });
              await enableMutation.mutateAsync(share.id);
            })
          }
          onUpdateTokenLimit={(share, tokenLimit) =>
            runShareAction(share, () =>
              shareScoped
                ? writeSharePatch(share, { tokenLimit })
                : updateTokenLimitMutation.mutateAsync({
                    shareId: share.id,
                    tokenLimit,
                  }),
            )
          }
          onUpdateParallelLimit={(share, parallelLimit) =>
            runShareAction(share, () =>
              shareScoped
                ? writeSharePatch(share, { parallelLimit })
                : updateParallelLimitMutation.mutateAsync({
                    shareId: share.id,
                    parallelLimit,
                  }),
            )
          }
        />
      </div>

      {!shareScoped ? (
        <CreateShareDialog
          open={createOpen}
          onOpenChange={setCreateOpen}
          defaultApp={defaultApp}
          ownerEmail={primaryShare?.ownerEmail ?? null}
          tunnelConfig={tunnelConfig}
          tunnelConfigSaving={configureTunnelMutation.isPending}
          isSubmitting={createMutation.isPending || enableMutation.isPending}
          markets={markets}
          marketsLoading={marketsLoading}
          marketsError={marketsError ? extractErrorMessage(marketsError) : null}
          providersByApp={
            providersByApp as Record<
              "claude" | "codex" | "gemini",
              typeof dialogProviderOptions
            >
          }
          onSaveTunnelConfig={(config) =>
            configureTunnelMutation.mutateAsync(config)
          }
          onRetryMarkets={() => void refetchMarkets()}
          onSubmit={handleCreate}
        />
      ) : null}

      <ConfirmDialog
        isOpen={Boolean(deleteTarget)}
        title={t("share.confirmDeleteTitle")}
        message={t("share.confirmDeleteMessage", {
          name: deleteTarget?.name ?? "",
        })}
        onCancel={() => setDeleteTarget(null)}
        onConfirm={() => {
          if (!deleteTarget) return;
          void runShareAction(deleteTarget, async () => {
            await deleteMutation.mutateAsync(deleteTarget.id);
            setDeleteTarget(null);
          });
        }}
      />
    </div>
  );
}

function useRouterSession(enabled: boolean): {
  session: RouterSessionStatus | null;
  loading: boolean;
  refresh: () => Promise<void>;
} {
  const [session, setSession] = useState<RouterSessionStatus | null>(null);
  const [loading, setLoading] = useState(enabled);

  const refresh = useCallback(async () => {
    if (!enabled) return;
    setLoading(true);
    try {
      setSession(await getRouterSessionStatus());
    } catch (error) {
      console.error("[SharePage] Failed to refresh router session", error);
      setSession({ authenticated: false });
    } finally {
      setLoading(false);
    }
  }, [enabled]);

  useEffect(() => {
    if (!enabled) return;
    void refresh();
    const handleAuthChanged = () => void refresh();
    window.addEventListener("router-auth-changed", handleAuthChanged);
    const interval = window.setInterval(() => void refresh(), 60_000);
    return () => {
      window.removeEventListener("router-auth-changed", handleAuthChanged);
      window.clearInterval(interval);
    };
  }, [enabled, refresh]);

  return { session, loading, refresh };
}

function maskEmail(email: string): string {
  const trimmed = email.trim();
  const at = trimmed.indexOf("@");
  if (at <= 0) return trimmed;
  const local = trimmed.slice(0, at);
  const domain = trimmed.slice(at);
  if (local.length <= 1) return `${local}***${domain}`;
  return `${local[0]}${"*".repeat(Math.max(3, local.length - 1))}${domain}`;
}

function ShareOwnerAuthBar({
  share,
  session,
  loading,
  canManageShare,
  onRefresh,
}: {
  share: ShareRecord | null;
  session: RouterSessionStatus | null;
  loading: boolean;
  canManageShare: boolean;
  onRefresh: () => Promise<void>;
}) {
  const { t } = useTranslation();
  const [step, setStep] = useState<"email" | "code">("email");
  const [email, setEmail] = useState("");
  const [code, setCode] = useState("");
  const [maskedDestination, setMaskedDestination] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const sendCode = async () => {
    const normalizedEmail = email.trim().toLowerCase();
    if (!normalizedEmail) return;
    setBusy(true);
    setError("");
    try {
      const result =
        await requestRouterEmailCodeWithIdentityRetry(normalizedEmail);
      setEmail(normalizedEmail);
      setMaskedDestination(
        result.maskedDestination || maskEmail(normalizedEmail),
      );
      setCode("");
      setStep("code");
    } catch (err) {
      setError(extractErrorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const verifyCode = async () => {
    if (!email.trim() || code.trim().length < 6) return;
    setBusy(true);
    setError("");
    try {
      await verifyRouterEmailCode(email.trim().toLowerCase(), code.trim());
      await onRefresh();
      setCode("");
    } catch (err) {
      setError(extractErrorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const logout = async () => {
    clearRouterSessionTokens();
    await onRefresh();
  };

  const sessionEmail = session?.user?.email ?? null;

  return (
    <div className="rounded-md border border-border-default/70 bg-card/80 px-4 py-3">
      <div className="flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
        <div className="text-sm">
          {loading ? (
            <span className="text-muted-foreground">{t("common.loading")}</span>
          ) : canManageShare ? (
            <span className="text-emerald-700 dark:text-emerald-300">
              {t("share.routerOwner.signedInOwner", {
                defaultValue: "已以 owner {{email}} 登录，可编辑配置",
                email: sessionEmail ?? "",
              })}
            </span>
          ) : session?.authenticated ? (
            <span className="text-amber-700 dark:text-amber-300">
              {t("share.routerOwner.signedInReadOnly", {
                defaultValue:
                  "当前登录 {{email}}，只有 owner {{owner}} 可以编辑配置",
                email: sessionEmail ?? "",
                owner: share?.ownerEmail ?? "-",
              })}
            </span>
          ) : (
            <span className="text-muted-foreground">
              {t("share.routerOwner.signInPrompt", {
                defaultValue: "使用 share owner 邮箱登录后可编辑配置",
              })}
            </span>
          )}
        </div>

        {session?.authenticated ? (
          <Button variant="outline" size="sm" onClick={() => void logout()}>
            {t("common.logout", { defaultValue: "退出登录" })}
          </Button>
        ) : (
          <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
            {step === "email" ? (
              <>
                <Input
                  type="email"
                  value={email}
                  disabled={busy}
                  placeholder={share?.ownerEmail || "owner@example.com"}
                  className="h-8 min-w-64"
                  onChange={(event) => setEmail(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      event.preventDefault();
                      void sendCode();
                    }
                  }}
                />
                <Button
                  size="sm"
                  disabled={busy || !email.trim()}
                  onClick={() => void sendCode()}
                >
                  {busy
                    ? t("common.loading")
                    : t("share.routerOwner.sendCode", {
                        defaultValue: "发送验证码",
                      })}
                </Button>
              </>
            ) : (
              <>
                <Input
                  value={code}
                  disabled={busy}
                  inputMode="numeric"
                  maxLength={6}
                  placeholder={t("share.routerOwner.codePlaceholder", {
                    defaultValue: "验证码",
                  })}
                  className="h-8 min-w-32"
                  onChange={(event) => setCode(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      event.preventDefault();
                      void verifyCode();
                    }
                  }}
                />
                <Button
                  size="sm"
                  disabled={busy || code.trim().length < 6}
                  onClick={() => void verifyCode()}
                >
                  {busy
                    ? t("common.loading")
                    : t("share.routerOwner.verify", {
                        defaultValue: "验证",
                      })}
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={busy}
                  onClick={() => {
                    setStep("email");
                    setCode("");
                    setError("");
                  }}
                >
                  {t("share.routerOwner.changeEmail", {
                    defaultValue: "换邮箱",
                  })}
                </Button>
              </>
            )}
          </div>
        )}
      </div>
      {step === "code" && !session?.authenticated ? (
        <div className="mt-2 text-xs text-muted-foreground">
          {t("share.routerOwner.codeSent", {
            defaultValue: "验证码已发送到 {{target}}",
            target: maskedDestination || maskEmail(email),
          })}
        </div>
      ) : null}
      {error ? (
        <div className="mt-2 text-xs text-destructive">{error}</div>
      ) : null}
    </div>
  );
}
