import { useState } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { useTranslation } from "react-i18next";

import { ProviderIcon } from "@/components/ProviderIcon";
import { Button } from "@/components/ui/button";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { useUsageRequests } from "@/lib/query/usage";
import {
  requestProcessedTokens,
  type UsageApp,
  type UsageFilters,
  type UsageRangeSelection,
} from "@/types/usage";
import { fmtInt, getLocaleFromLanguage } from "./format";

const APP_ICONS: Record<UsageApp, string> = { claude: "claude", codex: "openai", gemini: "gemini" };

interface RequestLogTableProps {
  range: UsageRangeSelection;
  filters: UsageFilters;
  refreshIntervalMs: number;
  onSelect: (requestId: string) => void;
}

export function RequestLogTable({ range, filters, refreshIntervalMs, onSelect }: RequestLogTableProps) {
  const { t, i18n } = useTranslation();
  const [page, setPage] = useState(0);
  const [cursors, setCursors] = useState<Array<string | undefined>>([undefined]);
  const cursor = cursors[page];
  const response = useUsageRequests({
    range,
    filters,
    cursor,
    limit: 50,
    options: { refetchInterval: refreshIntervalMs || false },
  });
  const locale = getLocaleFromLanguage(i18n.resolvedLanguage || i18n.language || "en");
  const rows = response.data?.data ?? [];
  const meta = response.data?.meta;

  const nextPage = () => {
    const nextCursor = meta?.nextCursor || undefined;
    if (!nextCursor) return;
    setCursors((current) => [...current.slice(0, page + 1), nextCursor]);
    setPage((current) => current + 1);
  };

  return (
    <div className="space-y-3">
      <div className="overflow-x-auto rounded-md border bg-card">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>{t("usage.time", "时间")}</TableHead>
              <TableHead>{t("usage.surface", "应用接口")}</TableHead>
              <TableHead>{t("usage.providerBundle", "供应商")}</TableHead>
              <TableHead>{t("usage.actualModel", "实际上游模型")}</TableHead>
              <TableHead>{t("usage.user", "用户")}</TableHead>
              <TableHead className="text-right">{t("usage.tokens", "Tokens")}</TableHead>
              <TableHead className="text-right">{t("usage.endToEnd", "端到端")}</TableHead>
              <TableHead className="text-right">{t("usage.attempts", "尝试")}</TableHead>
              <TableHead>{t("usage.outcome", "结果")}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {response.isLoading ? (
              <TableRow><TableCell colSpan={9} className="h-28 text-center text-muted-foreground">{t("common.loading", "加载中")}</TableCell></TableRow>
            ) : response.isError ? (
              <TableRow><TableCell colSpan={9} className="h-28 text-center text-muted-foreground">{t("usage.queryFailed", "查询失败")}</TableCell></TableRow>
            ) : rows.length === 0 ? (
              <TableRow><TableCell colSpan={9} className="h-28 text-center text-muted-foreground">{t("usage.noData", "暂无数据")}</TableCell></TableRow>
            ) : rows.map((row) => (
              <TableRow key={row.requestId} className="cursor-pointer" onClick={() => onSelect(row.requestId)}>
                <TableCell className="whitespace-nowrap text-xs">{new Date(row.startedAtMs).toLocaleString(locale)}</TableCell>
                <TableCell><span className="flex items-center gap-2"><ProviderIcon icon={APP_ICONS[row.app]} name={row.app} size={17} /><span className="text-xs">{t(`usage.appFilter.${row.app}`, row.app)}</span></span></TableCell>
                <TableCell className="max-w-[210px]"><div className="truncate text-sm" title={row.providerName}>{row.providerName}</div><div className="truncate font-mono text-[11px] text-muted-foreground" title={row.bundleId}>{row.bundleId}</div></TableCell>
                <TableCell className="max-w-[220px] truncate font-mono text-xs" title={row.actualModel || row.requestedModel || row.model || "unknown"}>{row.actualModel || row.requestedModel || row.model || "unknown"}</TableCell>
                <TableCell className="max-w-[180px] truncate font-mono text-xs" title={row.userEmail || row.accountDisplay || "-"}>{row.userEmail || row.accountDisplay || "-"}</TableCell>
                <TableCell className="text-right tabular-nums">{row.usageState === "observed" ? fmtInt(requestProcessedTokens(row), locale) : t(`usage.usageState.${row.usageState}`, row.usageState)}</TableCell>
                <TableCell className="text-right tabular-nums">{row.completedAtMs > 0 ? `${row.endToEndDurationMs} ms` : "-"}</TableCell>
                <TableCell className="text-right tabular-nums">{row.attemptCount}</TableCell>
                <TableCell><span className={row.outcome === "success" ? "text-emerald-600 dark:text-emerald-400" : row.outcome === "pending" ? "text-amber-600 dark:text-amber-400" : "text-red-600 dark:text-red-400"}>{t(`usage.outcomeValue.${row.outcome}`, row.outcome)}</span><span className="ml-1 text-xs text-muted-foreground">{row.statusCode}</span></TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
      <div className="flex items-center justify-between gap-3 text-sm text-muted-foreground">
        <span>{t("usage.totalRecords", { total: meta?.total ?? 0, defaultValue: "共 {{total}} 条" })}</span>
        <div className="flex items-center gap-2">
          <Button type="button" size="icon" variant="outline" title={t("common.previous", "上一页")} disabled={page === 0} onClick={() => setPage((current) => Math.max(0, current - 1))}><ChevronLeft className="h-4 w-4" /></Button>
          <span className="min-w-12 text-center tabular-nums">{page + 1}</span>
          <Button type="button" size="icon" variant="outline" title={t("common.next", "下一页")} disabled={!meta?.nextCursor} onClick={nextPage}><ChevronRight className="h-4 w-4" /></Button>
        </div>
      </div>
    </div>
  );
}
