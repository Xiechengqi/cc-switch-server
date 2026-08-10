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
import { useModelUsage } from "@/lib/query/usage";
import type { UsageApp, UsageFilters, UsageRangeSelection } from "@/types/usage";
import { fmtInt, getLocaleFromLanguage } from "./format";

const APP_ICONS: Record<UsageApp, string> = { claude: "claude", codex: "openai", gemini: "gemini" };

interface ModelStatsTableProps {
  range: UsageRangeSelection;
  filters: UsageFilters;
  refreshIntervalMs: number;
}

export function ModelStatsTable({ range, filters, refreshIntervalMs }: ModelStatsTableProps) {
  const { t, i18n } = useTranslation();
  const response = useModelUsage({ range, filters, options: { refetchInterval: refreshIntervalMs || false } });
  const rows = response.data?.data ?? [];
  const locale = getLocaleFromLanguage(i18n.resolvedLanguage || i18n.language || "en");

  return (
    <div className="overflow-x-auto rounded-md border bg-card">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>{t("usage.surface", "应用接口")}</TableHead>
            <TableHead>{t("usage.actualModel", "实际上游模型")}</TableHead>
            <TableHead>{t("usage.requestedModels", "请求模型")}</TableHead>
            <TableHead className="text-right">{t("usage.requests", "请求数")}</TableHead>
            <TableHead className="text-right">{t("usage.tokens", "Tokens")}</TableHead>
            <TableHead className="text-right">{t("usage.successRate", "成功率")}</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {response.isLoading ? (
            <TableRow><TableCell colSpan={6} className="h-28 text-center text-muted-foreground">{t("common.loading", "加载中")}</TableCell></TableRow>
          ) : response.isError ? (
            <TableRow><TableCell colSpan={6} className="h-28 text-center text-muted-foreground">{t("usage.queryFailed", "查询失败")}</TableCell></TableRow>
          ) : rows.length === 0 ? (
            <TableRow><TableCell colSpan={6} className="h-28 text-center text-muted-foreground">{t("usage.noData", "暂无数据")}</TableCell></TableRow>
          ) : rows.map((row) => (
            <TableRow key={`${row.app}:${row.actualModel}`}>
              <TableCell><span className="flex items-center gap-2"><ProviderIcon icon={APP_ICONS[row.app]} name={row.app} size={18} /><span>{t(`usage.appFilter.${row.app}`, row.app)}</span></span></TableCell>
              <TableCell className="font-mono text-xs">{row.actualModel}</TableCell>
              <TableCell className="max-w-[280px] truncate font-mono text-xs text-muted-foreground" title={row.requestedModels.join(", ")}>{row.requestedModels.join(", ") || "-"}</TableCell>
              <TableCell className="text-right tabular-nums">{fmtInt(row.metrics.requestCount, locale)}</TableCell>
              <TableCell className="text-right tabular-nums">{fmtInt(row.metrics.processedTokens, locale)}</TableCell>
              <TableCell className="text-right tabular-nums">{row.metrics.successRate.toFixed(1)}%</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}
