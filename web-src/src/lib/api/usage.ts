import { jsonFetch } from "@/lib/runtime";
import type {
  ModelUsage,
  ProviderBundleUsage,
  ShareUsage,
  UsageFacets,
  UsageFilters,
  UsageOverview,
  UsageRequest,
  UsageRequestPage,
  UsageResponse,
  UsageTrendPoint,
} from "@/types/usage";

export interface UsageApiQuery extends UsageFilters {
  fromMs: number;
  toMs: number;
}

function queryString(query: UsageApiQuery, extra: Record<string, unknown> = {}): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries({ ...query, ...extra })) {
    if (value !== undefined && value !== null && value !== "") {
      params.set(key, String(value));
    }
  }
  return params.toString();
}

function usageGet<T>(path: string, query: UsageApiQuery, extra?: Record<string, unknown>) {
  return jsonFetch<UsageResponse<T>>(
    `${path}?${queryString(query, extra)}`,
    { cache: "no-store" },
  );
}

export const usageApi = {
  overview: (query: UsageApiQuery) =>
    usageGet<UsageOverview>("/web-api/usage/overview", query),
  trends: (query: UsageApiQuery, windowMs?: number) =>
    usageGet<UsageTrendPoint[]>("/web-api/usage/trends", query, { windowMs }),
  facets: (query: UsageApiQuery) =>
    usageGet<UsageFacets>("/web-api/usage/facets", query),
  providerBundles: (query: UsageApiQuery) =>
    usageGet<ProviderBundleUsage[]>("/web-api/usage/provider-bundles", query),
  models: (query: UsageApiQuery) =>
    usageGet<ModelUsage[]>("/web-api/usage/models", query),
  shares: (query: UsageApiQuery) =>
    usageGet<ShareUsage[]>("/web-api/usage/shares", query),
  requests: (query: UsageApiQuery, cursor?: string, limit = 50) =>
    jsonFetch<UsageRequestPage>(
      `/web-api/usage/requests?${queryString(query, { cursor, limit })}`,
      { cache: "no-store" },
    ),
  request: (requestId: string) =>
    jsonFetch<UsageResponse<UsageRequest>>(
      `/web-api/usage/requests/${encodeURIComponent(requestId)}`,
      { cache: "no-store" },
    ),
};
