import { useMemo, useState } from "react";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import {
  shareApi,
  type AppId,
  type ShareReuseCandidate,
} from "@/lib/api";
import {
  useAddShareBindingMutation,
  useClientTunnelQuery,
  useCreateShareMutation,
  useEnableShareMutation,
  useRemoveShareBindingMutation,
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
  const addBindingMutation = useAddShareBindingMutation();
  const removeBindingMutation = useRemoveShareBindingMutation();
  const enableMutation = useEnableShareMutation();
  const [reuseCandidates, setReuseCandidates] = useState<
    ShareReuseCandidate[] | null
  >(null);

  const shareable = isShareableApp(appId) && Boolean(providerId);
  const sharePhase = getProviderSharePhase(share);
  const hasShare = Boolean(share);
  const isSharing = sharePhase === "sharing";

  const isPending =
    createMutation.isPending ||
    addBindingMutation.isPending ||
    removeBindingMutation.isPending ||
    enableMutation.isPending ||
    reuseCandidates !== null;

  const ownerEmail = useMemo(
    () => resolveShareOwnerEmail(clientTunnel?.config?.ownerEmail),
    [clientTunnel?.config?.ownerEmail],
  );

  const createNewShare = async () => {
    if (!shareable || !providerId) return;
    await createMutation.mutateAsync({
        bindings: { [appId]: providerId },
        forSale: "Yes",
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
            marketAccessMode: "all",
            sharedWithEmails: [],
            tokenLimit: UNLIMITED_TOKEN_LIMIT,
            parallelLimit: UNLIMITED_PARALLEL_LIMIT,
            expiresAt: "2099-12-31T23:59:59Z",
          },
        },
    });
  };

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

      const candidates = await shareApi.listReuseCandidates(appId, providerId);
      if (candidates.length > 0) {
        setReuseCandidates(candidates);
        return;
      }
      await createNewShare();
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
    if (!share || !shareable || !providerId) return;
    try {
      await removeBindingMutation.mutateAsync({
        shareId: share.id,
        app: appId,
        providerId,
        expectedConfigRevision: share.configRevision,
      });
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
    if (!share || !shareable || !providerId) return;
    try {
      await removeBindingMutation.mutateAsync({
        shareId: share.id,
        app: appId,
        providerId,
        expectedConfigRevision: share.configRevision,
      });
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

  const confirmShareReuse = async (reuse: boolean, shareId: string) => {
    if (!shareable || !providerId || !reuseCandidates) return;
    const candidate = reuseCandidates.find((item) => item.shareId === shareId);
    setReuseCandidates(null);
    if (!reuse || !candidate) {
      await createNewShare();
      return;
    }
    await addBindingMutation.mutateAsync({
      shareId: candidate.shareId,
      app: appId,
      providerId,
      expectedConfigRevision: candidate.configRevision,
    });
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
    reuseCandidates,
    confirmShareReuse,
    dismissShareReuse: () => setReuseCandidates(null),
    handleSharePrimaryAction,
    handleShareResume,
    state,
    share,
  };
}
