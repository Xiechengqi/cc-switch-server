import { useTranslation } from "react-i18next";

import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useUsageRequest } from "@/lib/query/usage";
import { requestProcessedTokens, usageToken } from "@/types/usage";
import { getLocaleFromLanguage } from "./format";

interface RequestDetailPanelProps {
  requestId: string;
  onClose: () => void;
}

function Detail({ label, value, mono = false }: { label: string; value: React.ReactNode; mono?: boolean }) {
  return <div className="min-w-0"><dt className="text-xs text-muted-foreground">{label}</dt><dd className={`mt-1 break-words text-sm ${mono ? "font-mono text-xs" : ""}`}>{value ?? "-"}</dd></div>;
}

export function RequestDetailPanel({ requestId, onClose }: RequestDetailPanelProps) {
  const { t, i18n } = useTranslation();
  const response = useUsageRequest(requestId);
  const request = response.data?.data;
  const locale = getLocaleFromLanguage(i18n.resolvedLanguage || i18n.language || "en");
  const yes = t("usage.yes", "Yes");
  const no = t("usage.no", "No");
  const serviceTierDecision = request?.serviceTierDecision
    ? t(`usage.serviceTierDecision.${request.serviceTierDecision}`, request.serviceTierDecision)
    : undefined;
  const imageDimensions = request?.imageWidth != null && request.imageHeight != null
    ? `${request.imageWidth} x ${request.imageHeight}`
    : request?.imageSize;

  return (
    <Dialog open onOpenChange={(open) => { if (!open) onClose(); }}>
      <DialogContent className="max-h-[85vh] max-w-3xl overflow-y-auto">
        <DialogHeader><DialogTitle>{t("usage.requestDetail", "请求详情")}</DialogTitle></DialogHeader>
        {response.isLoading ? <div className="h-64 animate-pulse rounded bg-muted" /> : response.isError ? (
          <div className="py-16 text-center text-muted-foreground">{t("usage.queryFailed", "查询失败")}</div>
        ) : !request ? (
          <div className="py-16 text-center text-muted-foreground">{t("usage.requestNotFound", "请求未找到")}</div>
        ) : (
          <div className="space-y-6">
            <dl className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
              <Detail label={t("usage.requestId", "请求 ID")} value={request.requestId} mono />
              <Detail label={t("usage.recordKind", "记录类型")} value={request.recordKind} mono />
              <Detail label={t("usage.parentRequestId", "父请求 ID")} value={request.parentRequestId} mono />
              <Detail label={t("usage.startedAt", "开始时间")} value={new Date(request.startedAtMs).toLocaleString(locale)} />
              <Detail label={t("usage.completedAt", "完成时间")} value={request.completedAtMs > 0 ? new Date(request.completedAtMs).toLocaleString(locale) : t("usage.pending", "进行中")} />
              <Detail label={t("usage.outcome", "结果")} value={`${t(`usage.outcomeValue.${request.outcome}`, request.outcome)} (${request.statusCode})`} mono />
              <Detail label={t("usage.failureKind", "失败分类")} value={request.failureKind} mono />
              <Detail label={t("usage.errorMessage", "错误信息")} value={request.errorMessage} mono />
              <Detail label={t("usage.usageStateLabel", "Usage 状态")} value={t(`usage.usageState.${request.usageState}`, request.usageState)} mono />
              <Detail label={t("usage.usageRevision", "Usage 修订")} value={request.usageRevision} />
              <Detail label={t("usage.usageEstimated", "估算 Usage")} value={request.usageEstimated ? yes : no} />
            </dl>

            <section>
              <h3 className="mb-3 text-sm font-semibold">{t("usage.routeIdentity", "路由与身份")}</h3>
              <dl className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
                <Detail label={t("usage.surface", "应用接口")} value={request.app} />
                <Detail label={t("usage.providerBundle", "供应商 Bundle")} value={`${request.providerName} · ${request.bundleId}`} mono />
                <Detail label={t("usage.familyId", "供应商家族")} value={request.familyId} mono />
                <Detail label={t("usage.supportedApps", "支持的应用接口")} value={request.supportedApps.map((app) => t(`usage.appFilter.${app}`, app)).join(", ")} />
                <Detail label={t("usage.providerId", "Surface Provider ID")} value={request.providerId} mono />
                <Detail label={t("usage.providerType", "供应商类型")} value={request.providerType} mono />
                <Detail label={t("usage.profileId", "Profile ID")} value={request.profileId} mono />
                <Detail label={t("usage.account", "上游账号")} value={request.accountDisplay} mono />
                <Detail label={t("usage.accountRef", "账号引用")} value={request.accountRef} mono />
                <Detail label={t("usage.identityGeneration", "身份代次")} value={request.authIdentityGeneration} />
                <Detail label={t("usage.share", "Share")} value={request.shareName} />
                <Detail label={t("usage.shareId", "Share ID")} value={request.shareId} mono />
                <Detail label={t("usage.shareSlug", "Share 子域名")} value={request.shareSlug} mono />
                <Detail label={t("usage.userEmail", "用户邮箱")} value={request.userEmail} mono />
                <Detail label={t("usage.userCountry", "用户国家/地区")} value={[request.userCountry, request.userCountryIso3].filter(Boolean).join(" · ") || undefined} />
                <Detail label={t("usage.dataSourceLabel", "数据来源")} value={request.dataSource} mono />
                <Detail label={t("usage.requestAgent", "请求客户端")} value={request.requestAgent} mono />
                <Detail label={t("usage.sessionId", "会话 ID")} value={request.sessionId} mono />
              </dl>
            </section>

            <section>
              <h3 className="mb-3 text-sm font-semibold">{t("usage.modelPath", "模型路径")}</h3>
              <dl className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
                <Detail label={t("usage.requestedModel", "请求模型")} value={request.requestedModel || request.model} mono />
                <Detail label={t("usage.actualModel", "实际上游模型")} value={request.actualModel || request.model} mono />
                <Detail label={t("usage.actualModelSource", "模型决策来源")} value={request.actualModelSource} mono />
                <Detail label={t("usage.requestedReasoningEffort", "请求推理等级")} value={request.requestedReasoningEffort} mono />
                <Detail label={t("usage.effectiveReasoningEffort", "实际推理等级")} value={request.effectiveReasoningEffort} mono />
                <Detail label={t("usage.clientServiceTier", "客户端 Service Tier")} value={request.clientServiceTier} mono />
                <Detail label={t("usage.effectiveServiceTier", "实际 Service Tier")} value={request.effectiveServiceTier} mono />
                <Detail label={t("usage.fastDecision", "Service Tier 决策")} value={serviceTierDecision} />
                <Detail label={t("usage.attempts", "上游尝试次数")} value={request.attemptCount} />
              </dl>
            </section>

            <section>
              <h3 className="mb-3 text-sm font-semibold">{t("usage.performance", "性能与 Token")}</h3>
              <dl className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
                <Detail label={t("usage.endToEnd", "端到端耗时")} value={`${request.endToEndDurationMs} ms`} />
                <Detail label={t("usage.upstreamDuration", "上游耗时")} value={`${request.upstreamDurationMs} ms`} />
                <Detail label={t("usage.firstToken", "首 Token")} value={request.firstTokenMs == null ? "-" : `${request.firstTokenMs} ms`} />
                <Detail label={t("usage.streaming", "流式请求")} value={request.isStreaming ? yes : no} />
                <Detail label={t("usage.streamStatus", "流状态")} value={request.streamStatus} mono />
                <Detail label={t("usage.processedTokens", "处理 Tokens")} value={requestProcessedTokens(request).toLocaleString(locale)} />
                <Detail label={t("usage.rawInput", "原始输入")} value={request.rawInputTokens?.toLocaleString(locale)} />
                <Detail label={t("usage.freshInput", "新增输入")} value={usageToken(request.inputTokens).toLocaleString(locale)} />
                <Detail label={t("usage.output", "输出")} value={usageToken(request.outputTokens).toLocaleString(locale)} />
                <Detail label={t("usage.cacheRead", "缓存读取")} value={usageToken(request.cacheReadTokens).toLocaleString(locale)} />
                <Detail label={t("usage.cacheWrite", "缓存写入")} value={usageToken(request.cacheCreationTokens).toLocaleString(locale)} />
                <Detail label={t("usage.reportedTotal", "上游上报总量")} value={request.totalTokens?.toLocaleString(locale)} />
              </dl>
            </section>

            {(request.imageCount != null || request.imageBytes != null || request.imageFormat || imageDimensions) && (
              <section>
                <h3 className="mb-3 text-sm font-semibold">{t("usage.imageOutput", "图片输出")}</h3>
                <dl className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
                  <Detail label={t("usage.imageCount", "图片数量")} value={request.imageCount} />
                  <Detail label={t("usage.imageBytes", "输出字节")} value={request.imageBytes?.toLocaleString(locale)} />
                  <Detail label={t("usage.imageFormat", "图片格式")} value={request.imageFormat} mono />
                  <Detail label={t("usage.imageDimensions", "图片尺寸")} value={imageDimensions} mono />
                </dl>
              </section>
            )}
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
