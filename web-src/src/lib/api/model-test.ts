import { invokeCommand } from "@/lib/runtime";
import type { AppId } from "./types";

// ===== 连通性检查类型 =====
// 注意：本检查只探测 base_url 是否可达，不发真实大模型请求，也不触碰故障转移熔断器。

export type HealthStatus = "operational" | "degraded" | "failed";

export interface StreamCheckResult {
  status: HealthStatus;
  success: boolean;
  message: string;
  responseTimeMs?: number;
  httpStatus?: number;
  testedAt: number;
  retryCount: number;
  /** 细粒度错误分类，如 "modelNotFound" */
  errorCategory?: string;
  /** 实际探测使用的模型名（用于错误消息中提示哪个模型未找到） */
  modelUsed?: string;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
}

// ===== 连通性检查 API =====

/**
 * 连通性检查（单个供应商）
 */
export async function streamCheckProvider(
  appType: AppId,
  providerId: string,
): Promise<StreamCheckResult> {
  return invokeCommand("stream_check_provider", { appType, providerId });
}

/**
 * 批量流式健康检查
 */
export async function streamCheckAllProviders(
  appType: AppId,
  proxyTargetsOnly: boolean = false,
): Promise<Array<[string, StreamCheckResult]>> {
  return invokeCommand("stream_check_all_providers", {
    appType,
    proxyTargetsOnly,
  });
}

/**
 * 真实模型测试（单个供应商）
 */
export async function modelTestProvider(
  appType: AppId,
  providerId: string,
): Promise<StreamCheckResult> {
  return invokeCommand("model_test_provider", { appType, providerId });
}

/**
 * 批量真实模型测试
 */
export async function modelTestAllProviders(
  appType: AppId,
  proxyTargetsOnly: boolean = false,
): Promise<Array<[string, StreamCheckResult]>> {
  return invokeCommand("model_test_all_providers", {
    appType,
    proxyTargetsOnly,
  });
}
