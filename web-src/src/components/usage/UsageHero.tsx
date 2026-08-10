import { Activity, CheckCircle2, Clock3, DatabaseZap, Gauge } from "lucide-react";
import { useTranslation } from "react-i18next";

import { useUsageOverview } from "@/lib/query/usage";
import type { UsageFilters, UsageRangeSelection } from "@/types/usage";
import { fmtInt, getLocaleFromLanguage } from "./format";

interface UsageHeroProps {
  range: UsageRangeSelection;
  filters: UsageFilters;
  refreshIntervalMs: number;
}

export function UsageHero({ range, filters, refreshIntervalMs }: UsageHeroProps) {
  const { t, i18n } = useTranslation();
  const response = useUsageOverview({
    range,
    filters,
    options: { refetchInterval: refreshIntervalMs || false },
  });
  const metrics = response.data?.data.metrics;
  const locale = getLocaleFromLanguage(i18n.resolvedLanguage || i18n.language || "en");

  const items = [
    {
      label: t("usage.processedTokens", "处理 Tokens"),
      value: fmtInt(metrics?.processedTokens ?? 0, locale),
      detail: t("usage.supplementalTokens", {
        count: metrics?.supplementalTokens ?? 0,
        defaultValue: "补充调用 {{count}}",
      }),
      icon: DatabaseZap,
      tone: "text-blue-600 dark:text-blue-400",
    },
    {
      label: t("usage.totalRequests", "请求数"),
      value: fmtInt(metrics?.requestCount ?? 0, locale),
      detail: t("usage.pendingRequests", {
        count: metrics?.pendingCount ?? 0,
        defaultValue: "进行中 {{count}}",
      }),
      icon: Activity,
      tone: "text-neutral-700 dark:text-neutral-300",
    },
    {
      label: t("usage.successRate", "成功率"),
      value: `${(metrics?.successRate ?? 0).toFixed(1)}%`,
      detail: t("usage.failedRequests", {
        count: metrics?.failureCount ?? 0,
        defaultValue: "失败 {{count}}",
      }),
      icon: CheckCircle2,
      tone: "text-emerald-600 dark:text-emerald-400",
    },
    {
      label: t("usage.usageCoverage", "Usage 覆盖率"),
      value: `${(metrics?.usageCoverage ?? 0).toFixed(1)}%`,
      detail: t("usage.missingUsage", {
        count: (metrics?.missingUsageCount ?? 0) + (metrics?.parseErrorUsageCount ?? 0),
        defaultValue: "缺失或解析失败 {{count}}",
      }),
      icon: Gauge,
      tone: "text-amber-600 dark:text-amber-400",
    },
    {
      label: t("usage.avgEndToEnd", "平均端到端耗时"),
      value:
        metrics?.averageEndToEndMs != null
          ? `${Math.round(metrics.averageEndToEndMs)} ms`
          : "-",
      detail:
        metrics?.averageFirstTokenMs != null
          ? t("usage.avgFirstTokenValue", {
              value: Math.round(metrics.averageFirstTokenMs),
              defaultValue: "首 Token {{value}} ms",
            })
          : t("usage.noFirstToken", "暂无首 Token 数据"),
      icon: Clock3,
      tone: "text-cyan-600 dark:text-cyan-400",
    },
  ];

  return (
    <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-5">
      {items.map(({ label, value, detail, icon: Icon, tone }) => (
        <div key={label} className="min-h-[112px] rounded-md border bg-card px-4 py-3">
          <div className="flex items-center justify-between gap-3 text-xs text-muted-foreground">
            <span>{label}</span>
            <Icon className={`h-4 w-4 shrink-0 ${tone}`} />
          </div>
          <div className="mt-3 text-2xl font-semibold tabular-nums">{response.isLoading || response.isError ? "-" : value}</div>
          <div className="mt-1 truncate text-xs text-muted-foreground" title={detail}>
            {detail}
          </div>
        </div>
      ))}
    </div>
  );
}
