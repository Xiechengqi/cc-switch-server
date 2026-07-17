import {
  useMutation,
  useQueryClient,
} from "@tanstack/react-query";
import { saveUpgradePolicy } from "@/lib/server-legacy-api";
import {
  DEFAULT_UPGRADE_POLICY,
  selectUpgradePolicy,
} from "@/lib/upgradePolicyDefaults";
import type { Settings, UpgradePolicy } from "@/types";
import { useSettingsQuery } from "./queries";

export { DEFAULT_UPGRADE_POLICY, selectUpgradePolicy };

export function useUpgradePolicyQuery() {
  const settingsQuery = useSettingsQuery();
  const policy = selectUpgradePolicy(settingsQuery.data);
  const isPlaceholder = settingsQuery.isPlaceholderData;
  const isLoading = settingsQuery.isPending || isPlaceholder;

  return {
    policy,
    isLoading,
    isPlaceholder,
    isFetched: settingsQuery.isFetched,
    isFetching: settingsQuery.isFetching,
    error: settingsQuery.error,
  };
}

export function useSaveUpgradePolicyMutation() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (policy: UpgradePolicy) => saveUpgradePolicy(policy),
    onMutate: async (next) => {
      await queryClient.cancelQueries({ queryKey: ["settings"] });
      const previous = queryClient.getQueryData<Settings>(["settings"]);
      if (previous) {
        queryClient.setQueryData<Settings>(["settings"], {
          ...previous,
          upgradePolicy: next,
        });
      }
      return { previous };
    },
    onError: (_error, _next, context) => {
      if (context?.previous) {
        queryClient.setQueryData(["settings"], context.previous);
      }
    },
    onSuccess: (saved) => {
      queryClient.setQueryData<Settings>(["settings"], (current) =>
        current ? { ...current, upgradePolicy: saved } : current,
      );
    },
  });
}
