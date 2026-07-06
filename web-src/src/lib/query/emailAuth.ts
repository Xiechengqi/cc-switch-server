import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import { emailAuthApi } from "@/lib/api";
import { extractErrorMessage } from "@/utils/errorUtils";
import { shareKeys } from "./share";

export const emailAuthKeys = {
  all: ["email-auth"] as const,
  status: () => [...emailAuthKeys.all, "status"] as const,
  session: () => [...emailAuthKeys.all, "session"] as const,
};

export function useEmailAuthStatusQuery() {
  return useQuery({
    queryKey: emailAuthKeys.status(),
    queryFn: emailAuthApi.getStatus,
    refetchInterval: 30_000,
    refetchIntervalInBackground: true,
  });
}

export function useEmailAuthSessionMeQuery(enabled = true) {
  return useQuery({
    queryKey: emailAuthKeys.session(),
    queryFn: emailAuthApi.sessionMe,
    enabled,
    refetchInterval: enabled ? 60_000 : false,
    refetchIntervalInBackground: true,
  });
}

export function useEmailAuthRequestCodeMutation() {
  const { t } = useTranslation();
  return useMutation({
    mutationFn: (params: { routerDomain: string; email: string }) =>
      emailAuthApi.requestCode(params),
    onSuccess: (result) => {
      toast.success(
        t("settings.authCenter.emailCodeSent", {
          defaultValue: "验证码已发送到 {{target}}",
          target: result.maskedDestination,
        }),
      );
    },
    onError: (error: Error) => {
      toast.error(
        t("settings.authCenter.emailCodeSendFailed", {
          defaultValue: "发送验证码失败: {{error}}",
          error: extractErrorMessage(error),
        }),
      );
    },
  });
}

export function useEmailAuthVerifyCodeMutation() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  return useMutation({
    mutationFn: ({
      routerDomain,
      email,
      code,
    }: {
      routerDomain: string;
      email: string;
      code: string;
    }) => emailAuthApi.verifyCode(routerDomain, email, code),
    onSuccess: async (status) => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: emailAuthKeys.status() }),
        queryClient.invalidateQueries({ queryKey: emailAuthKeys.session() }),
      ]);
      toast.success(
        t("settings.authCenter.emailLoginSuccess", {
          defaultValue: "已登录邮箱 {{email}}",
          email: status.email ?? "",
        }),
      );
    },
    onError: (error: Error) => {
      toast.error(
        t("settings.authCenter.emailLoginFailed", {
          defaultValue: "邮箱登录失败: {{error}}",
          error: extractErrorMessage(error),
        }),
      );
    },
  });
}

export function useEmailAuthRequestOwnerChangeCodeMutation() {
  const { t } = useTranslation();
  return useMutation({
    mutationFn: (params: {
      routerDomain: string;
      currentEmail: string;
      newEmail: string;
    }) => emailAuthApi.requestOwnerChangeCode(params),
    onSuccess: (result) => {
      toast.success(
        t("share.ownerChange.codeSent", {
          defaultValue: "验证码已发送到 {{target}}",
          target: result.maskedDestination,
        }),
      );
    },
    onError: (error: Error) => {
      toast.error(
        t("share.ownerChange.codeSendFailed", {
          defaultValue: "发送换绑验证码失败：{{error}}",
          error: extractErrorMessage(error),
        }),
      );
    },
  });
}

export function useEmailAuthChangeOwnerEmailMutation() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  return useMutation({
    mutationFn: (params: {
      routerDomain: string;
      currentEmail: string;
      newEmail: string;
      code: string;
    }) => emailAuthApi.changeOwnerEmail(params),
    onSuccess: async (status) => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: emailAuthKeys.status() }),
        queryClient.invalidateQueries({ queryKey: emailAuthKeys.session() }),
        queryClient.invalidateQueries({ queryKey: shareKeys.list() }),
      ]);
      toast.success(
        t("share.ownerChange.success", {
          defaultValue: "Share owner 已换绑为 {{email}}",
          email: status.email ?? "",
        }),
      );
    },
    onError: (error: Error) => {
      toast.error(
        t("share.ownerChange.failed", {
          defaultValue: "换绑 share owner 失败：{{error}}",
          error: extractErrorMessage(error),
        }),
      );
    },
  });
}
