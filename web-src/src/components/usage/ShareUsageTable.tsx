import { Fragment } from "react";
import { useTranslation } from "react-i18next";

import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { useShareUsage } from "@/lib/query/usage";
import type { UsageFilters, UsageRangeSelection } from "@/types/usage";
import { fmtInt, getLocaleFromLanguage } from "./format";

interface ShareUsageTableProps {
  range: UsageRangeSelection;
  filters: UsageFilters;
  refreshIntervalMs: number;
}

export function ShareUsageTable({ range, filters, refreshIntervalMs }: ShareUsageTableProps) {
  const { t, i18n } = useTranslation();
  const response = useShareUsage({ range, filters, options: { refetchInterval: refreshIntervalMs || false } });
  const rows = response.data?.data ?? [];
  const locale = getLocaleFromLanguage(i18n.resolvedLanguage || i18n.language || "en");

  return (
    <div className="overflow-x-auto rounded-md border bg-card">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>{t("usage.share", "Share")}</TableHead>
            <TableHead>{t("usage.userEmail", "用户邮箱")}</TableHead>
            <TableHead className="text-right">{t("usage.requests", "请求数")}</TableHead>
            <TableHead className="text-right">{t("usage.tokens", "Tokens")}</TableHead>
            <TableHead className="text-right">{t("usage.successRate", "成功率")}</TableHead>
            <TableHead className="text-right">{t("usage.usageCoverage", "Usage 覆盖率")}</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {response.isLoading ? (
            <TableRow><TableCell colSpan={6} className="h-28 text-center text-muted-foreground">{t("common.loading", "加载中")}</TableCell></TableRow>
          ) : response.isError ? (
            <TableRow><TableCell colSpan={6} className="h-28 text-center text-muted-foreground">{t("usage.queryFailed", "查询失败")}</TableCell></TableRow>
          ) : rows.length === 0 ? (
            <TableRow><TableCell colSpan={6} className="h-28 text-center text-muted-foreground">{t("usage.noData", "暂无数据")}</TableCell></TableRow>
          ) : rows.map((share) => (
            <Fragment key={share.shareId}>
              <TableRow className="bg-muted/25">
                <TableCell>
                  <div className="font-medium">{share.shareName || share.shareSlug || share.shareId}</div>
                  <div className="font-mono text-xs text-muted-foreground">{share.shareSlug || share.shareId}</div>
                </TableCell>
                <TableCell className="text-muted-foreground">{t("usage.allUsers", "所有用户")}</TableCell>
                <TableCell className="text-right tabular-nums">{fmtInt(share.metrics.requestCount, locale)}</TableCell>
                <TableCell className="text-right tabular-nums">{fmtInt(share.metrics.processedTokens, locale)}</TableCell>
                <TableCell className="text-right tabular-nums">{share.metrics.successRate.toFixed(1)}%</TableCell>
                <TableCell className="text-right tabular-nums">{share.metrics.usageCoverage.toFixed(1)}%</TableCell>
              </TableRow>
              {share.users.map((user) => (
                <TableRow key={`${share.shareId}:${user.userEmail}`}>
                  <TableCell />
                  <TableCell className="font-mono text-xs">{user.userEmail}</TableCell>
                  <TableCell className="text-right tabular-nums">{fmtInt(user.metrics.requestCount, locale)}</TableCell>
                  <TableCell className="text-right tabular-nums">{fmtInt(user.metrics.processedTokens, locale)}</TableCell>
                  <TableCell className="text-right tabular-nums">{user.metrics.successRate.toFixed(1)}%</TableCell>
                  <TableCell className="text-right tabular-nums">{user.metrics.usageCoverage.toFixed(1)}%</TableCell>
                </TableRow>
              ))}
            </Fragment>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}
