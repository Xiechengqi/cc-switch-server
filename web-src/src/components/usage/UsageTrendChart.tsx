import { useTranslation } from "react-i18next";
import {
  Area,
  AreaChart,
  CartesianGrid,
  Legend,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";

import { useUsageTrends } from "@/lib/query/usage";
import type { UsageFilters, UsageRangeSelection } from "@/types/usage";
import { fmtInt, getLocaleFromLanguage } from "./format";

interface UsageTrendChartProps {
  range: UsageRangeSelection;
  filters: UsageFilters;
  rangeLabel: string;
  refreshIntervalMs: number;
}

export function UsageTrendChart({
  range,
  filters,
  rangeLabel,
  refreshIntervalMs,
}: UsageTrendChartProps) {
  const { t, i18n } = useTranslation();
  const response = useUsageTrends({
    range,
    filters,
    options: { refetchInterval: refreshIntervalMs || false },
  });
  const locale = getLocaleFromLanguage(i18n.resolvedLanguage || i18n.language || "en");
  const data = (response.data?.data ?? []).map((point) => ({
    label: new Date(point.startMs).toLocaleString(locale, {
      month: "2-digit",
      day: "2-digit",
      hour: range.preset === "today" || range.preset === "1d" ? "2-digit" : undefined,
    }),
    input: point.metrics.freshInputTokens,
    output: point.metrics.outputTokens,
    cacheRead: point.metrics.cacheReadTokens,
    cacheWrite: point.metrics.cacheCreationTokens,
  }));

  return (
    <section className="rounded-md border bg-card p-4">
      <div className="mb-4 flex items-center justify-between gap-4">
        <h3 className="text-sm font-semibold">{t("usage.trends", "使用趋势")}</h3>
        <span className="truncate text-xs text-muted-foreground" title={rangeLabel}>
          {rangeLabel}
        </span>
      </div>
      <div className="h-[300px] min-w-0">
        {response.isLoading ? (
          <div className="flex h-full items-center justify-center text-sm text-muted-foreground">{t("common.loading", "加载中")}</div>
        ) : response.isError ? (
          <div className="flex h-full items-center justify-center text-sm text-muted-foreground">{t("usage.queryFailed", "查询失败")}</div>
        ) : data.length === 0 ? (
          <div className="flex h-full items-center justify-center text-sm text-muted-foreground">{t("usage.noData", "暂无数据")}</div>
        ) : <ResponsiveContainer width="100%" height="100%">
          <AreaChart data={data} margin={{ top: 8, right: 12, left: 0, bottom: 0 }}>
            <CartesianGrid strokeDasharray="3 3" vertical={false} opacity={0.35} />
            <XAxis dataKey="label" tick={{ fontSize: 11 }} tickLine={false} axisLine={false} />
            <YAxis
              tick={{ fontSize: 11 }}
              tickLine={false}
              axisLine={false}
              tickFormatter={(value) => fmtInt(Number(value), locale)}
              width={56}
            />
            <Tooltip formatter={(value) => fmtInt(Number(value), locale)} />
            <Legend wrapperStyle={{ fontSize: 12 }} />
            <Area type="monotone" dataKey="input" name={t("usage.freshInput", "新增输入")} stroke="#2563eb" fill="#2563eb" fillOpacity={0.08} />
            <Area type="monotone" dataKey="output" name={t("usage.output", "输出")} stroke="#16a34a" fill="#16a34a" fillOpacity={0.06} />
            <Area type="monotone" dataKey="cacheRead" name={t("usage.cacheRead", "缓存读取")} stroke="#0891b2" fill="#0891b2" fillOpacity={0.05} />
            <Area type="monotone" dataKey="cacheWrite" name={t("usage.cacheWrite", "缓存写入")} stroke="#d97706" fill="#d97706" fillOpacity={0.05} />
          </AreaChart>
        </ResponsiveContainer>}
      </div>
    </section>
  );
}
