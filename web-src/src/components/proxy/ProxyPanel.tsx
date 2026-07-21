import { Activity, Clock, Copy, Power, Server, TrendingUp } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { useProxyStatus } from "@/hooks/useProxyStatus";
import { useProxyTakeoverStatus } from "@/lib/query/proxy";

export function ProxyPanel() {
  const { t } = useTranslation();
  const { status, isRunning } = useProxyStatus();
  const { data: routingStatus } = useProxyTakeoverStatus();

  const serviceAddress = status
    ? formatAddressForUrl(status.address, status.port)
    : null;

  return (
    <section className="space-y-4">
      <div className="flex items-center justify-between rounded-lg border border-border bg-card/50 p-4">
        <div className="flex items-center gap-3">
          <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-background ring-1 ring-border">
            <Power className="h-4 w-4 text-green-500" />
          </div>
          <div className="space-y-1">
            <p className="text-sm font-medium leading-none">
              {t("proxyConfig.proxyEnabled", { defaultValue: "代理服务" })}
            </p>
            <p className="text-xs text-muted-foreground">
              {isRunning
                ? t("settings.advanced.proxy.running")
                : t("settings.advanced.proxy.stopped")}
            </p>
          </div>
        </div>
        <span className="rounded-full border border-emerald-500/30 bg-emerald-500/10 px-2.5 py-1 text-xs font-medium text-emerald-700 dark:text-emerald-300">
          {t("proxy.alwaysOn", { defaultValue: "始终开启" })}
        </span>
      </div>

      <div className="rounded-lg border border-primary/20 bg-primary/5 p-4">
        <div className="grid gap-2 sm:grid-cols-3">
          {(["claude", "codex", "gemini"] as const).map((appType) => {
            const ready = routingStatus?.[appType] ?? false;
            return (
              <div
                key={appType}
                className="flex items-center justify-between rounded-md border border-primary/20 bg-background/60 px-3 py-2"
              >
                <span className="text-sm font-medium capitalize">{appType}</span>
                <span
                  className={`rounded-full px-2 py-0.5 text-xs font-medium ${
                    ready
                      ? "bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
                      : "bg-amber-500/10 text-amber-700 dark:text-amber-300"
                  }`}
                >
                  {ready
                    ? t("proxy.routing.ready", { defaultValue: "已路由" })
                    : t("proxy.routing.needsProvider", {
                        defaultValue: "待配置",
                      })}
                </span>
              </div>
            );
          })}
        </div>
      </div>

      {isRunning && status ? (
        <>
          <div className="space-y-4 rounded-lg border border-border bg-muted/40 p-4">
            <div>
              <p className="mb-2 text-xs text-muted-foreground">
                {t("proxy.panel.serviceAddress", { defaultValue: "服务地址" })}
              </p>
              <div className="flex items-center gap-2">
                <code className="min-w-0 flex-1 truncate rounded border border-border/60 bg-background px-3 py-2 text-sm">
                  {serviceAddress}
                </code>
                <Button
                  type="button"
                  size="icon"
                  variant="outline"
                  title={t("common.copy")}
                  aria-label={t("common.copy")}
                  onClick={() => {
                    if (!serviceAddress) return;
                    void navigator.clipboard.writeText(serviceAddress);
                    toast.success(t("common.copied", { defaultValue: "已复制" }), {
                      closeButton: true,
                    });
                  }}
                >
                  <Copy className="h-4 w-4" />
                </Button>
              </div>
            </div>

            <div className="space-y-2 border-t border-border pt-3">
              <p className="text-xs text-muted-foreground">
                {t("provider.inUse")}
              </p>
              {status.active_targets?.length ? (
                <div className="grid gap-2 sm:grid-cols-2">
                  {status.active_targets.map((target) => (
                    <div
                      key={target.app_type}
                      className="flex min-w-0 items-center justify-between rounded-md border border-border bg-background/60 px-2 py-1.5 text-xs"
                    >
                      <span className="text-muted-foreground">
                        {target.app_type}
                      </span>
                      <span
                        className="ml-2 truncate font-medium text-foreground"
                        title={target.provider_name}
                      >
                        {target.provider_name}
                      </span>
                    </div>
                  ))}
                </div>
              ) : (
                <p className="text-sm text-amber-600 dark:text-amber-400">
                  {t("proxy.routing.needsProvider", {
                    defaultValue: "待配置",
                  })}
                </p>
              )}
            </div>
          </div>

          <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
            <StatCard
              icon={<Activity className="h-4 w-4" />}
              label={t("proxy.panel.stats.activeConnections", {
                defaultValue: "活跃连接",
              })}
              value={status.active_connections ?? 0}
            />
            <StatCard
              icon={<TrendingUp className="h-4 w-4" />}
              label={t("proxy.panel.stats.totalRequests", {
                defaultValue: "总请求数",
              })}
              value={status.total_requests ?? 0}
            />
            <StatCard
              icon={<Clock className="h-4 w-4" />}
              label={t("proxy.panel.stats.successRate", {
                defaultValue: "成功率",
              })}
              value={`${(status.success_rate ?? 0).toFixed(1)}%`}
            />
            <StatCard
              icon={<Clock className="h-4 w-4" />}
              label={t("proxy.panel.stats.uptime", {
                defaultValue: "运行时间",
              })}
              value={formatUptime(status.uptime_seconds ?? 0)}
            />
          </div>
        </>
      ) : (
        <div className="flex items-center gap-3 rounded-lg border border-border bg-muted/40 p-4 text-sm text-muted-foreground">
          <Server className="h-5 w-5" />
          {t("settings.advanced.proxy.stopped")}
        </div>
      )}
    </section>
  );
}

function formatAddressForUrl(address: string, port: number): string {
  const host = address.includes(":") ? `[${address}]` : address;
  return `http://${host}:${port}`;
}

function formatUptime(seconds: number): string {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const remainder = seconds % 60;
  if (hours > 0) return `${hours}h ${minutes}m ${remainder}s`;
  if (minutes > 0) return `${minutes}m ${remainder}s`;
  return `${remainder}s`;
}

function StatCard({
  icon,
  label,
  value,
}: {
  icon: React.ReactNode;
  label: string;
  value: string | number;
}) {
  return (
    <div className="rounded-lg border border-border bg-card/60 p-4 text-sm text-muted-foreground">
      <div className="mb-2 flex items-center gap-2 text-muted-foreground">
        {icon}
        <span className="text-xs">{label}</span>
      </div>
      <p className="text-xl font-semibold text-foreground">{value}</p>
    </div>
  );
}
