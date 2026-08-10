export type UsageApp = "claude" | "codex" | "gemini";
export type UsageAppFilter = "all" | UsageApp;

export const USAGE_APPS: readonly UsageApp[] = ["claude", "codex", "gemini"];

export type UsageOutcome =
  | "pending"
  | "success"
  | "client_error"
  | "rate_limited"
  | "upstream_error"
  | "timeout"
  | "interrupted"
  | "internal_error";

export type UsageState =
  | "pending"
  | "observed"
  | "missing"
  | "parse_error"
  | "interrupted";

export interface UsageMetrics {
  requestCount: number;
  successCount: number;
  failureCount: number;
  pendingCount: number;
  processedTokens: number;
  freshInputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  supplementalTokens: number;
  observedUsageCount: number;
  missingUsageCount: number;
  parseErrorUsageCount: number;
  interruptedUsageCount: number;
  successRate: number;
  usageCoverage: number;
  averageEndToEndMs?: number | null;
  averageUpstreamMs?: number | null;
  averageFirstTokenMs?: number | null;
  lastRequestAtMs?: number | null;
}

export interface UsageSurface {
  app: UsageApp;
  metrics: UsageMetrics;
}

export interface UsageOverview {
  metrics: UsageMetrics;
  surfaces: UsageSurface[];
}

export interface UsageTrendPoint {
  startMs: number;
  endMs: number;
  metrics: UsageMetrics;
}

export interface BundleSurfaceUsage {
  app: UsageApp;
  providerId: string;
  providerType: string;
  metrics: UsageMetrics;
}

export interface ProviderBundleUsage {
  bundleId: string;
  providerName: string;
  familyId?: string | null;
  supportedApps: UsageApp[];
  metrics: UsageMetrics;
  surfaces: BundleSurfaceUsage[];
}

export interface ModelUsage {
  app: UsageApp;
  actualModel: string;
  requestedModels: string[];
  metrics: UsageMetrics;
}

export interface ShareUserUsage {
  userEmail: string;
  metrics: UsageMetrics;
}

export interface ShareUsage {
  shareId: string;
  shareName?: string | null;
  shareSlug?: string | null;
  metrics: UsageMetrics;
  users: ShareUserUsage[];
}

export interface ValueFacet {
  value: string;
  requestCount: number;
}

export interface BundleFacet {
  bundleId: string;
  providerName: string;
  supportedApps: UsageApp[];
  requestCount: number;
}

export interface ShareFacet {
  shareId: string;
  shareName?: string | null;
  shareSlug?: string | null;
  requestCount: number;
}

export interface UserFacet {
  userEmail: string;
  requestCount: number;
}

export interface ModelFacet {
  app: UsageApp;
  actualModel: string;
  requestCount: number;
}

export interface UsageFacets {
  surfaces: ValueFacet[];
  bundles: BundleFacet[];
  shares: ShareFacet[];
  users: UserFacet[];
  models: ModelFacet[];
  outcomes: ValueFacet[];
  usageStates: ValueFacet[];
}

export interface UsageRequest {
  requestId: string;
  recordKind: "user_inference" | "internal_supplemental" | "health_probe";
  parentRequestId?: string | null;
  app: UsageApp;
  bundleId: string;
  familyId?: string | null;
  supportedApps: UsageApp[];
  providerId: string;
  providerName: string;
  providerType: string;
  profileId?: string | null;
  accountRef?: string | null;
  accountDisplay?: string | null;
  authIdentityGeneration?: number | null;
  model?: string | null;
  requestAgent?: string | null;
  sessionId?: string | null;
  requestedModel?: string | null;
  actualModel?: string | null;
  actualModelSource?: string | null;
  requestedReasoningEffort?: string | null;
  effectiveReasoningEffort?: string | null;
  clientServiceTier?: string | null;
  effectiveServiceTier?: string | null;
  serviceTierDecision?: string | null;
  statusCode: number;
  outcome: UsageOutcome;
  failureKind?: string | null;
  errorMessage?: string | null;
  durationMs: number;
  startedAtMs: number;
  completedAtMs: number;
  endToEndDurationMs: number;
  upstreamDurationMs: number;
  attemptCount: number;
  firstTokenMs?: number | null;
  rawInputTokens?: number | null;
  inputTokens?: number | null;
  outputTokens?: number | null;
  cacheReadTokens?: number | null;
  cacheCreationTokens?: number | null;
  totalTokens?: number | null;
  imageCount?: number | null;
  imageBytes?: number | null;
  imageFormat?: string | null;
  imageWidth?: number | null;
  imageHeight?: number | null;
  imageSize?: string | null;
  shareId?: string | null;
  shareSlug?: string | null;
  shareName?: string | null;
  userEmail?: string | null;
  dataSource?: string | null;
  isStreaming: boolean;
  streamStatus?: string | null;
  usageState: UsageState;
  usageRevision: number;
  usageEstimated: boolean;
  userCountry?: string | null;
  userCountryIso3?: string | null;
}

export interface UsageResponseMeta {
  fromMs: number;
  toMs: number;
  generatedAtMs: number;
}

export interface UsageResponse<T> {
  data: T;
  meta: UsageResponseMeta;
}

export interface UsageRequestPageMeta extends UsageResponseMeta {
  total: number;
  nextCursor?: string | null;
}

export interface UsageRequestPage {
  data: UsageRequest[];
  meta: UsageRequestPageMeta;
}

export interface UsageFilters {
  app?: UsageApp;
  bundleId?: string;
  shareId?: string;
  userEmail?: string;
  actualModel?: string;
  outcome?: UsageOutcome;
  usageState?: UsageState;
}

export type UsageRangePreset = "today" | "1d" | "7d" | "14d" | "30d" | "custom";

export interface UsageRangeSelection {
  preset: UsageRangePreset;
  customStartDate?: number;
  customEndDate?: number;
  liveEndTime?: boolean;
}

export function hasObservedUsage(log: { usageState?: string | null }): boolean {
  return log.usageState === "observed";
}

export function usageToken(value?: number | null): number {
  return value ?? 0;
}

export function requestProcessedTokens(log: UsageRequest): number {
  return (
    usageToken(log.inputTokens) +
    usageToken(log.outputTokens) +
    usageToken(log.cacheReadTokens) +
    usageToken(log.cacheCreationTokens)
  );
}
