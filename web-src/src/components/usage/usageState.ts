import type {
  ProviderLimitStatus,
  UsageLog,
  UsageRollup,
  UsageStatsFilter,
} from "@/lib/api";
import {
  emptyDataSourceSummary,
  type UsageDataSourceSummary,
} from "@/components/usage/DataSourceBar";
import type { UsageFilterDraft } from "@/components/usage/UsageFilterBar";
import { freshInputTokens } from "@/components/usage/usageDisplay";

export function defaultFilterDraft(): UsageFilterDraft {
  return {
    range: "1d",
    customFrom: "",
    customTo: "",
    app: "all",
    providerId: "",
    shareId: "",
    userEmail: "",
    sessionId: "",
    dataSource: "",
    health: "all",
    streamStatus: "",
    limit: "100",
  };
}

export function filterFromDraft(draft: UsageFilterDraft): UsageStatsFilter {
  const bounds = rangeBounds(draft);
  const filter: UsageStatsFilter = {
    ...bounds,
    limit: positiveInt(draft.limit) || 100,
    windowMs: trendWindowMs(bounds),
  };
  if (draft.app !== "all") filter.app = draft.app;
  if (draft.providerId.trim()) filter.providerId = draft.providerId.trim();
  if (draft.shareId.trim()) filter.shareId = draft.shareId.trim();
  if (draft.userEmail.trim()) filter.userEmail = draft.userEmail.trim();
  if (draft.sessionId.trim()) filter.sessionId = draft.sessionId.trim();
  if (draft.dataSource.trim()) filter.dataSource = draft.dataSource.trim();
  if (draft.health !== "all") filter.isHealthCheck = draft.health === "true";
  if (draft.streamStatus.trim()) filter.streamStatus = draft.streamStatus.trim();
  return filter;
}

export function filterProviderLimits(limits: ProviderLimitStatus[], draft: UsageFilterDraft): ProviderLimitStatus[] {
  const app = draft.app === "all" ? "" : draft.app;
  const providerId = draft.providerId.trim().toLowerCase();
  if (!app && !providerId) return limits;
  return limits.filter((limit) => {
    if (app && limit.app !== app) return false;
    if (!providerId) return true;
    return [
      limit.providerId,
      limit.providerName,
      limit.providerType,
      limit.accountId,
      limit.accountEmail,
      ...limit.shares.map((share) => `${share.shareId} ${share.shareName} ${share.status}`),
    ]
      .filter(Boolean)
      .join(" ")
      .toLowerCase()
      .includes(providerId);
  });
}

export function emptyRollup(): UsageRollup {
  return {
    requests: 0,
    successes: 0,
    failures: 0,
    inputTokens: 0,
    outputTokens: 0,
    cacheReadTokens: 0,
    cacheCreationTokens: 0,
    totalTokens: 0,
    totalCostUsd: 0,
  };
}

export function dataSourceBreakdown(logs: UsageLog[]): UsageDataSourceSummary[] {
  const summaries = new Map<string, UsageDataSourceSummary>();
  for (const log of logs) {
    const dataSource = (log.dataSource || "unknown").trim() || "unknown";
    const summary = summaries.get(dataSource) || emptyDataSourceSummary(dataSource);
    summary.requests += 1;
    if (log.statusCode >= 200 && log.statusCode < 300) {
      summary.successes += 1;
    } else {
      summary.failures += 1;
    }
    summary.totalTokens += log.totalTokens ?? freshInputTokens(log) + (log.outputTokens || 0) + (log.cacheReadTokens || 0) + (log.cacheCreationTokens || 0);
    summary.totalCostUsd += log.totalCostUsd || 0;
    if (log.isHealthCheck) summary.healthChecks += 1;
    summaries.set(dataSource, summary);
  }
  return [...summaries.values()].sort((left, right) => right.requests - left.requests || left.dataSource.localeCompare(right.dataSource));
}

export function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}

function rangeBounds(draft: UsageFilterDraft): Pick<UsageStatsFilter, "fromMs" | "toMs"> {
  const now = Date.now();
  if (draft.range === "all") return {};
  if (draft.range === "custom") {
    return {
      fromMs: dateInputToMs(draft.customFrom),
      toMs: dateInputToMs(draft.customTo),
    };
  }
  if (draft.range === "today") {
    const start = new Date();
    start.setHours(0, 0, 0, 0);
    return { fromMs: start.getTime(), toMs: now };
  }
  const days = draft.range === "1d" ? 1 : draft.range === "7d" ? 7 : draft.range === "14d" ? 14 : 30;
  return { fromMs: now - days * 24 * 60 * 60 * 1000, toMs: now };
}

function trendWindowMs(bounds: Pick<UsageStatsFilter, "fromMs" | "toMs">): number {
  const duration = bounds.fromMs && bounds.toMs ? bounds.toMs - bounds.fromMs : 30 * 24 * 60 * 60 * 1000;
  if (duration <= 36 * 60 * 60 * 1000) return 60 * 60 * 1000;
  if (duration <= 10 * 24 * 60 * 60 * 1000) return 6 * 60 * 60 * 1000;
  return 24 * 60 * 60 * 1000;
}

function dateInputToMs(value: string): number | undefined {
  if (!value.trim()) return undefined;
  const parsed = new Date(value).getTime();
  return Number.isFinite(parsed) ? parsed : undefined;
}

function positiveInt(value: string): number | undefined {
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : undefined;
}
