import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  useClientTunnelQuery,
  useConfigureTunnelMutation,
  useSettingsQuery,
  useStartClientTunnelMutation,
  useStopClientTunnelMutation,
} from "@/lib/query";
import { copyText } from "@/lib/clipboard";
import { ShareOwnerChangeEmailDialog } from "@/components/share/ShareOwnerChangeEmailDialog";
import { getTunnelConfigFromSettings } from "@/utils/shareUtils";

export interface ClientTunnelFormState {
  dirty: boolean;
  canSave: boolean;
  isSaving: boolean;
  save: () => Promise<void>;
}

interface ClientTunnelSettingsPanelProps {
  embedded?: boolean;
  hideSaveButton?: boolean;
  onFormStateChange?: (state: ClientTunnelFormState | null) => void;
}

export function ClientTunnelSettingsPanel({
  embedded = false,
  onFormStateChange,
}: ClientTunnelSettingsPanelProps) {
  const { t } = useTranslation();
  const { data: settings } = useSettingsQuery();
  const { data: clientTunnel, refetch: refetchClientTunnel } =
    useClientTunnelQuery();
  const configureTunnelMutation = useConfigureTunnelMutation();
  const startMutation = useStartClientTunnelMutation();
  const stopMutation = useStopClientTunnelMutation();
  const [ownerChangeOpen, setOwnerChangeOpen] = useState(false);
  const tunnelConfig = useMemo(
    () => getTunnelConfigFromSettings(settings),
    [settings],
  );
  const currentOwnerEmail = clientTunnel?.config?.ownerEmail?.trim() ?? "";
  const isRunning = Boolean(clientTunnel?.status?.info);
  const isToggling = startMutation.isPending || stopMutation.isPending;

  useEffect(() => {
    onFormStateChange?.(null);
    return () => onFormStateChange?.(null);
  }, [onFormStateChange]);

  const handleToggleTunnel = useCallback(async () => {
    if (isRunning) {
      await stopMutation.mutateAsync();
      return;
    }
    await startMutation.mutateAsync();
  }, [isRunning, startMutation, stopMutation]);

  const statusLabel = isRunning
    ? t("settings.share.clientTunnel.running", { defaultValue: "运行中" })
    : clientTunnel?.status?.lastError
      ? t("settings.share.clientTunnel.failed", {
          defaultValue: "失败: {{error}}",
          error: clientTunnel.status.lastError,
        })
      : t("settings.share.clientTunnel.stopped", { defaultValue: "未运行" });

  const body = (
    <>
      <div className="grid gap-4 md:grid-cols-3">
        <div className="space-y-2">
          <div className="text-xs font-medium text-muted-foreground">
            {t("settings.share.clientTunnel.ownerEmail", {
              defaultValue: "Client Tunnel Owner",
            })}
          </div>
          <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
            <Input
              className="h-9"
              type="email"
              value={currentOwnerEmail}
              disabled
              readOnly
            />
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="shrink-0"
              disabled={!currentOwnerEmail}
              onClick={() => setOwnerChangeOpen(true)}
            >
              {t("share.ownerChange.action", {
                defaultValue: "Change Owner Email",
              })}
            </Button>
          </div>
        </div>
        <div className="space-y-2">
          <div className="text-xs font-medium text-muted-foreground">
            {t("settings.share.clientTunnel.subdomain", {
              defaultValue: "Client Subdomain",
            })}
          </div>
          <Input
            className="h-9"
            value={clientTunnel?.config?.subdomain?.trim() ?? ""}
            disabled
            readOnly
          />
        </div>
        <div className="space-y-2">
          <div className="text-xs font-medium text-muted-foreground">
            {t("settings.share.clientTunnel.url", {
              defaultValue: "Client URL",
            })}
          </div>
          <button
            type="button"
            className="block max-w-full truncate text-left text-sm underline-offset-4 hover:underline disabled:opacity-50"
            disabled={!clientTunnel?.config?.tunnelUrl}
            onClick={() => {
              if (!clientTunnel?.config?.tunnelUrl) return;
              void copyText(clientTunnel.config.tunnelUrl).then(() =>
                toast.success(t("common.copied", { defaultValue: "已复制" })),
              );
            }}
          >
            {clientTunnel?.config?.tunnelUrl ?? "-"}
          </button>
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <span className="text-xs text-muted-foreground">{statusLabel}</span>
        <Button
          variant="outline"
          size="sm"
          disabled={isToggling}
          onClick={() => void handleToggleTunnel()}
        >
          {isRunning
            ? t("settings.share.clientTunnel.stop", { defaultValue: "停止" })
            : t("settings.share.clientTunnel.start", { defaultValue: "启动" })}
        </Button>
      </div>
    </>
  );

  if (embedded) {
    return (
      <div className="space-y-4">
        {body}
        <ShareOwnerChangeEmailDialog
          open={ownerChangeOpen}
          tunnelConfig={tunnelConfig}
          tunnelConfigSaving={configureTunnelMutation.isPending}
          currentEmail={currentOwnerEmail || null}
          onOpenChange={setOwnerChangeOpen}
          onSaveTunnelConfig={async (config) => {
            await configureTunnelMutation.mutateAsync(config);
          }}
          onChanged={async () => {
            await refetchClientTunnel();
          }}
        />
      </div>
    );
  }

  return (
    <section className="rounded-xl border border-border/60 bg-card/60 p-6 space-y-4">
      <div>
        <h4 className="font-medium">
          {t("settings.share.clientTunnel.title", {
            defaultValue: "Client Tunnel",
          })}
        </h4>
        <p className="text-sm text-muted-foreground">
          {t("settings.share.clientTunnel.description", {
            defaultValue: "Client 子域名在初始设置后保持不变。",
          })}
        </p>
      </div>
      {body}
      <ShareOwnerChangeEmailDialog
        open={ownerChangeOpen}
        tunnelConfig={tunnelConfig}
        tunnelConfigSaving={configureTunnelMutation.isPending}
        currentEmail={currentOwnerEmail || null}
        onOpenChange={setOwnerChangeOpen}
        onSaveTunnelConfig={async (config) => {
          await configureTunnelMutation.mutateAsync(config);
        }}
        onChanged={async () => {
          await refetchClientTunnel();
        }}
      />
    </section>
  );
}
