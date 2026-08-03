import { useCallback } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { providersApi } from "@/lib/api/providers";
import type {
  ProviderCredentialPatches,
  ProviderCustomBinding,
} from "@/lib/api/providers";
import type { ProvidersQueryData } from "@/lib/query/queries";
import { providerHealthKeys } from "@/lib/query/providerHealth";
import type { CoreProviderApp } from "@/server/providerRegistry";
import type { Provider } from "@/types";
import { extractErrorMessage } from "@/utils/errorUtils";
import { generateUUID } from "@/utils/uuid";

export interface ServerProviderCreateInput extends Omit<Provider, "id"> {
  profileId?: string;
  customBinding?: ProviderCustomBinding;
  credentialPatches?: ProviderCredentialPatches;
}

interface ServerProviderUpdateOptions {
  profileId?: string;
  customBinding?: ProviderCustomBinding;
  credentialPatches?: ProviderCredentialPatches;
}

export function useServerProviderActions(activeApp: CoreProviderApp) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const invalidate = useCallback(
    () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: ["providers", activeApp] }),
        queryClient.invalidateQueries({
          queryKey: providerHealthKeys.app(activeApp),
        }),
      ]),
    [activeApp, queryClient],
  );

  const addProvider = useCallback(
    async (input: ServerProviderCreateInput) => {
      const { profileId, customBinding, credentialPatches, ...provider } =
        input;
      try {
        await providersApi.add(
          {
            ...provider,
            id: generateUUID(),
            createdAt: Date.now(),
          },
          activeApp,
          undefined,
          {
            profileId,
            customBinding,
            credentialPatches,
            clientRequestId: crypto.randomUUID(),
          },
        );
        await invalidate();
        toast.success(
          t("notifications.providerAdded", {
            defaultValue: "供应商已添加",
          }),
          { closeButton: true },
        );
      } catch (error) {
        const detail = extractErrorMessage(error) || t("common.unknown");
        toast.error(
          t("notifications.addFailed", {
            defaultValue: "添加供应商失败: {{error}}",
            error: detail,
          }),
        );
        throw error;
      }
    },
    [activeApp, invalidate, t],
  );

  const updateProvider = useCallback(
    async (
      provider: Provider,
      originalId?: string,
      options: ServerProviderUpdateOptions = {},
    ) => {
      const providerId = originalId ?? provider.id;
      const baseline = queryClient.getQueryData<ProvidersQueryData>([
        "providers",
        activeApp,
      ]);
      const expectedRevision = baseline?.resources[providerId]?.revision;
      if (expectedRevision === undefined) {
        throw new Error(
          "Provider revision is unavailable; refresh the Provider list before saving",
        );
      }
      try {
        await providersApi.update(provider, activeApp, originalId, {
          expectedRevision,
          ...options,
        });
        await invalidate();
        toast.success(
          t("notifications.updateSuccess", {
            defaultValue: "供应商更新成功",
          }),
          { closeButton: true },
        );
      } catch (error) {
        const detail = extractErrorMessage(error) || t("common.unknown");
        toast.error(
          t("notifications.updateFailed", {
            defaultValue: "更新供应商失败: {{error}}",
            error: detail,
          }),
        );
        throw error;
      }
    },
    [activeApp, invalidate, queryClient, t],
  );

  const deleteProvider = useCallback(
    async (providerId: string) => {
      const resource = queryClient.getQueryData<ProvidersQueryData>([
        "providers",
        activeApp,
      ])?.resources[providerId];
      try {
        await providersApi.delete(providerId, activeApp, resource?.revision);
        await invalidate();
        toast.success(
          t("notifications.deleteSuccess", {
            defaultValue: "供应商已删除",
          }),
          { closeButton: true },
        );
      } catch (error) {
        const detail = extractErrorMessage(error) || t("common.unknown");
        toast.error(
          t("notifications.deleteFailed", {
            defaultValue: "删除供应商失败: {{error}}",
            error: detail,
          }),
        );
        throw error;
      }
    },
    [activeApp, invalidate, queryClient, t],
  );

  return {
    addProvider,
    updateProvider,
    deleteProvider,
  };
}
