import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  useClaimClientTunnelMutation,
  useClientTunnelQuery,
  useStartClientTunnelMutation,
  useStopClientTunnelMutation,
} from "@/lib/query";
import { copyText } from "@/lib/clipboard";

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
  hideSaveButton = false,
  onFormStateChange,
}: ClientTunnelSettingsPanelProps) {
  const { t } = useTranslation();
  const { data: clientTunnel, isLoading } = useClientTunnelQuery();
  const claimMutation = useClaimClientTunnelMutation();
  const startMutation = useStartClientTunnelMutation();
  const stopMutation = useStopClientTunnelMutation();

  const [subdomainInput, setSubdomainInput] = useState("");

  useEffect(() => {
    if (clientTunnel?.config?.subdomain) {
      setSubdomainInput(clientTunnel.config.subdomain);
    }
  }, [clientTunnel?.config?.subdomain]);

  const ownerEmail = clientTunnel?.config?.ownerEmail?.trim() ?? "";
  const isSaving = claimMutation.isPending;
  const isRunning = Boolean(clientTunnel?.status?.info);
  const isToggling = startMutation.isPending || stopMutation.isPending;
  const dirty =
    subdomainInput.trim() !== (clientTunnel?.config?.subdomain?.trim() ?? "");
  const canSave = Boolean(subdomainInput.trim() && ownerEmail);

  const handleSave = useCallback(async () => {
    if (!subdomainInput.trim() || !ownerEmail) return;
    await claimMutation.mutateAsync({
      subdomain: subdomainInput.trim(),
      enabled: true,
      autoStart: true,
    });
  }, [claimMutation, ownerEmail, subdomainInput]);

  const handleToggleTunnel = useCallback(async () => {
    if (isRunning) {
      await stopMutation.mutateAsync();
      return;
    }
    await startMutation.mutateAsync();
  }, [isRunning, startMutation, stopMutation]);

  useEffect(() => {
    if (!onFormStateChange) return;
    onFormStateChange({
      dirty,
      canSave,
      isSaving,
      save: handleSave,
    });
    return () => onFormStateChange(null);
  }, [canSave, dirty, handleSave, isSaving, onFormStateChange]);

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
          <Input
            className="h-9"
            type="email"
            value={ownerEmail}
            placeholder="owner@example.com"
            disabled
            readOnly
          />
        </div>
        <div className="space-y-2">
          <div className="text-xs font-medium text-muted-foreground">
            {t("settings.share.clientTunnel.subdomain", {
              defaultValue: "Client Subdomain",
            })}
          </div>
          <Input
            className="h-9"
            value={subdomainInput}
            disabled={isLoading || isSaving}
            onChange={(event) => setSubdomainInput(event.target.value)}
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
                toast.success(
                  t("common.copied", { defaultValue: "已复制" }),
                ),
              );
            }}
          >
            {clientTunnel?.config?.tunnelUrl ?? "-"}
          </button>
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <span className="text-xs text-muted-foreground">{statusLabel}</span>
        {!hideSaveButton ? (
          <Button
            variant="outline"
            size="sm"
            disabled={!canSave || isSaving}
            onClick={() => void handleSave()}
          >
            {t("common.save", { defaultValue: "保存" })}
          </Button>
        ) : null}
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
    return <div className="space-y-4">{body}</div>;
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
            defaultValue:
              "配置本机 Client Tunnel 的 Owner 邮箱、子域名与启停状态。",
          })}
        </p>
      </div>
      {body}
    </section>
  );
}
