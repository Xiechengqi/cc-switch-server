import { useEffect, useMemo, useState, type ReactNode } from "react";
import { Activity, Laptop, Network, WalletCards } from "lucide-react";
import { motion } from "framer-motion";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { ClientTunnelSettingsPanel } from "@/components/settings/ClientTunnelSettingsPanel";
import { PayoutProfileSettingsPanel } from "@/components/settings/PayoutProfileSettingsPanel";
import {
  formatShareHealthOverview,
  ShareHealthStatusPanel,
} from "@/components/settings/ShareHealthStatusPanel";
import { ShareRouterSelector } from "@/components/share/ShareRouterSelector";
import { useConfigureTunnelMutation, useClientTunnelQuery, useSettingsQuery, useShareHealthQuery } from "@/lib/query";
import {
  formatShareRouterDisplay,
  normalizeShareRouterDomain,
} from "@/utils/shareRouter";
import { getTunnelConfigFromSettings } from "@/utils/shareUtils";

interface ShareSettingsAccordionItemProps {
  value: string;
  icon: ReactNode;
  title: string;
  description: string;
  children: ReactNode;
}

function ShareSettingsAccordionItem({
  value,
  icon,
  title,
  description,
  children,
}: ShareSettingsAccordionItemProps) {
  return (
    <AccordionItem
      value={value}
      className="rounded-xl glass-card overflow-hidden"
    >
      <AccordionTrigger className="px-6 py-4 hover:no-underline hover:bg-muted/50 data-[state=open]:bg-muted/50">
        <div className="flex items-center gap-3">
          <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-background ring-1 ring-border">
            {icon}
          </div>
          <div className="text-left">
            <h3 className="text-base font-semibold">{title}</h3>
            <p className="text-sm font-normal text-muted-foreground">
              {description}
            </p>
          </div>
        </div>
      </AccordionTrigger>
      <AccordionContent className="border-t border-border/50 px-6 pb-6 pt-4">
        {children}
      </AccordionContent>
    </AccordionItem>
  );
}

export function ShareSettingsTab() {
  const { t } = useTranslation();
  const { data: settings } = useSettingsQuery();
  const { data: clientTunnel } = useClientTunnelQuery();
  const { data: health } = useShareHealthQuery();
  const configureTunnelMutation = useConfigureTunnelMutation();

  const tunnelConfig = useMemo(
    () => getTunnelConfigFromSettings(settings),
    [settings],
  );

  const [routerDomain, setRouterDomain] = useState(tunnelConfig.domain);
  const [routerDomainError, setRouterDomainError] = useState<string | null>(
    null,
  );

  useEffect(() => {
    setRouterDomain(tunnelConfig.domain);
  }, [tunnelConfig.domain]);

  const routerDisplay = formatShareRouterDisplay(tunnelConfig.domain);
  const routerDirty = routerDomain.trim() !== tunnelConfig.domain.trim();
  const healthOverview = useMemo(
    () => formatShareHealthOverview(health, t),
    [health, t],
  );

  const clientStatusLabel = clientTunnel?.status?.info
    ? t("settings.share.clientTunnel.running", { defaultValue: "运行中" })
    : clientTunnel?.status?.lastError
      ? t("settings.share.clientTunnel.failed", {
          defaultValue: "失败: {{error}}",
          error: clientTunnel.status.lastError,
        })
      : t("settings.share.clientTunnel.stopped", { defaultValue: "未运行" });

  const handleSaveRouter = async () => {
    try {
      const normalized = normalizeShareRouterDomain(routerDomain);
      setRouterDomainError(null);
      await configureTunnelMutation.mutateAsync({ domain: normalized });
      setRouterDomain(normalized);
    } catch (error) {
      const key =
        error instanceof Error
          ? error.message
          : "share.validation.invalidRouterDomain";
      setRouterDomainError(
        t(key, { defaultValue: "Router 域名无效" }),
      );
    }
  };

  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3 }}
      className="space-y-4"
    >
      <Accordion type="multiple" defaultValue={[]} className="w-full space-y-4">
        <ShareSettingsAccordionItem
          value="client"
          icon={<Laptop className="h-5 w-5 text-sky-500" />}
          title={t("settings.share.sections.client.title", {
            defaultValue: "Client",
          })}
          description={t("settings.share.sections.client.description", {
            defaultValue:
              "配置 Client Tunnel 的 Owner 邮箱、子域名与启停状态。当前：{{status}}",
            status: clientStatusLabel,
          })}
        >
          <ClientTunnelSettingsPanel embedded />
        </ShareSettingsAccordionItem>

        <ShareSettingsAccordionItem
          value="payout"
          icon={<WalletCards className="h-5 w-5 text-amber-500" />}
          title={t("settings.share.sections.payout.title", {
            defaultValue: "收款信息",
          })}
          description={t("settings.share.sections.payout.description", {
            defaultValue: "配置公开的 EVM 收款地址、Token 与支持网络。",
          })}
        >
          <PayoutProfileSettingsPanel />
        </ShareSettingsAccordionItem>

        <ShareSettingsAccordionItem
          value="router"
          icon={<Network className="h-5 w-5 text-emerald-500" />}
          title={t("settings.share.sections.router.title", {
            defaultValue: "Router",
          })}
          description={t("settings.share.sections.router.description", {
            defaultValue:
              "配置新建 share 时使用的默认路由节点。当前：{{value}}",
            value: routerDisplay,
          })}
        >
          <div className="space-y-4">
            <p className="text-sm text-muted-foreground">
              {t("settings.share.defaultRouter.description", {
                defaultValue:
                  "新建 share 时默认使用此路由节点。已创建的 share 仍绑定创建时的节点。",
              })}
            </p>

            <div className="space-y-2">
              <Label htmlFor="settings-share-router">
                {t("share.tunnel.region", { defaultValue: "路由节点" })}
              </Label>
              <ShareRouterSelector
                value={routerDomain}
                onChange={(value) => {
                  setRouterDomain(value);
                  setRouterDomainError(null);
                }}
                selectId="settings-share-router"
                customInputId="settings-share-router-custom"
                disabled={configureTunnelMutation.isPending}
                error={routerDomainError}
              />
            </div>

            <div className="flex justify-end">
              <Button
                type="button"
                disabled={!routerDirty || configureTunnelMutation.isPending}
                onClick={() => void handleSaveRouter()}
              >
                {t("settings.share.defaultRouter.save", {
                  defaultValue: "保存默认节点",
                })}
              </Button>
            </div>
          </div>
        </ShareSettingsAccordionItem>

        <ShareSettingsAccordionItem
          value="health"
          icon={<Activity className="h-5 w-5 text-violet-500" />}
          title={t("settings.share.sections.health.title", {
            defaultValue: "健康状态",
          })}
          description={healthOverview}
        >
          <ShareHealthStatusPanel />
        </ShareSettingsAccordionItem>
      </Accordion>
    </motion.div>
  );
}
