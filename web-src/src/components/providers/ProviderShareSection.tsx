import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Copy, ExternalLink, Loader2, Pause, Play, Share2, Trash2 } from "lucide-react";
import type { AppId } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { Badge } from "@/components/ui/badge";
import { copyText } from "@/lib/clipboard";
import { toast } from "sonner";
import {
  useClientTunnelQuery,
  useSettingsQuery,
  useCreateShareMutation,
  useDeleteShareMutation,
  useDisableShareMutation,
  useEnableShareMutation,
  usePauseShareMutation,
  useResumeShareMutation,
  useUpdateShareDescriptionMutation,
  useUpdateShareParallelLimitMutation,
  useUpdateShareTokenLimitMutation,
} from "@/lib/query";
import {
  getProviderShareState,
  isShareableApp,
  resolveShareOwnerEmail,
  useProviderShare,
  type ProviderShareState,
} from "@/hooks/useProviderShare";
import {
  UNLIMITED_PARALLEL_LIMIT,
  UNLIMITED_TOKEN_LIMIT,
  permanentExpiresInSecs,
} from "@/utils/shareUtils";
import { formatShareRouterDisplay } from "@/utils/shareRouter";
import { getTunnelConfigFromSettings } from "@/utils/shareUtils";

interface ProviderShareSectionProps {
  appId: AppId;
  providerId: string;
  providerName: string;
}

function shareStateLabel(state: ProviderShareState, t: (key: string, options?: Record<string, unknown>) => string) {
  if (state === "active") {
    return t("provider.share.stateActive", { defaultValue: "分享已启用" });
  }
  if (state === "paused") {
    return t("provider.share.statePaused", { defaultValue: "分享已暂停" });
  }
  if (state === "error") {
    return t("provider.share.stateError", { defaultValue: "分享异常" });
  }
  return t("provider.share.stateNone", { defaultValue: "未启用分享" });
}

function shareStateVariant(state: ProviderShareState): "default" | "secondary" | "destructive" | "outline" {
  if (state === "active") return "default";
  if (state === "paused") return "secondary";
  if (state === "error") return "destructive";
  return "outline";
}

export function ProviderShareSection({
  appId,
  providerId,
  providerName,
}: ProviderShareSectionProps) {
  const { t } = useTranslation();
  const { share, state, data: shares = [] } = useProviderShare(appId, providerId);
  const { data: clientTunnel } = useClientTunnelQuery();
  const { data: settings } = useSettingsQuery();
  const createMutation = useCreateShareMutation();
  const deleteMutation = useDeleteShareMutation();
  const enableMutation = useEnableShareMutation();
  const disableMutation = useDisableShareMutation();
  const pauseMutation = usePauseShareMutation();
  const resumeMutation = useResumeShareMutation();
  const updateTokenLimitMutation = useUpdateShareTokenLimitMutation();
  const updateParallelLimitMutation = useUpdateShareParallelLimitMutation();
  const updateDescriptionMutation = useUpdateShareDescriptionMutation();

  const [tokenLimitInput, setTokenLimitInput] = useState("");
  const [parallelLimitInput, setParallelLimitInput] = useState("");
  const [descriptionInput, setDescriptionInput] = useState("");
  const [tokenTouched, setTokenTouched] = useState(false);
  const [parallelTouched, setParallelTouched] = useState(false);
  const [descriptionTouched, setDescriptionTouched] = useState(false);

  useEffect(() => {
    if (!share) {
      setTokenLimitInput("");
      setParallelLimitInput("");
      setDescriptionInput("");
      setTokenTouched(false);
      setParallelTouched(false);
      setDescriptionTouched(false);
      return;
    }
    setTokenLimitInput(
      share.tokenLimit === UNLIMITED_TOKEN_LIMIT ? "" : String(share.tokenLimit),
    );
    setParallelLimitInput(
      share.parallelLimit === UNLIMITED_PARALLEL_LIMIT
        ? ""
        : String(share.parallelLimit),
    );
    setDescriptionInput(share.description || "");
    setTokenTouched(false);
    setParallelTouched(false);
    setDescriptionTouched(false);
  }, [share?.id]);

  const ownerEmail = useMemo(
    () => resolveShareOwnerEmail(clientTunnel?.config?.ownerEmail, shares),
    [clientTunnel?.config?.ownerEmail, shares],
  );

  const routerConsoleUrl = useMemo(() => {
    const domain = getTunnelConfigFromSettings(settings).domain;
    if (!domain) return null;
    const host = domain.split(":")[0] ?? domain;
    const isLocal =
      host === "localhost" || host === "127.0.0.1" || host === "0.0.0.0";
    return `${isLocal ? "http" : "https"}://${domain}`;
  }, [settings]);

  const busy =
    createMutation.isPending ||
    deleteMutation.isPending ||
    enableMutation.isPending ||
    disableMutation.isPending ||
    pauseMutation.isPending ||
    resumeMutation.isPending ||
    updateTokenLimitMutation.isPending ||
    updateParallelLimitMutation.isPending ||
    updateDescriptionMutation.isPending;

  if (!isShareableApp(appId)) {
    return null;
  }

  const handleCreate = async () => {
    if (!ownerEmail) {
      toast.error(
        t("provider.share.ownerRequired", {
          defaultValue: "请先在分享页配置 Client Tunnel Owner 邮箱",
        }),
      );
      return;
    }
    const created = await createMutation.mutateAsync({
      ownerEmail,
      bindings: { [appId]: providerId },
      forSale: "No",
      tokenLimit: UNLIMITED_TOKEN_LIMIT,
      parallelLimit: UNLIMITED_PARALLEL_LIMIT,
      expiresInSecs: permanentExpiresInSecs(),
      description: descriptionInput.trim() || undefined,
    });
    await enableMutation.mutateAsync(created.id);
  };

  const handleSaveLimits = async () => {
    if (!share) return;
    const tokenLimit = tokenTouched
      ? tokenLimitInput.trim()
        ? Number(tokenLimitInput)
        : UNLIMITED_TOKEN_LIMIT
      : share.tokenLimit;
    const parallelLimit = parallelTouched
      ? parallelLimitInput.trim()
        ? Number(parallelLimitInput)
        : UNLIMITED_PARALLEL_LIMIT
      : share.parallelLimit;
    if (Number.isNaN(tokenLimit) || Number.isNaN(parallelLimit)) {
      toast.error(t("provider.share.invalidNumber", { defaultValue: "请输入有效数字" }));
      return;
    }
    if (tokenLimit !== share.tokenLimit) {
      await updateTokenLimitMutation.mutateAsync({ shareId: share.id, tokenLimit });
    }
    if (parallelLimit !== share.parallelLimit) {
      await updateParallelLimitMutation.mutateAsync({
        shareId: share.id,
        parallelLimit,
      });
    }
    if (
      descriptionTouched &&
      (share.description || "") !== descriptionInput.trim()
    ) {
      await updateDescriptionMutation.mutateAsync({
        shareId: share.id,
        description: descriptionInput.trim(),
      });
    }
  };

  const tunnelLabel = share?.tunnelUrl || share?.subdomain
    ? formatShareRouterDisplay(share.tunnelUrl || share.subdomain || "")
    : null;

  return (
    <section className="rounded-xl border border-border-default/80 bg-muted/10 p-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="grid gap-1">
          <div className="flex items-center gap-2">
            <Share2 className="h-4 w-4 text-primary" />
            <h3 className="text-sm font-semibold">
              {t("provider.share.sectionTitle", { defaultValue: "远程分享" })}
            </h3>
            <Badge variant={shareStateVariant(state)}>
              {shareStateLabel(state, t)}
            </Badge>
          </div>
          <p className="text-xs text-muted-foreground">
            {t("provider.share.sectionHint", {
              defaultValue:
                "每个 Provider 对应一个 Share。限额与描述在此保存；售卖、ACL、过期等运营配置请在 Router Console 管理。",
            })}
          </p>
          {routerConsoleUrl ? (
            <a
              href={routerConsoleUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex w-fit items-center gap-1 text-xs font-medium text-primary hover:underline"
            >
              {t("provider.share.openRouterConsole", {
                defaultValue: "打开 Router Console",
              })}
              <ExternalLink className="h-3 w-3" />
            </a>
          ) : null}
        </div>
        {busy ? <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" /> : null}
      </div>

      <div className="mt-4 grid gap-4">
        <div className="grid gap-2">
          <Label htmlFor="provider-share-description">
            {t("share.description", { defaultValue: "描述" })}
          </Label>
          <Textarea
            id="provider-share-description"
            rows={2}
            value={descriptionInput}
            placeholder={providerName}
            disabled={busy}
            onChange={(event) => {
              setDescriptionTouched(true);
              setDescriptionInput(event.target.value);
            }}
          />
        </div>

        <div className="grid gap-4 md:grid-cols-2">
          <div className="grid gap-2">
            <Label htmlFor="provider-share-token-limit">
              {t("share.tokenLimit", { defaultValue: "Token 限额" })}
            </Label>
            <Input
              id="provider-share-token-limit"
              type="number"
              min={0}
              placeholder={t("share.unlimited", { defaultValue: "不限" })}
              value={tokenLimitInput}
              disabled={busy}
              onChange={(event) => {
                setTokenTouched(true);
                setTokenLimitInput(event.target.value);
              }}
            />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="provider-share-parallel-limit">
              {t("share.parallelLimit", { defaultValue: "并发限额" })}
            </Label>
            <Input
              id="provider-share-parallel-limit"
              type="number"
              min={0}
              placeholder={t("share.unlimited", { defaultValue: "不限" })}
              value={parallelLimitInput}
              disabled={busy}
              onChange={(event) => {
                setParallelTouched(true);
                setParallelLimitInput(event.target.value);
              }}
            />
          </div>
        </div>

        {tunnelLabel ? (
          <div className="flex flex-wrap items-center gap-2 rounded-lg border bg-background px-3 py-2 text-sm">
            <span className="font-mono text-xs">{tunnelLabel}</span>
            <Button
              type="button"
              size="sm"
              variant="ghost"
              onClick={() => void copyText(tunnelLabel)}
            >
              <Copy className="h-3.5 w-3.5" />
            </Button>
          </div>
        ) : null}

        <div className="flex flex-wrap gap-2">
          {!share ? (
            <Button type="button" disabled={busy} onClick={() => void handleCreate()}>
              <Play className="mr-2 h-4 w-4" />
              {t("provider.share.createAndEnable", { defaultValue: "创建并启用分享" })}
            </Button>
          ) : (
            <>
              <Button
                type="button"
                variant="outline"
                disabled={busy}
                onClick={() => void handleSaveLimits()}
              >
                {t("common.save", { defaultValue: "保存" })}
              </Button>
              {share.status === "active" ? (
                <Button
                  type="button"
                  variant="outline"
                  disabled={busy}
                  onClick={() => void pauseMutation.mutateAsync(share.id)}
                >
                  <Pause className="mr-2 h-4 w-4" />
                  {t("share.pause", { defaultValue: "暂停" })}
                </Button>
              ) : (
                <Button
                  type="button"
                  variant="outline"
                  disabled={busy}
                  onClick={() => void resumeMutation.mutateAsync(share.id)}
                >
                  <Play className="mr-2 h-4 w-4" />
                  {t("share.resume", { defaultValue: "恢复" })}
                </Button>
              )}
              {share.tunnelUrl ? (
                <Button
                  type="button"
                  variant="outline"
                  disabled={busy}
                  onClick={() => void disableMutation.mutateAsync(share.id)}
                >
                  {t("share.disable", { defaultValue: "关闭隧道" })}
                </Button>
              ) : (
                <Button
                  type="button"
                  variant="outline"
                  disabled={busy}
                  onClick={() => void enableMutation.mutateAsync(share.id)}
                >
                  {t("share.enable", { defaultValue: "开启隧道" })}
                </Button>
              )}
              <Button
                type="button"
                variant="ghost"
                className="text-destructive hover:text-destructive"
                disabled={busy}
                onClick={() => void deleteMutation.mutateAsync(share.id)}
              >
                <Trash2 className="mr-2 h-4 w-4" />
                {t("share.delete", { defaultValue: "删除分享" })}
              </Button>
            </>
          )}
        </div>
      </div>
    </section>
  );
}

export { getProviderShareState };
