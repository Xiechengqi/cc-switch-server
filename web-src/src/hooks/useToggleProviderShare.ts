import { useMemo } from "react";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import type { AppId } from "@/lib/api";
import {
  useClientTunnelQuery,
  useCreateShareMutation,
  useDeleteShareMutation,
  useDisableShareMutation,
  useEnableShareMutation,
} from "@/lib/query";
import {
  isShareableApp,
  resolveShareOwnerEmail,
  useProviderShare,
} from "@/hooks/useProviderShare";
import { extractErrorMessage } from "@/utils/errorUtils";
import {
  getProviderSharePhase,
  isShareRunning,
  permanentExpiresInSecs,
  UNLIMITED_PARALLEL_LIMIT,
  UNLIMITED_TOKEN_LIMIT,
  type ProviderSharePhase,
} from "@/utils/shareUtils";

export { isShareRunning, type ProviderSharePhase };

export function useToggleProviderShare(
  appId: AppId,
  providerId: string | undefined,
) {
  const { t } = useTranslation();
  const providerShare = useProviderShare(appId, providerId);
  const { share, state } = providerShare;
  const { data: clientTunnel } = useClientTunnelQuery();
  const createMutation = useCreateShareMutation();
  const enableMutation = useEnableShareMutation();
  const disableMutation = useDisableShareMutation();
  const deleteMutation = useDeleteShareMutation();

  const shareable = isShareableApp(appId) && Boolean(providerId);
  const sharePhase = getProviderSharePhase(share);
  const hasShare = Boolean(share);
  const isSharing = sharePhase === "sharing";

  const isPending =
    createMutation.isPending ||
    enableMutation.isPending ||
    disableMutation.isPending ||
    deleteMutation.isPending;

  const ownerEmail = useMemo(
    () => resolveShareOwnerEmail(clientTunnel?.config?.ownerEmail),
    [clientTunnel?.config?.ownerEmail],
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

      await createMutation.mutateAsync({
        bindings: { [appId]: providerId },
        forSale: "Yes",
        saleMarketKind: "token",
        tokenLimit: UNLIMITED_TOKEN_LIMIT,
        parallelLimit: UNLIMITED_PARALLEL_LIMIT,
        expiresInSecs: permanentExpiresInSecs(),
        sharedWithEmails: [],
        marketAccessMode: "all",
        accessByApp: {
          [appId]: { sharedWithEmails: [], marketAccessMode: "all" },
        },
        appSettings: {
          [appId]: {
            forSale: "Yes",
            saleMarketKind: "token",
            marketAccessMode: "all",
            sharedWithEmails: [],
            tokenLimit: UNLIMITED_TOKEN_LIMIT,
            parallelLimit: UNLIMITED_PARALLEL_LIMIT,
            expiresAt: "2099-12-31T23:59:59Z",
          },
        },
      });
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

  const deleteShare = async () => {
    if (!share) return;
    try {
      await deleteMutation.mutateAsync(share.id);
    } catch (error) {
      toast.error(
        t("provider.share.deleteFailed", {
          defaultValue: "删除分享失败：{{error}}",
          error: extractErrorMessage(error),
        }),
      );
      throw error;
    }
  };

  const handleSharePrimaryAction = async () => {
    if (!shareable || isPending) return;
    if (sharePhase === "sharing") {
      await disableShare();
      return;
    }
    if (sharePhase === "not_created") {
      await enableShare();
    }
  };

  const handleShareResume = async () => {
    if (!shareable || isPending || sharePhase !== "stopped") return;
    await enableShare();
  };

  return {
    ...providerShare,
    shareable,
    sharePhase,
    hasShare,
    isSharing,
    isPending,
    enableShare,
    disableShare,
    deleteShare,
    handleSharePrimaryAction,
    handleShareResume,
    state,
    share,
  };
}
