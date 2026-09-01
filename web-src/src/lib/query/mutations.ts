import { useMutation, useQueryClient } from "@tanstack/react-query";

import { settingsApi } from "@/lib/api";
import type { Settings } from "@/types";

export const useSaveSettingsMutation = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (settings: Settings) => {
      await settingsApi.save(settings);
    },
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
  });
};
