import { useCallback, useEffect, useMemo, useState } from "react";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import {
  authApi,
  shareApi,
  type AppId,
  type ShareRecord,
  type ShareTunnelStatus,
} from "@/lib/api";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { useSettingsQuery } from "@/lib/query";
import {
  useDeleteShareMutation,
  useDisableShareMutation,
  useEnableShareMutation,
  useClientTunnelQuery,
  useProvidersQuery,
  useResetShareUsageMutation,
  useSharesQuery,
} from "@/lib/query";
import { shareKeys } from "@/lib/query/share";
import { extractErrorMessage } from "@/utils/errorUtils";
import {
  getTunnelConfigFromSettings,
  isTunnelConfigured,
} from "@/utils/shareUtils";
import { ShareList } from "./ShareList";
import {
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

const SHARE_PROVIDER_APPS = [
  { app: "claude", label: "Claude" },
  { app: "codex", label: "Codex" },
  { app: "gemini", label: "Gemini" },
] as const;

interface SharePageProps {
  defaultApp?: AppId;
  shareScoped?: boolean;
  readOnly?: boolean;
  onOpenShareSettings?: () => void;
}

export function SharePage({
  defaultApp,
  shareScoped = false,
  readOnly = true,
  onOpenShareSettings,
}: SharePageProps) {
  const { t } = useTranslation();
  const { data: shares = [], isLoading, error, refetch } = useSharesQuery();
  const { data: settings } = useSettingsQuery();
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
  const [deleteTarget, setDeleteTarget] = useState<ShareRecord | null>(null);
  const [pendingActionShareId, setPendingActionShareId] = useState<
    string | null
  >(null);

  const deleteMutation = useDeleteShareMutation();
  const enableMutation = useEnableShareMutation();
  const disableMutation = useDisableShareMutation();
  const resetUsageMutation = useResetShareUsageMutation();
  const clientTunnelQuery = useClientTunnelQuery(!shareScoped);
  const clientTunnel = clientTunnelQuery.data;
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
  const clientTunnelConfigured = Boolean(
    clientTunnel?.config?.ownerEmail?.trim() &&
      clientTunnel?.config?.subdomain?.trim(),
  );
  const canManageShareFromRouter =
    shareScoped &&
    Boolean(routerSession?.authenticated) &&
    Boolean(routerSessionEmail) &&
    routerSessionEmail === primaryShareOwnerEmail;
  const effectiveReadOnly =
    readOnly || (shareScoped && !canManageShareFromRouter);

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

        {!shareScoped && !clientTunnelConfigured && onOpenShareSettings ? (
          <div className="rounded-xl border border-amber-500/30 bg-amber-500/10 px-4 py-3">
            <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
              <p className="text-sm text-amber-800 dark:text-amber-200">
                {t("share.settingsSetupHint", {
                  defaultValue:
                    "请先在设置 → 分享中配置默认 Router 节点与 Client Tunnel。",
                })}
              </p>
              <Button
                variant="outline"
                size="sm"
                className="shrink-0"
                onClick={onOpenShareSettings}
              >
                {t("share.openShareSettings", {
                  defaultValue: "前往分享设置",
                })}
              </Button>
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
          providerSalePricing={providerSalePricing}
          providerNameByKey={providerNameByKey}
          providerAccountByKey={providerAccountByKey}
          readOnly={effectiveReadOnly}
          hideRuntimeActions={shareScoped || effectiveReadOnly}
          onRetry={() => void refetch()}
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
        />
      </div>

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
