import { useCallback, useEffect, useState } from "react";
import {
  CheckCircle2,
  Loader2,
  Network,
  RefreshCw,
  RotateCcw,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Input } from "@/components/ui/input";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { RouterFacts } from "@/components/settings/SettingsStatusPanels";
import {
  errorMessage,
  routerState,
  routerStatusText,
} from "@/components/settings/settingsDrafts";
import {
  batchSyncRouterShares,
  heartbeatRouter,
  registerRouter,
  updateRouterConfig,
  type RouterConfigView,
  type RouterStatusResponse,
} from "@/lib/server-legacy-api";
import { jsonFetch } from "@/lib/runtime";

interface RouterAdvancedDraft {
  apiBase: string;
  sshHost: string;
  sshUser: string;
  custom: boolean;
}

function advancedDraftFrom(router: RouterConfigView): RouterAdvancedDraft {
  return {
    apiBase: router.apiBase || "",
    sshHost: router.sshHost || "",
    sshUser: router.sshUser || "",
    custom: router.custom,
  };
}

export function ShareRouterAdminPanel() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [router, setRouter] = useState<RouterConfigView | null>(null);
  const [status, setStatus] = useState<RouterStatusResponse | null>(null);
  const [advanced, setAdvanced] = useState<RouterAdvancedDraft>({
    apiBase: "",
    sshHost: "",
    sshUser: "",
    custom: false,
  });
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [syncConfirmOpen, setSyncConfirmOpen] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const [routerRes, statusRes] = await Promise.all([
        jsonFetch<{ router: RouterConfigView }>("/api/router/config"),
        jsonFetch<RouterStatusResponse>("/api/router/status"),
      ]);
      setRouter(routerRes.router);
      setStatus(statusRes);
      setAdvanced(advancedDraftFrom(routerRes.router));
      await queryClient.invalidateQueries({ queryKey: ["settings"] });
    } catch (reason) {
      toast.error(errorMessage(reason));
    } finally {
      setLoading(false);
    }
  }, [queryClient]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const runAction = useCallback(
    async (action: string, task: () => Promise<string>) => {
      setBusy(action);
      try {
        const message = await task();
        toast.success(message);
        await refresh();
      } catch (reason) {
        toast.error(errorMessage(reason));
      } finally {
        setBusy(null);
      }
    },
    [refresh],
  );

  const saveAdvanced = useCallback(async () => {
    if (!router) return;
    await runAction("router-advanced-save", async () => {
      await updateRouterConfig({
        url: router.url || undefined,
        domain: router.domain || undefined,
        region: router.region || undefined,
        apiBase: advanced.apiBase,
        sshHost: advanced.sshHost,
        sshUser: advanced.sshUser,
        custom: advanced.custom,
      });
      return t("settings.share.advanced.saved", {
        defaultValue: "高级 Router 配置已保存",
      });
    });
  }, [advanced, router, runAction, t]);

  const statusLabel = routerState(status ?? undefined);
  const registered = status?.registered ?? false;

  return (
    <>
      <section className="rounded-xl border border-border/60 bg-card/60 p-6 space-y-4">
        <div className="flex items-start gap-3">
          <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-muted">
            <Network className="h-5 w-5" />
          </div>
          <div className="min-w-0 flex-1 space-y-1">
            <h4 className="font-medium">
              {t("settings.share.routerConnection.title", {
                defaultValue: "Router 连接",
              })}
            </h4>
            <p className="text-sm text-muted-foreground">
              {t("settings.share.routerConnection.description", {
                defaultValue:
                  "注册 installation 并将 share 元数据同步到远程 Router。",
              })}
            </p>
            <p className="text-xs text-muted-foreground">
              {loading
                ? t("common.loading", { defaultValue: "加载中..." })
                : statusLabel}
            </p>
          </div>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={loading || busy !== null}
            onClick={() => void refresh()}
          >
            <RefreshCw className="h-4 w-4" />
          </Button>
        </div>

        <div className="flex flex-wrap gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={busy !== null}
            onClick={() =>
              void runAction("router-register", async () => {
                await registerRouter();
                return t("settings.share.routerConnection.registered", {
                  defaultValue: "Router installation 已注册",
                });
              })
            }
          >
            {busy === "router-register" ? (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            ) : (
              <CheckCircle2 className="mr-2 h-4 w-4" />
            )}
            {t("server.settings.register", { defaultValue: "Register" })}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={busy !== null || !registered}
            onClick={() =>
              void runAction("router-heartbeat", async () =>
                routerStatusText(await heartbeatRouter()),
              )
            }
          >
            {busy === "router-heartbeat" ? (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            ) : (
              <RefreshCw className="mr-2 h-4 w-4" />
            )}
            {t("server.settings.heartbeat", { defaultValue: "Heartbeat" })}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={busy !== null || !registered}
            onClick={() => setSyncConfirmOpen(true)}
          >
            {busy === "router-sync" ? (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            ) : (
              <RotateCcw className="mr-2 h-4 w-4" />
            )}
            {t("server.settings.batchSync", { defaultValue: "Batch Sync" })}
          </Button>
        </div>

        {router ? (
          <div className="rounded-lg border border-border/50 bg-muted/20 p-3">
            <RouterFacts router={router} status={status ?? undefined} />
          </div>
        ) : null}

        <Accordion type="single" collapsible className="w-full">
          <AccordionItem value="advanced" className="border-none">
            <AccordionTrigger className="py-2 text-sm font-medium hover:no-underline">
              {t("settings.share.advanced.title", {
                defaultValue: "高级 Router 配置",
              })}
            </AccordionTrigger>
            <AccordionContent className="space-y-4 pt-2">
              <p className="text-xs text-muted-foreground">
                {t("settings.share.advanced.hint", {
                  defaultValue:
                    "节点域名请在下方「默认 Router 节点」中选择；此处仅配置 API Base、SSH 等高级项。",
                })}
              </p>
              <div className="grid gap-4 md:grid-cols-2">
                <div className="space-y-2 md:col-span-2">
                  <Label htmlFor="share-router-api-base">
                    {t("server.settings.apiBase", { defaultValue: "API Base" })}
                  </Label>
                  <Input
                    id="share-router-api-base"
                    value={advanced.apiBase}
                    placeholder="https://sgptokenswitch.cc"
                    disabled={busy !== null}
                    onChange={(event) =>
                      setAdvanced((current) => ({
                        ...current,
                        apiBase: event.target.value,
                      }))
                    }
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="share-router-ssh-host">
                    {t("server.settings.sshHost", { defaultValue: "SSH Host" })}
                  </Label>
                  <Input
                    id="share-router-ssh-host"
                    value={advanced.sshHost}
                    disabled={busy !== null}
                    onChange={(event) =>
                      setAdvanced((current) => ({
                        ...current,
                        sshHost: event.target.value,
                      }))
                    }
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="share-router-ssh-user">
                    {t("server.settings.sshUser", { defaultValue: "SSH User" })}
                  </Label>
                  <Input
                    id="share-router-ssh-user"
                    value={advanced.sshUser}
                    disabled={busy !== null}
                    onChange={(event) =>
                      setAdvanced((current) => ({
                        ...current,
                        sshUser: event.target.value,
                      }))
                    }
                  />
                </div>
              </div>
              <label className="flex items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={advanced.custom}
                  disabled={busy !== null}
                  onChange={(event) =>
                    setAdvanced((current) => ({
                      ...current,
                      custom: event.target.checked,
                    }))
                  }
                />
                <span>
                  {t("server.settings.customRouter", {
                    defaultValue: "自定义 router",
                  })}
                </span>
              </label>
              <div className="flex justify-end">
                <Button
                  type="button"
                  size="sm"
                  disabled={busy !== null}
                  onClick={() => void saveAdvanced()}
                >
                  {busy === "router-advanced-save" ? (
                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  ) : null}
                  {t("settings.share.advanced.save", {
                    defaultValue: "保存高级配置",
                  })}
                </Button>
              </div>
            </AccordionContent>
          </AccordionItem>
        </Accordion>
      </section>

      <ConfirmDialog
        isOpen={syncConfirmOpen}
        title={t("settings.share.batchSyncConfirm.title", {
          defaultValue: "批量同步 share 到 Router？",
        })}
        message={t("settings.share.batchSyncConfirm.message", {
          defaultValue:
            "将把本地 share 元数据同步到远程 Router，匹配的 share 记录可能被更新。",
        })}
        confirmText={t("settings.share.batchSyncConfirm.confirm", {
          defaultValue: "同步",
        })}
        onConfirm={() => {
          setSyncConfirmOpen(false);
          void runAction("router-sync", async () =>
            (await batchSyncRouterShares()).message,
          );
        }}
        onCancel={() => setSyncConfirmOpen(false)}
      />
    </>
  );
}
