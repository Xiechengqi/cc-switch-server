import { useMemo } from "react";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import type { AppId } from "@/lib/api";
import {
  useClientTunnelQuery,
  useCreateShareMutation,
  useDisableShareMutation,
  useEnableShareMutation,
  useSharesQuery,
} from "@/lib/query";
import {
  isShareableApp,
  resolveShareOwnerEmail,
  useProviderShare,
} from "@/hooks/useProviderShare";
import { extractErrorMessage } from "@/utils/errorUtils";
import {
  permanentExpiresInSecs,
  UNLIMITED_PARALLEL_LIMIT,
  UNLIMITED_TOKEN_LIMIT,
} from "@/utils/shareUtils";

export function isShareRunning(
  share: { status: string; tunnelUrl?: string | null; subdomain?: string | null },
): boolean {
  if (share.status !== "active") return false;
  return Boolean(share.tunnelUrl?.trim() || share.subdomain?.trim());
}

export function useToggleProviderShare(
  appId: AppId,
  providerId: string | undefined,
) {
  const { t } = useTranslation();
  const providerShare = useProviderShare(appId, providerId);
  const { share, state } = providerShare;
  const { data: clientTunnel } = useClientTunnelQuery();
  const { data: shares = [] } = useSharesQuery();
  const createMutation = useCreateShareMutation();
  const enableMutation = useEnableShareMutation();
  const disableMutation = useDisableShareMutation();

  const shareable = isShareableApp(appId) && Boolean(providerId);
  const hasShare = Boolean(share);
  const isSharing = share ? isShareRunning(share) : false;

  const isPending =
    createMutation.isPending ||
    enableMutation.isPending ||
    disableMutation.isPending;

  const ownerEmail = useMemo(
    () => resolveShareOwnerEmail(clientTunnel?.config?.ownerEmail, shares),
    [clientTunnel?.config?.ownerEmail, shares],
  );

  const enableShare = async () => {
    if (!shareable || !providerId) return;
    try {
      if (share) {
        if (!isShareRunning(share)) {
          await enableMutation.mutateAsync(share.id);
        }
        return;
      }

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
      });
      await enableMutation.mutateAsync(created.id);
    } catch (error) {
      toast.error(
        t("share.toggle.enableFailed", {
          defaultValue: "开启分享失败：{{error}}",
          error: extractErrorMessage(error),
        }),
      );
      throw error;
    }
  };

  const disableShare = async () => {
    if (!share) return;
    try {
      if (isShareRunning(share)) {
        await disableMutation.mutateAsync(share.id);
      }
    } catch (error) {
      toast.error(
        t("share.toggle.disableFailed", {
          defaultValue: "关闭分享失败：{{error}}",
          error: extractErrorMessage(error),
        }),
      );
      throw error;
    }
  };

  const toggleShare = async () => {
    if (!shareable || isPending) return;
    if (isSharing) {
      await disableShare();
    } else {
      await enableShare();
    }
  };

  return {
    ...providerShare,
    shareable,
    hasShare,
    isSharing,
    isPending,
    enableShare,
    disableShare,
    toggleShare,
    state,
    share,
  };
}
