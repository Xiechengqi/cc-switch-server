// 使用统计相关类型定义

export interface TokenUsage {
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
}

export interface RequestLog {
  requestId: string;
  providerId: string;
  providerName?: string;
  appType: string;
  model: string;
  requestModel?: string;
  requestAgent: string;
  requestedModel: string;
  actualModel: string;
  actualModelSource: string;
  rawInputTokens?: number | null;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  totalTokens?: number | null;
  isStreaming: boolean;
  latencyMs: number;
  firstTokenMs?: number;
  durationMs?: number;
  statusCode: number;
  errorMessage?: string;
  createdAt: number;
  shareId?: string;
  shareName?: string;
  userEmail?: string;
  dataSource?: string;
}

export interface SessionSyncResult {
  imported: number;
  skipped: number;
  filesScanned: number;
  errors: string[];
}

export interface DataSourceSummary {
  dataSource: string;
  requestCount: number;
  totalTokens: number;
}

export interface PaginatedLogs {
  data: RequestLog[];
  total: number;
  page: number;
  pageSize: number;
}

export interface UsageSummary {
  totalRequests: number;
  totalInputTokens: number;
  totalOutputTokens: number;
  totalCacheCreationTokens: number;
  totalCacheReadTokens: number;
  successRate: number;
  /** input + output + cache_creation + cache_read, all cache-normalized */
  realTotalTokens: number;
  /** cache_read / (input + cache_creation + cache_read), range 0–1 */
  cacheHitRate: number;
}

export interface UsageSummaryByApp {
  appType: string;
  summary: UsageSummary;
}

export interface DailyStats {
  date: number;
  requestCount: number;
  totalTokens: number;
  totalInputTokens: number;
  totalOutputTokens: number;
  totalCacheCreationTokens: number;
  totalCacheReadTokens: number;
}

export interface ProviderStats {
  providerId: string;
  providerName: string;
  requestCount: number;
  totalTokens: number;
  successRate: number;
  avgLatencyMs: number;
}

export interface ModelStats {
  model: string;
  requestCount: number;
  totalTokens: number;
  successRate: number;
  avgLatencyMs: number;
}

export interface LogFilters {
  appType?: string;
  providerName?: string;
  model?: string;
  shareId?: string;
  statusCode?: number;
  startDate?: number;
  endDate?: number;
}

/**
 * Dashboard 顶栏的全局筛选维度，作用于 Hero / 趋势图 / 三个统计 Tab。
 *
 * - `providerName` 按展示名精确匹配（与 Provider 统计列表同口径，含
 *   "Claude (Session)" 等会话占位名）；
 * - `model` 按实际模型优先、请求模型回落的有效模型匹配，与模型统计
 *   的分组口径一致。
 */
export interface UsageScopeFilters {
  appType?: string;
  providerName?: string;
  model?: string;
}

export type UsageRangePreset = "today" | "1d" | "7d" | "14d" | "30d" | "custom";

export interface UsageRangeSelection {
  preset: UsageRangePreset;
  customStartDate?: number;
  customEndDate?: number;
  /** When true (custom mode only), endDate resolves to "now" instead of the
   *  fixed customEndDate snapshot, and the end-time field becomes read-only. */
  liveEndTime?: boolean;
}

/**
 * App types surfaced as dashboard filter buttons.
 *
 * `claude-desktop` is a retained legacy app identifier, not a Server dashboard
 * category. Requests carrying that identifier remain visible in request detail,
 * while aggregate queries fold them into `claude` to avoid a partial duplicate
 * category.
 * `opencode` / `openclaw` / `hermes` have no proxy handler at all - they
 * appear only as managed apps elsewhere.
 */
export type AppType = "claude" | "codex" | "gemini" | "opencode";

export type AppTypeFilter = "all" | AppType;

export const KNOWN_APP_TYPES: ReadonlyArray<AppType> = [
  "claude",
  "codex",
  "gemini",
  "opencode",
];

/**
 * App types whose proxy uses an OpenAI-style protocol. The protocol does not
 * report cache _creation_ separately, only cache
 *    _reads_. So `cacheCreationTokens` is always 0 for these app types and
 *    the UI should label it as N/A rather than 0.
 *
 * Mirror of the Rust `CACHE_INCLUSIVE_APP_TYPES` whitelist.
 */
export const CACHE_INCLUSIVE_APP_TYPES: ReadonlySet<string> = new Set([
  "codex",
  "gemini",
]);

/** Subset of request-log fields needed to derive cache-normalized input. */
export interface CacheNormalizableLog {
  inputTokens: number;
}

/**
 * Request logs from the Server API already expose normalized fresh input.
 */
export function getFreshInputTokens(log: CacheNormalizableLog): number {
  return log.inputTokens;
}

export function getTotalTokens(
  log: CacheNormalizableLog & {
    rawInputTokens?: number | null;
    outputTokens: number;
    cacheReadTokens: number;
    cacheCreationTokens: number;
    totalTokens?: number | null;
  },
): number {
  return (
    log.totalTokens ??
    (log.rawInputTokens ??
      log.inputTokens + log.cacheReadTokens + log.cacheCreationTokens) +
      log.outputTokens
  );
}

export interface StatsFilters {
  timeRange: UsageRangePreset;
  providerId?: string;
  appType?: string;
}
