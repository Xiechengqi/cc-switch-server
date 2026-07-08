import { useEffect, useMemo, useState } from "react";
import { Share2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { ShareRouterAdminPanel } from "@/components/settings/ShareRouterAdminPanel";
import { ServerSettingsExtensions } from "@/components/settings/ServerSettingsExtensions";
import { ShareEmailLoginCard } from "@/components/settings/ShareEmailLoginCard";
import { ClientTunnelSettingsPanel } from "@/components/settings/ClientTunnelSettingsPanel";
import { ShareRouterBar } from "@/components/share/ShareRouterBar";
import { ShareRouterSelector } from "@/components/share/ShareRouterSelector";
import { ShareOwnerChangeEmailDialog } from "@/components/share/ShareOwnerChangeEmailDialog";
import {
  useClientTunnelQuery,
  useConfigureTunnelMutation,
  useSettingsQuery,
  useSharesQuery,
} from "@/lib/query";
import { useProxyStatus } from "@/lib/query/proxy";
import {
  formatShareRouterDisplay,
  normalizeShareRouterDomain,
} from "@/utils/shareRouter";
import { getTunnelConfigFromSettings } from "@/utils/shareUtils";

export function ShareSettingsTab() {
  const { t } = useTranslation();
  const { data: settings } = useSettingsQuery();
  const { data: shares = [] } = useSharesQuery();
  const { data: proxyStatus } = useProxyStatus();
  const { data: clientTunnel } = useClientTunnelQuery();
  const configureTunnelMutation = useConfigureTunnelMutation();

  const tunnelConfig = useMemo(
    () => getTunnelConfigFromSettings(settings),
    [settings],
  );

  const [routerDomain, setRouterDomain] = useState(tunnelConfig.domain);
  const [routerDomainError, setRouterDomainError] = useState<string | null>(
    null,
  );
  const [ownerChangeOpen, setOwnerChangeOpen] = useState(false);

  useEffect(() => {
    setRouterDomain(tunnelConfig.domain);
  }, [tunnelConfig.domain]);

  const routerDisplay = formatShareRouterDisplay(tunnelConfig.domain);
  const routerDirty = routerDomain.trim() !== tunnelConfig.domain.trim();

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
    <div className="space-y-6">
      <ShareRouterAdminPanel />

      <section className="rounded-xl border border-border/60 bg-card/60 p-6 space-y-4">
        <div className="flex items-start gap-3">
          <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-muted">
            <Share2 className="h-5 w-5" />
          </div>
          <div className="min-w-0 flex-1 space-y-1">
            <h4 className="font-medium">
              {t("settings.share.defaultRouter.title", {
                defaultValue: "默认 Router 节点",
              })}
            </h4>
            <p className="text-sm text-muted-foreground">
              {t("settings.share.defaultRouter.description", {
                defaultValue:
                  "新建 share 时默认使用此路由节点。已创建的 share 仍绑定创建时的节点。",
              })}
            </p>
            <p className="text-xs text-muted-foreground">
              {t("settings.share.defaultRouter.current", {
                defaultValue: "当前：{{value}}",
                value: routerDisplay,
              })}
            </p>
          </div>
        </div>

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
      </section>

      <ShareRouterBar
        mode="settings"
        proxyRunning={proxyStatus?.running ?? false}
        proxyAddress={proxyStatus?.address ?? null}
        proxyPort={proxyStatus?.port ?? null}
        hasShare={shares.length > 0}
      />

      <ClientTunnelSettingsPanel
        onChangeOwnerEmail={() => setOwnerChangeOpen(true)}
      />

      <ShareEmailLoginCard />

      <ServerSettingsExtensions
        sections={["diagnostics", "importExport", "auth"]}
      />

      <ShareOwnerChangeEmailDialog
        open={ownerChangeOpen}
        tunnelConfig={tunnelConfig}
        tunnelConfigSaving={configureTunnelMutation.isPending}
        currentEmail={clientTunnel?.config?.ownerEmail ?? null}
        onOpenChange={setOwnerChangeOpen}
        onSaveTunnelConfig={(config) =>
          configureTunnelMutation.mutateAsync(config)
        }
      />
    </div>
  );
}
