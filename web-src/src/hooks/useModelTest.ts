import { useState, useCallback } from "react";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import {
  modelTestProvider,
  type StreamCheckResult,
} from "@/lib/api/model-test";
import { useResetCircuitBreaker } from "@/lib/query/failover";
import type { AppId } from "@/lib/api";

/**
 * 供应商真实模型测试。
 *
 * 该检查会发送真实模型请求，用于验证鉴权、模型、配额和协议转换是否可用。
 * 与轻量“测试链接”不同，它可以作为供应商健康状态恢复信号。
 */
export function useModelTest(appId: AppId) {
  const { t } = useTranslation();
  const [testingIds, setTestingIds] = useState<Set<string>>(new Set());
  const resetCircuitBreaker = useResetCircuitBreaker();

  const testProvider = useCallback(
    async (
      providerId: string,
      providerName: string,
    ): Promise<StreamCheckResult | null> => {
      setTestingIds((prev) => new Set(prev).add(providerId));

      try {
        const result = await modelTestProvider(appId, providerId);

        if (result.status === "operational") {
          toast.success(
            t("streamCheck.operational", {
              providerName,
              responseTimeMs: result.responseTimeMs,
              defaultValue: `${providerName} 模型测试正常 (${result.responseTimeMs}ms)`,
            }),
            { closeButton: true },
          );
          resetCircuitBreaker.mutate({ providerId, appType: appId });
        } else if (result.status === "degraded") {
          toast.warning(
            t("streamCheck.degraded", {
              providerName,
              responseTimeMs: result.responseTimeMs,
              defaultValue: `${providerName} 模型可用但较慢 (${result.responseTimeMs}ms)`,
            }),
          );
          resetCircuitBreaker.mutate({ providerId, appType: appId });
        } else if (result.errorCategory === "modelNotFound") {
          toast.error(
            t("streamCheck.modelNotFound", {
              providerName,
              model: result.modelUsed,
              defaultValue: `${providerName} 测试模型 ${result.modelUsed} 不存在或已下架`,
            }),
            {
              description: t("streamCheck.modelNotFoundHint", {
                defaultValue: "",
              }),
              duration: 10000,
              closeButton: true,
            },
          );
        } else if (result.errorCategory === "quotaExceeded") {
          toast.warning(
            t("streamCheck.quotaExceeded", {
              providerName,
              defaultValue: `${providerName} Coding Plan quota has been exceeded`,
            }),
            {
              description: t("streamCheck.quotaExceededHint", {
                defaultValue: "",
              }),
              duration: 10000,
              closeButton: true,
            },
          );
        } else if (result.errorCategory === "codexOauthTokenInvalidated") {
          toast.warning(
            t("streamCheck.codexOauthTokenInvalidated", {
              providerName,
              defaultValue: `${providerName} OAuth token has been invalidated`,
            }),
            {
              description: t("streamCheck.codexOauthTokenInvalidatedHint", {
                defaultValue:
                  "cc-switch retried after refreshing the token, but OpenAI still rejected it. Sign in with OpenAI OAuth again.",
              }),
              duration: 10000,
              closeButton: true,
            },
          );
        } else if (result.errorCategory === "tokenInvalidated") {
          toast.warning(
            t("streamCheck.tokenInvalidated", {
              providerName,
              defaultValue: `${providerName} authentication token has been invalidated`,
            }),
            {
              description: t("streamCheck.tokenInvalidatedHint", {
                defaultValue:
                  "Refresh the managed account sign-in and try again.",
              }),
              duration: 10000,
              closeButton: true,
            },
          );
        } else {
          toast.error(
            t("streamCheck.failed", {
              providerName,
              message: result.message,
              defaultValue: `${providerName} 模型测试失败: ${result.message}`,
            }),
            {
              duration: 8000,
              closeButton: true,
            },
          );
        }

        return result;
      } catch (e) {
        toast.error(
          t("streamCheck.error", {
            providerName,
            error: String(e),
            defaultValue: `${providerName} 检查出错: ${String(e)}`,
          }),
        );
        return null;
      } finally {
        setTestingIds((prev) => {
          const next = new Set(prev);
          next.delete(providerId);
          return next;
        });
      }
    },
    [appId, resetCircuitBreaker, t],
  );

  const isTesting = useCallback(
    (providerId: string) => testingIds.has(providerId),
    [testingIds],
  );

  return { testProvider, isTesting };
}
