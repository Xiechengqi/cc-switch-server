import { useQuery } from "@tanstack/react-query";

import { usageApi, type UsageApiQuery } from "@/lib/api/usage";
import { resolveUsageRange } from "@/lib/usageRange";
import type {
  UsageFilters,
  UsageRangeSelection,
} from "@/types/usage";

const DEFAULT_REFETCH_INTERVAL_MS = 30_000;

export type UsageQueryOptions = {
  refetchInterval?: number | false;
  refetchIntervalInBackground?: boolean;
};

export type UsageQueryInput = {
  range: UsageRangeSelection;
  filters?: UsageFilters;
  options?: UsageQueryOptions;
};

export type UsageRequestsInput = UsageQueryInput & {
  cursor?: string;
  limit?: number;
};

function apiQuery(range: UsageRangeSelection, filters: UsageFilters = {}): UsageApiQuery {
  const { startDate, endDate } = resolveUsageRange(range);
  return {
    fromMs: startDate * 1_000,
    toMs: endDate * 1_000 + 1_000,
    ...filters,
  };
}

function rangeKey(range: UsageRangeSelection) {
  return [
    range.preset,
    range.customStartDate ?? null,
    range.customEndDate ?? null,
    range.liveEndTime ?? false,
  ] as const;
}

function filterKey(filters: UsageFilters = {}) {
  return [
    filters.app ?? null,
    filters.bundleId ?? null,
    filters.shareId ?? null,
    filters.userEmail ?? null,
    filters.actualModel ?? null,
    filters.outcome ?? null,
    filters.usageState ?? null,
  ] as const;
}

function queryOptions(options?: UsageQueryOptions) {
  return {
    refetchInterval: options?.refetchInterval ?? DEFAULT_REFETCH_INTERVAL_MS,
    refetchIntervalInBackground: options?.refetchIntervalInBackground ?? false,
  };
}

export const usageKeys = {
  all: ["usage"] as const,
  resource: (
    resource: string,
    range: UsageRangeSelection,
    filters?: UsageFilters,
    suffix: readonly unknown[] = [],
  ) =>
    [
      ...usageKeys.all,
      resource,
      ...rangeKey(range),
      ...filterKey(filters),
      ...suffix,
    ] as const,
  detail: (requestId: string) => [...usageKeys.all, "request", requestId] as const,
};

export function useUsageOverview({ range, filters, options }: UsageQueryInput) {
  return useQuery({
    queryKey: usageKeys.resource("overview", range, filters),
    queryFn: () => usageApi.overview(apiQuery(range, filters)),
    ...queryOptions(options),
  });
}

export function useUsageTrends({ range, filters, options }: UsageQueryInput) {
  const { startDate, endDate } = resolveUsageRange(range);
  const windowMs = endDate - startDate <= 24 * 60 * 60 ? 60 * 60 * 1_000 : 24 * 60 * 60 * 1_000;
  return useQuery({
    queryKey: usageKeys.resource("trends", range, filters, [windowMs]),
    queryFn: () => usageApi.trends(apiQuery(range, filters), windowMs),
    ...queryOptions(options),
  });
}

export function useUsageFacets({ range, filters, options }: UsageQueryInput) {
  return useQuery({
    queryKey: usageKeys.resource("facets", range, filters),
    queryFn: () => usageApi.facets(apiQuery(range, filters)),
    ...queryOptions(options),
  });
}

export function useProviderBundles({ range, filters, options }: UsageQueryInput) {
  return useQuery({
    queryKey: usageKeys.resource("provider-bundles", range, filters),
    queryFn: () => usageApi.providerBundles(apiQuery(range, filters)),
    ...queryOptions(options),
  });
}

export function useModelUsage({ range, filters, options }: UsageQueryInput) {
  return useQuery({
    queryKey: usageKeys.resource("models", range, filters),
    queryFn: () => usageApi.models(apiQuery(range, filters)),
    ...queryOptions(options),
  });
}

export function useShareUsage({ range, filters, options }: UsageQueryInput) {
  return useQuery({
    queryKey: usageKeys.resource("shares", range, filters),
    queryFn: () => usageApi.shares(apiQuery(range, filters)),
    ...queryOptions(options),
  });
}

export function useUsageRequests({
  range,
  filters,
  cursor,
  limit = 50,
  options,
}: UsageRequestsInput) {
  return useQuery({
    queryKey: usageKeys.resource("requests", range, filters, [cursor ?? null, limit]),
    queryFn: () => usageApi.requests(apiQuery(range, filters), cursor, limit),
    ...queryOptions(options),
  });
}

export function useUsageRequest(requestId: string) {
  return useQuery({
    queryKey: usageKeys.detail(requestId),
    queryFn: () => usageApi.request(requestId),
    enabled: requestId.length > 0,
  });
}
