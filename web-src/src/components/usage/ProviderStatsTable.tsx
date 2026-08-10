import { useTranslation } from "react-i18next";

import { ProviderIcon } from "@/components/ProviderIcon";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { useProviderBundles } from "@/lib/query/usage";
import type { UsageApp, UsageFilters, UsageRangeSelection } from "@/types/usage";
import { fmtInt, getLocaleFromLanguage } from "./format";

const APP_ICONS: Record<UsageApp, string> = {
  claude: "claude",
  codex: "openai",
  gemini: "gemini",
};

interface ProviderStatsTableProps {
  range: UsageRangeSelection;
  filters: UsageFilters;
  refreshIntervalMs: number;
}

export function ProviderStatsTable({ range, filters, refreshIntervalMs }: ProviderStatsTableProps) {
  const { t, i18n } = useTranslation();
  const response = useProviderBundles({
    range,
    filters,
    options: { refetchInterval: refreshIntervalMs || false },
  });
  const rows = response.data?.data ?? [];
  const locale = getLocaleFromLanguage(i18n.resolvedLanguage || i18n.language || "en");

  return (
    <div className="overflow-x-auto rounded-md border bg-card">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>{t("usage.providerBundle", "供应商")}</TableHead>
            <TableHead>{t("usage.surfaces", "应用接口")}</TableHead>
            <TableHead className="text-right">{t("usage.requests", "请求数")}</TableHead>
            <TableHead className="text-right">{t("usage.tokens", "Tokens")}</TableHead>
            <TableHead className="text-right">{t("usage.successRate", "成功率")}</TableHead>
            <TableHead className="text-right">{t("usage.avgLatency", "平均耗时")}</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {response.isLoading ? (
            <TableRow><TableCell colSpan={6} className="h-28 text-center text-muted-foreground">{t("common.loading", "加载中")}</TableCell></TableRow>
          ) : response.isError ? (
            <TableRow><TableCell colSpan={6} className="h-28 text-center text-muted-foreground">{t("usage.queryFailed", "查询失败")}</TableCell></TableRow>
          ) : rows.length === 0 ? (
            <TableRow><TableCell colSpan={6} className="h-28 text-center text-muted-foreground">{t("usage.noData", "暂无数据")}</TableCell></TableRow>
          ) : (
            rows.map((row) => (
              <TableRow key={row.bundleId}>
                <TableCell>
                  <div className="font-medium">{row.providerName}</div>
                  <div className="max-w-[260px] truncate font-mono text-xs text-muted-foreground" title={row.bundleId}>{row.bundleId}</div>
                </TableCell>
                <TableCell>
                  <div className="flex min-w-[120px] items-center gap-2">
                    {row.supportedApps.map((app) => (
                      <span key={app} title={t(`usage.appFilter.${app}`, app)}>
                        <ProviderIcon icon={APP_ICONS[app]} name={app} size={18} />
                      </span>
                    ))}
                  </div>
                  <div className="mt-1 text-xs text-muted-foreground">
                    {row.surfaces.map((surface) => `${surface.app}: ${surface.metrics.requestCount}`).join(" · ")}
                  </div>
                </TableCell>
                <TableCell className="text-right tabular-nums">{fmtInt(row.metrics.requestCount, locale)}</TableCell>
                <TableCell className="text-right tabular-nums">{fmtInt(row.metrics.processedTokens, locale)}</TableCell>
                <TableCell className="text-right tabular-nums">{row.metrics.successRate.toFixed(1)}%</TableCell>
                <TableCell className="text-right tabular-nums">{row.metrics.averageEndToEndMs == null ? "-" : `${Math.round(row.metrics.averageEndToEndMs)} ms`}</TableCell>
              </TableRow>
            ))
          )}
        </TableBody>
      </Table>
    </div>
  );
}
