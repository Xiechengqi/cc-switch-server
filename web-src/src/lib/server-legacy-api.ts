import { invokeCommand, jsonFetch, readCachedPassword } from "@/lib/runtime";

export type AppKind = "claude" | "codex" | "gemini";

export interface Provider {
  id: string;
  name: string;
  settingsConfig?: Record<string, unknown>;
  category?: string | null;
  meta?: ProviderMeta | null;
  [key: string]: unknown;
}

export interface ProviderMeta {
  apiFormat?: string | null;
  providerType?: string | null;
  authBinding?: {
    source?: string | null;
    authProvider?: string | null;
    accountId?: string | null;
  } | null;
  [key: string]: unknown;
}

export interface StoredProvider {
  app: AppKind;
  provider: Provider;
  providerType: string;
  providerTypeId: string;
}

export interface ProviderDefaults {
  baseUrl: string;
  apiFormat: string;
  model: string;
  key: string;
  awsRegion?: string | null;
}

export interface ProviderMatrixEntry {
  app: AppKind;
  providerType: string;
  providerTypeId: string;
  label: string;
  defaults: ProviderDefaults;
  templateEnv: string[];
  uiVisible: boolean;
  visibility: "ui" | "diagnostic_only";
  credentialMode: string;
  accountSupported: boolean;
  directConfigSupported: boolean;
  managedAccountRecommended: boolean;
  apiKeyUrl?: string | null;
  websiteUrl?: string | null;
  note: string;
}

export interface ProviderMatrix {
  ok: boolean;
  apps: AppKind[];
  entries: ProviderMatrixEntry[];
  summary: {
    uiVisibleEntries: number;
    diagnosticOnlyEntries: number;
  };
}

export interface ProviderPresetSummary {
  name: string;
  providerType?: string | null;
  apiFormat?: string | null;
  baseUrl?: string | null;
}

export interface ProviderSortUpdate {
  id: string;
  sortIndex: number;
}

export type ProviderPresetsByApp = Record<AppKind, ProviderPresetSummary[]>;

export interface FailoverAppConfig {
  enabled: boolean;
  providerQueue: string[];
  failureThreshold: number;
  openDurationMs: number;
  halfOpenMaxProbes: number;
}

export interface ProviderBreaker {
  app: AppKind;
  providerId: string;
  state: string;
  consecutiveFailures: number;
  openedAtMs?: number | null;
  halfOpenStartedAtMs?: number | null;
  halfOpenProbeCount: number;
  lastStatusCode?: number | null;
  lastError?: string | null;
  lastFailureAtMs?: number | null;
  lastSuccessAtMs?: number | null;
}

export interface FailoverSnapshot {
  apps: Partial<Record<AppKind, FailoverAppConfig>>;
  breakers: ProviderBreaker[];
}

export interface UpdateFailoverAppInput {
  enabled?: boolean;
  providerQueue?: string[];
  failureThreshold?: number;
  openDurationMs?: number;
  halfOpenMaxProbes?: number;
}

export interface ProviderHealth {
  providerId: string;
  app: AppKind;
  requests: number;
  successes: number;
  failures: number;
  successRate?: number | null;
  avgLatencyMs?: number | null;
  lastStatusCode?: number | null;
  lastRequestAtMs?: number | null;
  healthy: boolean;
  reason?: string | null;
}

export interface AccountRecord {
  id: string;
  providerType: string;
  email?: string | null;
  accessToken?: string | null;
  refreshToken?: string | null;
  idToken?: string | null;
  tokenType?: string | null;
  apiKey?: string | null;
  scopes?: string[];
  profile?: unknown;
  raw?: unknown;
  subscriptionLevel?: string | null;
  entitlementStatus?: string | null;
  quotaPercent?: number | null;
  quota?: AccountQuota | null;
  quotaRefreshedAt?: number | null;
  quotaNextRefreshAt?: number | null;
  expiresAt?: number | null;
  lastRefreshError?: string | null;
  refreshConsecutiveFailures?: number;
  needsRelogin?: boolean;
}

export interface AccountQuota {
  success?: boolean;
  credentialMessage?: string | null;
  tiers?: AccountQuotaTier[];
  extraUsage?: unknown;
  [key: string]: unknown;
}

export interface AccountQuotaTier {
  name: string;
  utilization?: number | null;
  used?: number | null;
  limit?: number | null;
  unit?: string | null;
  resetsAt?: number | null;
}

export interface AccountManagerCapability {
  providerType: string;
  manager: string;
  support: string;
  status: string;
  blockingReason?: string | null;
  supportsStartLogin: boolean;
  supportsCallback: boolean;
  supportsRefresh: boolean;
  supportsQuota: boolean;
  supportsRefreshPlan: boolean;
  supportsImport: boolean;
  supportsDelete: boolean;
  serverNativeStage?: string | null;
  profileStrategy?: string | null;
  quotaStrategy?: string | null;
}

export interface AccountImportTemplate {
  providerType: string;
  credentialKind: string;
  requiredFields: string[];
  optionalFields: string[];
  profileHints: string[];
  rawHints: string[];
  notes: string;
}

export interface UpsertAccountInput {
  id?: string;
  providerType: string;
  email?: string;
  accessToken?: string;
  refreshToken?: string;
  idToken?: string;
  tokenType?: string;
  apiKey?: string;
  scopes?: string[];
  profile?: unknown;
  raw?: unknown;
  subscriptionLevel?: string;
  quotaPercent?: number;
  quota?: AccountQuota;
  quotaRefreshedAt?: number;
  quotaNextRefreshAt?: number;
  expiresAt?: number;
  lastRefreshError?: string;
}

export interface OAuthHttpRequest {
  method: string;
  url: string;
  headers: Array<[string, string]>;
  body: unknown;
  bodyFormat: string;
}

export interface OAuthLoginStart {
  providerType: string;
  method: string;
  sessionId: string;
  state: string;
  authorizeUrl: string;
  redirectUri?: string | null;
  codeChallenge: string;
  codeChallengeMethod: string;
  flow: string;
  status: string;
  serverNativeStage: string;
  expiresAtMs: number;
  tokenExchangeEnabled: boolean;
  message: string;
}

export interface OAuthLoginFinish {
  providerType: string;
  method: string;
  sessionId: string;
  state: string;
  flow: string;
  status: string;
  tokenExchangeEnabled: boolean;
  tokenRequest?: OAuthHttpRequest | null;
  accountImportHint?: unknown;
  message: string;
}

export interface AccountLoginAccountSummary {
  id: string;
  providerType: string;
  email?: string | null;
  subscriptionLevel?: string | null;
  entitlementStatus?: string | null;
  expiresAt?: number | null;
  hasAccessToken: boolean;
  hasRefreshToken: boolean;
  scopes: string[];
}

export interface AccountRefreshPlanResponse {
  ok: boolean;
  accountId: string;
  providerType: string;
  refreshRequired: boolean;
  serverNativeStage?: string | null;
  quotaStrategy?: string | null;
  refreshRequest?: OAuthHttpRequest | null;
  profileRequest?: OAuthHttpRequest | null;
  message: string;
}

export interface AccountQuotaResponse {
  ok: boolean;
  quota?: AccountQuota | null;
  account?: AccountRecord | null;
  refreshed: boolean;
  message?: string | null;
  nextRefreshAt?: number | null;
}

export interface AccountDeviceCodeResponse {
  deviceCode: string;
  userCode: string;
  verificationUri: string;
  verificationUriComplete?: string | null;
  expiresIn: number;
  interval: number;
  githubDomain?: string;
  region?: string;
  startUrl?: string;
}

export interface AccountDevicePollResponse {
  ok: boolean;
  pending: boolean;
  message: string;
  retryAfterSecs?: number | null;
  account?: AccountLoginAccountSummary | null;
}

export interface ShareAcl {
  sharedWithEmails: string[];
  publicMarketEmail?: string | null;
  marketAccessMode?: string | null;
}

export interface ShareBinding {
  app: AppKind;
  providerId: string;
  providerType: string;
}

export interface ShareMarketGrantStatus {
  status: string;
  grantId?: string | null;
  buyerEmail?: string | null;
  marketEmail?: string | null;
  message?: string | null;
  updatedAtMs?: number | null;
  [key: string]: unknown;
}

export interface ShareRecord {
  id: string;
  ownerEmail?: string | null;
  app: AppKind;
  providerId: string;
  providerType: string;
  displayName?: string | null;
  enabled: boolean;
  status: string;
  subscriptionLevel?: string | null;
  accountEmail?: string | null;
  quotaPercent?: number | null;
  tunnelSubdomain?: string | null;
  acl?: ShareAcl | null;
  tokenLimit?: number | null;
  parallelLimit?: number | null;
  tokensUsed: number;
  requestsCount: number;
  createdAtMs?: number | null;
  createdAt?: number | string | null;
  created_at_ms?: number | string | null;
  created_at?: number | string | null;
  expiresAt?: number | null;
  forSale: boolean;
  saleMarketKind: string;
  accessByApp?: Record<string, unknown>;
  appSettings?: Record<string, unknown>;
  forSaleOfficialPricePercentByApp?: Record<string, number>;
  officialPricePercent?: number | null;
  autoStart: boolean;
  description?: string | null;
  bindings?: ShareBinding[];
  bindingHistory?: unknown[];
  runtimeSnapshot?: unknown;
  marketGrant?: ShareMarketGrantStatus | null;
  lastError?: string | null;
  routerLastSyncedAtMs?: number | null;
  routerLastSyncError?: string | null;
  routerUrl?: string | null;
}

export interface UpsertShareInput {
  id?: string;
  ownerEmail?: string;
  app: AppKind;
  providerId: string;
  providerType: string;
  displayName?: string | null;
  enabled?: boolean;
  status?: string;
  subscriptionLevel?: string | null;
  accountEmail?: string | null;
  quotaPercent?: number | null;
  tunnelSubdomain?: string | null;
  acl?: ShareAcl;
  tokenLimit?: number;
  parallelLimit?: number;
  expiresAt?: number;
  forSale?: boolean;
  saleMarketKind?: string;
  accessByApp?: Record<string, unknown>;
  appSettings?: Record<string, unknown>;
  forSaleOfficialPricePercentByApp?: Record<string, number>;
  officialPricePercent?: number;
  autoStart?: boolean;
  description?: string | null;
  bindings?: ShareBinding[];
  runtimeSnapshot?: unknown;
  marketGrant?: ShareMarketGrantStatus | null;
}

export interface ShareConnectInfo {
  ok: boolean;
  shareId: string;
  directUrl: string;
  subdomain: string;
  routerDomain: string;
  snippets: Array<{
    app: AppKind;
    title: string;
    env: Record<string, string>;
  }>;
  note: string;
}

export interface PublicShareMarket {
  id: string;
  displayName: string;
  email: string;
  subdomain: string;
  publicBaseUrl?: string | null;
  marketKind: string;
  status: string;
  scopes?: string[];
}

export interface RouterShareEditPullResponse {
  ok: boolean;
  summary: unknown;
}

export interface UsageRollup {
  requests: number;
  successes: number;
  failures: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  totalTokens: number;
  totalCostUsd: number;
}

export interface UsageTrendPoint {
  startMs: number;
  endMs: number;
  rollup: UsageRollup;
  avgDurationMs?: number | null;
  avgFirstTokenMs?: number | null;
  lastRequestAtMs?: number | null;
}

export interface UsageLog {
  requestId: string;
  app: AppKind;
  providerId: string;
  providerName: string;
  providerType: string;
  model?: string | null;
  requestAgent?: string | null;
  sessionId?: string | null;
  requestedModel?: string | null;
  actualModel?: string | null;
  actualModelSource?: string | null;
  pricingModel?: string | null;
  costMultiplier?: number | null;
  statusCode: number;
  durationMs: number;
  firstTokenMs?: number | null;
  rawInputTokens?: number | null;
  billedInputTokens?: number | null;
  inputTokens?: number | null;
  outputTokens?: number | null;
  cacheReadTokens?: number | null;
  cacheCreationTokens?: number | null;
  totalTokens?: number | null;
  inputCostUsd?: number | null;
  outputCostUsd?: number | null;
  cacheReadCostUsd?: number | null;
  cacheCreationCostUsd?: number | null;
  totalCostUsd?: number | null;
  shareId?: string | null;
  shareName?: string | null;
  userEmail?: string | null;
  dataSource?: string | null;
  isHealthCheck: boolean;
  isStreaming: boolean;
  streamStatus?: string | null;
  userCountry?: string | null;
  userCountryIso3?: string | null;
  routerLastSyncedAtMs?: number | null;
  routerLastSyncError?: string | null;
  routerSyncAttemptCount?: number;
  createdAtMs: number;
}

export interface UsageStatsFilter {
  limit?: number;
  fromMs?: number;
  toMs?: number;
  windowMs?: number;
  app?: AppKind;
  providerId?: string;
  shareId?: string;
  userEmail?: string;
  sessionId?: string;
  dataSource?: string;
  isHealthCheck?: boolean;
  streamStatus?: string;
}

export interface ProviderUsageStats {
  app: AppKind;
  providerId: string;
  providerName: string;
  providerType: string;
  rollup: UsageRollup;
  avgDurationMs?: number | null;
  avgFirstTokenMs?: number | null;
  lastRequestAtMs?: number | null;
}

export interface ModelUsageStats {
  app: AppKind;
  model: string;
  requestedModel?: string | null;
  actualModel?: string | null;
  actualModelSource?: string | null;
  pricingModel?: string | null;
  rollup: UsageRollup;
  avgDurationMs?: number | null;
  avgFirstTokenMs?: number | null;
  lastRequestAtMs?: number | null;
}

export interface ModelPricingEntry {
  modelId: string;
  displayName: string;
  inputCostPerMillion: string;
  outputCostPerMillion: string;
  cacheReadCostPerMillion: string;
  cacheCreationCostPerMillion: string;
}

export interface UpdateModelPricingInput {
  modelId?: string;
  displayName: string;
  inputCostPerMillion: string;
  outputCostPerMillion: string;
  cacheReadCostPerMillion: string;
  cacheCreationCostPerMillion: string;
}

export interface ProviderLimitShareStatus {
  shareId: string;
  shareName: string;
  status: string;
  enabled: boolean;
  tokenLimit?: number | null;
  tokensUsed: number;
  parallelLimit?: number | null;
  expiresAt?: number | null;
  tokenExceeded: boolean;
  expired: boolean;
  blocked: boolean;
  warnings: string[];
}

export interface ProviderLimitStatus {
  app: AppKind;
  providerId: string;
  providerName: string;
  providerType: string;
  dailyUsageUsd: number;
  dailyLimitUsd?: number | null;
  dailyExceeded: boolean;
  monthlyUsageUsd: number;
  monthlyLimitUsd?: number | null;
  monthlyExceeded: boolean;
  accountId?: string | null;
  accountEmail?: string | null;
  accountQuotaPercent?: number | null;
  accountQuotaRefreshedAt?: number | null;
  accountLastRefreshError?: string | null;
  quotaDispatchLimitPercent?: number | null;
  quotaDispatchExceeded: boolean;
  shares: ProviderLimitShareStatus[];
  warnings: string[];
  blocked: boolean;
}

export interface ConfigSnapshot {
  ownerEmail?: string | null;
  routerUrl?: string | null;
  clientTunnelSubdomain?: string | null;
  upstreamProxy: UpstreamProxyView;
}

export interface UpstreamProxyView {
  enabled: boolean;
  url?: string | null;
  maskedUrl?: string | null;
  followSystemProxy: boolean;
}

export interface UpdateUpstreamProxyInput {
  url?: string;
  clear?: boolean;
  followSystemProxy?: boolean;
}

export interface RouterConfigView {
  url?: string | null;
  apiBase?: string | null;
  domain?: string | null;
  region?: string | null;
  sshHost?: string | null;
  sshUser?: string | null;
  custom: boolean;
  installationId?: string | null;
  publicKey?: string | null;
  controlSecretPresent: boolean;
  lastRegisterError?: string | null;
  lastRegisteredAtMs?: number | null;
}

export interface UpdateRouterConfigInput {
  url?: string;
  apiBase?: string;
  domain?: string;
  region?: string;
  sshHost?: string;
  sshUser?: string;
  custom?: boolean;
}

export interface TunnelRuntimeStatus {
  key: string;
  kind: string;
  status: string;
  tunnelUrl?: string | null;
  subdomain?: string | null;
  leaseId?: string | null;
  connectionId?: string | null;
  leaseExpiresAt?: string | null;
  remotePort?: number | null;
  lastError?: string | null;
  connectedAtMs?: number | null;
  updatedAtMs: number;
}

export interface ClientTunnelResponse {
  ok: boolean;
  tunnelSubdomain?: string | null;
  tunnelStatus?: string | null;
  lastHeartbeatMs?: number | null;
  runtimeStatus?: TunnelRuntimeStatus | null;
}

export interface RouterStatusResponse {
  ok: boolean;
  registered: boolean;
  lastError?: string | null;
  lastHeartbeatMs?: number | null;
  pendingRequestLogSync: number;
}

export interface ShareSyncDiagnostic {
  shareId: string;
  shareName: string;
  status: string;
  enabled: boolean;
  routerLastSyncedAtMs?: number | null;
  routerLastSyncError?: string | null;
  routerUrl?: string | null;
}

export interface RouterDiagnosticsResponse extends RouterStatusResponse {
  router: RouterConfigView;
  tunnels: TunnelRuntimeStatus[];
  shareSync: ShareSyncDiagnostic[];
}

export interface BackupFile {
  fileName: string;
  sizeBytes: number;
}

export interface BackupManifest {
  id: string;
  createdAtMs: number;
  reason?: string | null;
  files: BackupFile[];
}

export interface BackupRestoreResult {
  restored: BackupManifest;
  preRestore?: BackupManifest | null;
}

export interface EmailCodeRequestResponse {
  ok: boolean;
  cooldownSecs: number;
  maskedDestination: string;
}

export interface LoginResponse {
  ok: boolean;
  token: string;
  tokenType: string;
}

export interface SettingsPageData {
  config: ConfigSnapshot;
  router: RouterConfigView;
  tunnel: ClientTunnelResponse;
  routerStatus: RouterStatusResponse;
  diagnostics: RouterDiagnosticsResponse;
  backups: BackupManifest[];
  buildInfo: BuildInfo;
}

export interface BuildInfo {
  name: string;
  version: string;
  versionLine: string;
  commitId: string;
  commitShort: string;
  commitMessage: string;
  commitTime: string;
  buildTime: string;
  target: string;
  profile: string;
  rustcVersion: string;
  dirty: boolean;
}

export interface AdminVersionInfo extends BuildInfo {
  binaryPath: string;
  rollbackPath: string;
  rollbackAvailable: boolean;
  uptimeSecs: number;
  restartPending: boolean;
  upgradeCapable: boolean;
  service: {
    manager: "service" | "nohup";
    active: boolean;
    unitName?: string | null;
    activeState?: string | null;
    unitFileState?: string | null;
  };
  latest: {
    binaryUrl: string;
    available: boolean;
    commitId?: string | null;
    commitShort?: string | null;
    updateAvailable?: boolean;
    etag?: string | null;
    contentLength?: number | null;
    error?: string | null;
  };
}

const ADMIN_VERSION_CACHE_KEY = "cc_switch_admin_version_info_v1";

interface AdminVersionInfoCacheEntry {
  cachedAt: number;
  info: AdminVersionInfo;
}

export function readAdminVersionInfoCache(): AdminVersionInfo | null {
  if (typeof window === "undefined") {
    return null;
  }
  try {
    const raw = window.localStorage.getItem(ADMIN_VERSION_CACHE_KEY);
    if (!raw) {
      return null;
    }
    const parsed = JSON.parse(raw) as AdminVersionInfoCacheEntry;
    if (!parsed?.info?.commitId) {
      return null;
    }
    return parsed.info;
  } catch {
    return null;
  }
}

export function writeAdminVersionInfoCache(info: AdminVersionInfo): void {
  if (typeof window === "undefined") {
    return;
  }
  try {
    const entry: AdminVersionInfoCacheEntry = {
      cachedAt: Date.now(),
      info,
    };
    window.localStorage.setItem(ADMIN_VERSION_CACHE_KEY, JSON.stringify(entry));
  } catch {
    // ignore quota / private mode errors
  }
}

export function clearAdminVersionInfoCache(): void {
  if (typeof window === "undefined") {
    return;
  }
  try {
    window.localStorage.removeItem(ADMIN_VERSION_CACHE_KEY);
  } catch {
    // ignore
  }
}

export interface ProviderTestResult {
  ok: boolean;
  providerId: string;
  app: AppKind;
  providerType: string;
  adapter: string;
  support: "native" | "genericFallback" | "planned" | string;
  endpoint: string;
  model: string;
  stream: boolean;
  headerNames: string[];
  networkChecked: boolean;
  networkStatusCode?: number | null;
  networkLatencyMs?: number | null;
  networkStreamCompleted?: boolean | null;
  networkError?: string | null;
  message: string;
}

export interface FetchModelsResult {
  ok: boolean;
  providerId: string;
  app: AppKind;
  providerType: string;
  url: string;
  merged: boolean;
  mergedCount: number;
  models: Array<{ id: string; upstreamModel: string; displayName?: string | null }>;
  provider?: StoredProvider;
}

export async function loadBuildInfo(): Promise<BuildInfo> {
  return invokeCommand<BuildInfo>("get_build_info");
}

export async function loadAdminVersionInfo(): Promise<AdminVersionInfo> {
  return invokeCommand<AdminVersionInfo>("get_admin_version_info");
}

export async function restartServerService(): Promise<void> {
  await invokeCommand<{ ok: boolean }>("restart_server_service");
}

export async function rollbackServerService(): Promise<void> {
  await invokeCommand<{ ok: boolean }>("rollback_server_service");
}

export async function startServerUpgrade(input: {
  restartAfter: boolean;
}): Promise<{ taskId: string }> {
  const result = await invokeCommand<{ ok: boolean; taskId: string }>(
    "start_admin_upgrade",
    { restartAfter: input.restartAfter },
  );
  return { taskId: result.taskId };
}

export interface AdminUpgradeStatus {
  taskId: string;
  status: "running" | "success" | "failed";
  restartPending: boolean;
  targetCommitId?: string | null;
  logs: Array<{
    taskId?: string;
    step?: number;
    totalSteps?: number;
    level?: string;
    message?: string;
    progress?: number | null;
    at?: string;
  }>;
}

export async function loadUpgradeStatus(
  taskId: string,
): Promise<AdminUpgradeStatus> {
  const params = new URLSearchParams({ taskId });
  return jsonFetch<AdminUpgradeStatus>(
    `/web-api/admin/upgrade/status?${params}`,
  );
}

export async function completeServerSetup(input: {
  password: string;
  ownerEmail: string;
  routerUrl: string;
  clientTunnelSubdomain?: string;
}): Promise<void> {
  await invokeCommand("complete_server_setup", {
    password: input.password,
    ownerEmail: input.ownerEmail,
    routerUrl: input.routerUrl,
    clientTunnelSubdomain: input.clientTunnelSubdomain,
  });
}

export async function loginWithApiToken(apiToken: string): Promise<LoginResponse> {
  return invokeCommand<LoginResponse>("login_with_api_token", { apiToken });
}

export async function changeServerPassword(newPassword: string): Promise<void> {
  const currentPassword = readCachedPassword()?.trim();
  if (!currentPassword) {
    throw new Error(
      "无法验证当前密码，请先退出后使用原密码重新登录再修改。",
    );
  }
  await jsonFetch<{ ok: boolean }>("/web-api/auth/password/change", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ currentPassword, newPassword }),
  });
}

export async function requestEmailLoginCode(
  email: string,
): Promise<EmailCodeRequestResponse> {
  return invokeCommand<EmailCodeRequestResponse>(
    "request_admin_email_login_code",
    { email },
  );
}

export async function verifyEmailLoginCode(input: {
  email: string;
  code: string;
}): Promise<LoginResponse> {
  return invokeCommand<LoginResponse>("verify_admin_email_login_code", input);
}
