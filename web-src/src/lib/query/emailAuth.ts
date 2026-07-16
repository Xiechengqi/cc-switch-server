import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import { emailAuthApi } from "@/lib/api/emailAuth";
import { extractErrorMessage } from "@/utils/errorUtils";
import { shareKeys } from "./share";

export const emailAuthKeys = {
  all: ["email-auth"] as const,
  status: () => [...emailAuthKeys.all, "status"] as const,
  session: () => [...emailAuthKeys.all, "session"] as const,
};

export function useEmailAuthChangeOwnerEmailMutation() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    mutationFn: (params: {
      routerDomain?: string;
      currentEmail: string;
      newEmail: string;
    }) => emailAuthApi.changeOwnerEmail(params),
    onSuccess: async (status) => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: emailAuthKeys.status() }),
        queryClient.invalidateQueries({ queryKey: emailAuthKeys.session() }),
        queryClient.invalidateQueries({ queryKey: shareKeys.list() }),
        queryClient.invalidateQueries({ queryKey: shareKeys.clientTunnel() }),
        queryClient.invalidateQueries({ queryKey: ["settings"] }),
      ]);
      toast.success(
        t("share.ownerChange.success", {
          defaultValue: "Owner 已换绑为 {{email}}",
          email: status.email ?? "",
        }),
      );
    },
    onError: (error: Error) => {
      toast.error(
        t("share.ownerChange.failed", {
          defaultValue: "换绑 owner 失败：{{error}}",
          error: extractErrorMessage(error),
        }),
      );
    },
  });
}
