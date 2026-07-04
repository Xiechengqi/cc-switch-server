import { invokeCommand, jsonFetch } from "@/lib/runtime";

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
  quotaPercent?: number | null;
  quota?: AccountQuota | null;
  quotaRefreshedAt?: number | null;
  quotaNextRefreshAt?: number | null;
  expiresAt?: number | null;
  lastRefreshError?: string | null;
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

export interface RouterBatchSyncResponse {
  ok: boolean;
  synced: number;
  remoteSynced: boolean;
  message: string;
  shares: ShareRecord[];
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

export interface UniversalProviderApps {
  claude: boolean;
  codex: boolean;
  gemini: boolean;
}

export interface ClaudeUniversalModelConfig {
  model?: string;
  haikuModel?: string;
  sonnetModel?: string;
  opusModel?: string;
  modelCatalog?: unknown;
  modelMapping?: unknown;
}

export interface CodexUniversalModelConfig {
  model?: string;
  reasoningEffort?: string;
  chatReasoning?: unknown;
  modelCatalog?: unknown;
  modelMapping?: unknown;
}

export interface GeminiUniversalModelConfig {
  model?: string;
  modelCatalog?: unknown;
  modelMapping?: unknown;
}

export interface UniversalProviderModels {
  claude?: ClaudeUniversalModelConfig;
  codex?: CodexUniversalModelConfig;
  gemini?: GeminiUniversalModelConfig;
}

export interface UniversalProvider {
  id: string;
  name: string;
  providerType: string;
  apps: UniversalProviderApps;
  baseUrl: string;
  apiKey: string;
  models?: UniversalProviderModels;
  websiteUrl?: string | null;
  notes?: string | null;
  icon?: string | null;
  iconColor?: string | null;
  meta?: ProviderMeta | null;
  createdAt?: number | null;
  sortIndex?: number | null;
  [key: string]: unknown;
}

export interface UniversalProviderSyncResult {
  synced: string[];
  skipped: string[];
  removed: string[];
}

export interface UniversalProviderPreset {
  name: string;
  providerType: string;
  defaultApps: UniversalProviderApps;
  defaultModels: UniversalProviderModels;
  websiteUrl?: string | null;
  icon?: string | null;
  iconColor?: string | null;
  description?: string | null;
  isCustomTemplate?: boolean;
}

export interface SettingsDashboardData {
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

export async function loadProviderDashboardData(): Promise<{
  providers: StoredProvider[];
  matrix: ProviderMatrix;
  health: ProviderHealth[];
  accounts: AccountRecord[];
  capabilities: AccountManagerCapability[];
  limits: ProviderLimitStatus[];
  presets: ProviderPresetsByApp;
}> {
  const [providers, matrix, health, accounts, capabilities, limits, presets] = await Promise.all([
    jsonFetch<{ providers: StoredProvider[] }>("/api/providers"),
    jsonFetch<ProviderMatrix>("/api/provider-matrix"),
    jsonFetch<{ providers: ProviderHealth[] }>("/api/providers/health"),
    jsonFetch<{ accounts: AccountRecord[] }>("/api/accounts"),
    jsonFetch<{ capabilities: AccountManagerCapability[] }>("/api/accounts/capabilities"),
    jsonFetch<{ limits: ProviderLimitStatus[] }>("/api/provider-limits"),
    loadProviderPresetsByApp(),
  ]);
  return {
    providers: providers.providers || [],
    matrix,
    health: health.providers || [],
    accounts: accounts.accounts || [],
    capabilities: capabilities.capabilities || [],
    limits: limits.limits || [],
    presets,
  };
}

export async function loadProviderPresets(app: AppKind): Promise<ProviderPresetSummary[]> {
  const params = new URLSearchParams({ app });
  const result = await jsonFetch<{ presets: ProviderPresetSummary[] }>(
    `/api/provider-presets?${params}`,
  );
  return result.presets || [];
}

export async function loadProviderPresetsByApp(): Promise<ProviderPresetsByApp> {
  const [claude, codex, gemini] = await Promise.all([
    loadProviderPresets("claude"),
    loadProviderPresets("codex"),
    loadProviderPresets("gemini"),
  ]);
  return { claude, codex, gemini };
}

export async function createProviderFromPreset(
  app: AppKind,
  name: string,
): Promise<StoredProvider> {
  const result = await jsonFetch<{ stored: StoredProvider }>("/api/providers/from-preset", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ app, name }),
  });
  return result.stored;
}

export async function saveProvider(app: AppKind, provider: Provider): Promise<StoredProvider> {
  const result = await jsonFetch<{ stored: StoredProvider }>("/api/providers", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ app, provider }),
  });
  return result.stored;
}

export async function exportProviders(): Promise<StoredProvider[]> {
  const result = await jsonFetch<{ providers: StoredProvider[] }>("/api/providers/export");
  return result.providers || [];
}

export async function importProviders(providers: StoredProvider[]): Promise<number> {
  const result = await jsonFetch<{ imported: number }>("/api/providers/import", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      providers: providers.map((item) => ({
        app: item.app,
        provider: item.provider,
      })),
    }),
  });
  return result.imported;
}

export async function deleteProvider(app: AppKind, id: string): Promise<boolean> {
  return invokeCommand<boolean>("delete_provider", { app, id });
}

export async function switchProvider(app: AppKind, id: string): Promise<void> {
  await invokeCommand("switch_provider", { app, id });
}

export async function updateProvidersSortOrder(
  app: AppKind,
  updates: ProviderSortUpdate[],
): Promise<boolean> {
  return invokeCommand<boolean>("update_providers_sort_order", { app, updates });
}

export async function getCurrentProvider(app: AppKind): Promise<string> {
  return invokeCommand<string>("get_current_provider", { app });
}

export async function testProvider(
  app: AppKind,
  id: string,
  options: { network?: boolean; stream?: boolean; model?: string } = {},
): Promise<ProviderTestResult> {
  const params = new URLSearchParams({ app });
  if (options.network) params.set("network", "true");
  if (options.stream) params.set("stream", "true");
  if (options.model) params.set("model", options.model);
  return jsonFetch<ProviderTestResult>(`/api/providers/${encodeURIComponent(id)}/test?${params}`, {
    method: "POST",
  });
}

export async function fetchProviderModels(
  app: AppKind,
  id: string,
  merge: boolean,
): Promise<FetchModelsResult> {
  return jsonFetch<FetchModelsResult>(`/api/providers/${encodeURIComponent(id)}/fetch-models`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ app, merge }),
  });
}

export async function loadAccountsDashboardData(): Promise<{
  accounts: AccountRecord[];
  capabilities: AccountManagerCapability[];
  templates: AccountImportTemplate[];
}> {
  const [accounts, capabilities, templates] = await Promise.all([
    jsonFetch<{ accounts: AccountRecord[] }>("/api/accounts"),
    jsonFetch<{ capabilities: AccountManagerCapability[] }>("/api/accounts/capabilities"),
    jsonFetch<{ templates: AccountImportTemplate[] }>("/api/accounts/import-templates"),
  ]);
  return {
    accounts: accounts.accounts || [],
    capabilities: capabilities.capabilities || [],
    templates: templates.templates || [],
  };
}

export async function upsertAccount(input: UpsertAccountInput): Promise<AccountRecord> {
  const result = await jsonFetch<{ account: AccountRecord }>("/api/accounts", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  return result.account;
}

export async function deleteAccount(id: string): Promise<boolean> {
  const result = await jsonFetch<{ deleted: boolean }>(`/api/accounts/${encodeURIComponent(id)}`, {
    method: "DELETE",
  });
  return result.deleted;
}

export async function refreshAccount(id: string): Promise<AccountRecord> {
  const result = await jsonFetch<{ account: AccountRecord }>(
    `/api/accounts/${encodeURIComponent(id)}/refresh`,
    { method: "POST" },
  );
  return result.account;
}

export async function loadAccountQuota(
  id: string,
  options: { refresh?: boolean; force?: boolean } = {},
): Promise<AccountQuotaResponse> {
  const params = new URLSearchParams();
  if (options.refresh) params.set("refresh", "true");
  if (options.force) params.set("force", "true");
  const suffix = params.toString() ? `?${params}` : "";
  return jsonFetch<AccountQuotaResponse>(`/api/accounts/${encodeURIComponent(id)}/quota${suffix}`);
}

export async function loadAccountRefreshPlan(id: string): Promise<AccountRefreshPlanResponse> {
  return jsonFetch<AccountRefreshPlanResponse>(
    `/api/accounts/${encodeURIComponent(id)}/refresh-plan`,
  );
}

export async function startAccountLogin(input: {
  providerType: string;
  redirectUri?: string;
}): Promise<OAuthLoginStart> {
  const result = await jsonFetch<{ login: OAuthLoginStart }>("/api/accounts/login/start", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  return result.login;
}

export async function finishAccountLogin(input: {
  sessionId?: string;
  state?: string;
  code?: string;
  executeTokenExchange?: boolean;
}): Promise<{ login: OAuthLoginFinish; account?: AccountLoginAccountSummary | null }> {
  const result = await jsonFetch<{
    login: OAuthLoginFinish;
    account?: AccountLoginAccountSummary | null;
  }>("/api/accounts/login/finish", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  return { login: result.login, account: result.account };
}

export async function startCopilotDeviceLogin(input: {
  githubDomain?: string;
}): Promise<AccountDeviceCodeResponse> {
  const result = await jsonFetch<{ device: AccountDeviceCodeResponse }>(
    "/api/accounts/copilot/device/start",
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(input),
    },
  );
  return result.device;
}

export async function pollCopilotDeviceLogin(input: {
  deviceCode: string;
  githubDomain?: string;
}): Promise<AccountDevicePollResponse> {
  return jsonFetch<AccountDevicePollResponse>("/api/accounts/copilot/device/poll", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
}

export async function startKiroDeviceLogin(input: {
  region?: string;
  startUrl?: string;
}): Promise<AccountDeviceCodeResponse> {
  const result = await jsonFetch<{ device: AccountDeviceCodeResponse }>(
    "/api/accounts/kiro/device/start",
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(input),
    },
  );
  return result.device;
}

export async function pollKiroDeviceLogin(input: {
  deviceCode: string;
}): Promise<AccountDevicePollResponse> {
  return jsonFetch<AccountDevicePollResponse>("/api/accounts/kiro/device/poll", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
}

export async function loadShareDashboardData(): Promise<{
  shares: ShareRecord[];
  providers: StoredProvider[];
  requestLogs: UsageLog[];
}> {
  const [shares, providers, logs] = await Promise.all([
    jsonFetch<{ shares: ShareRecord[] }>("/api/shares"),
    jsonFetch<{ providers: StoredProvider[] }>("/api/providers"),
    loadShareRequestLogs(),
  ]);
  return {
    shares: shares.shares || [],
    providers: providers.providers || [],
    requestLogs: logs,
  };
}

export async function loadShareRequestLogs(limit = 80): Promise<UsageLog[]> {
  const result = await jsonFetch<{ logs: UsageLog[] }>(
    `/api/usage/logs${usageQuery({ limit }, false)}`,
  );
  return result.logs || [];
}

export async function saveShare(input: UpsertShareInput): Promise<ShareRecord> {
  const result = await jsonFetch<{ share: ShareRecord }>("/api/shares", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  return result.share;
}

export async function exportShares(): Promise<ShareRecord[]> {
  const result = await jsonFetch<{ shares: ShareRecord[] }>("/api/shares/export");
  return result.shares || [];
}

export async function importShares(shares: ShareRecord[]): Promise<number> {
  const result = await jsonFetch<{ imported: number }>("/api/shares/import", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ shares }),
  });
  return result.imported;
}

export async function deleteShare(id: string): Promise<boolean> {
  const result = await jsonFetch<{ deleted: boolean }>(`/api/shares/${encodeURIComponent(id)}`, {
    method: "DELETE",
  });
  return result.deleted;
}

export async function pauseShare(id: string): Promise<ShareRecord> {
  return sharePost(id, "pause");
}

export async function resumeShare(id: string): Promise<ShareRecord> {
  return sharePost(id, "resume");
}

export async function startShareTunnel(id: string): Promise<ShareRecord> {
  return sharePost(id, "tunnel/start");
}

export async function stopShareTunnel(id: string): Promise<ShareRecord> {
  return sharePost(id, "tunnel/stop");
}

export async function resetShareUsage(id: string): Promise<ShareRecord> {
  return sharePost(id, "reset-usage");
}

export async function restoreShareTunnels(): Promise<ShareRecord[]> {
  const result = await jsonFetch<{ shares: ShareRecord[] }>("/api/shares/tunnels/restore", {
    method: "POST",
  });
  return result.shares || [];
}

export async function refreshShareRuntimeSnapshots(): Promise<ShareRecord[]> {
  const result = await jsonFetch<{ shares: ShareRecord[] }>("/api/shares/runtime-snapshot", {
    method: "POST",
  });
  return result.shares || [];
}

export async function updateShareBinding(id: string, binding: ShareBinding): Promise<ShareRecord> {
  const result = await jsonFetch<{ share: ShareRecord }>(`/api/shares/${encodeURIComponent(id)}/binding`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ binding }),
  });
  return result.share;
}

export async function replaceShareAcl(id: string, acl: ShareAcl): Promise<ShareRecord> {
  const result = await jsonFetch<{ share: ShareRecord }>(`/api/shares/${encodeURIComponent(id)}/acl`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ acl }),
  });
  return result.share;
}

export async function updateShareSubdomain(id: string, subdomain: string): Promise<{
  share: ShareRecord;
  remoteClaimed: boolean;
}> {
  const result = await jsonFetch<{ share: ShareRecord; remoteClaimed: boolean }>(
    `/api/shares/${encodeURIComponent(id)}/subdomain`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ subdomain }),
    },
  );
  return { share: result.share, remoteClaimed: result.remoteClaimed };
}

export async function requestShareOwnerChangeCode(
  id: string,
  newOwnerEmail: string,
): Promise<EmailCodeRequestResponse> {
  return jsonFetch<EmailCodeRequestResponse>(
    `/api/shares/${encodeURIComponent(id)}/owner/request-code`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ newOwnerEmail }),
    },
  );
}

export async function verifyShareOwnerChangeCode(input: {
  id: string;
  newOwnerEmail: string;
  code: string;
}): Promise<ShareRecord> {
  const result = await jsonFetch<{ share: ShareRecord }>(
    `/api/shares/${encodeURIComponent(input.id)}/owner/verify-code`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ newOwnerEmail: input.newOwnerEmail, code: input.code }),
    },
  );
  return result.share;
}

export async function loadShareConnectInfo(id: string): Promise<ShareConnectInfo> {
  return jsonFetch<ShareConnectInfo>(`/api/shares/${encodeURIComponent(id)}/connect-info`);
}

export async function loadShareMarkets(): Promise<PublicShareMarket[]> {
  const result = await jsonFetch<{ markets: PublicShareMarket[] }>("/api/share-markets");
  return result.markets || [];
}

export async function authorizeShareMarket(id: string, marketEmail: string): Promise<ShareRecord> {
  const result = await jsonFetch<{ share: ShareRecord }>(
    `/api/shares/${encodeURIComponent(id)}/authorize-market`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ marketEmail }),
    },
  );
  return result.share;
}

export async function updateShareMarketGrant(
  id: string,
  marketGrant: ShareMarketGrantStatus | null,
): Promise<ShareRecord> {
  const result = await jsonFetch<{ share: ShareRecord }>(
    `/api/shares/${encodeURIComponent(id)}/market-grant`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ marketGrant }),
    },
  );
  return result.share;
}

export async function pullRouterShareEdits(): Promise<RouterShareEditPullResponse> {
  return jsonFetch<RouterShareEditPullResponse>("/api/router/share-edits/pull", {
    method: "POST",
  });
}

export async function loadUsageDashboardData(filter: UsageStatsFilter): Promise<{
  summary: UsageRollup;
  trends: UsageTrendPoint[];
  providers: ProviderUsageStats[];
  models: ModelUsageStats[];
  logs: UsageLog[];
  sourceLogs: UsageLog[];
  pricing: ModelPricingEntry[];
  limits: ProviderLimitStatus[];
}> {
  const statsQuery = usageQuery(filter, true);
  const logsQuery = usageQuery(filter, false);
  const sourceLogsQuery = usageQuery(
    {
      ...filter,
      dataSource: undefined,
      limit: Math.max(filter.limit ?? 100, 1000),
    },
    false,
  );
  const [summary, trends, providers, models, logs, sourceLogs, pricing, limits] = await Promise.all([
    jsonFetch<{ summary: UsageRollup }>(`/api/usage/summary${statsQuery}`),
    jsonFetch<{ trends: UsageTrendPoint[] }>(`/api/usage/trends${statsQuery}`),
    jsonFetch<{ providers: ProviderUsageStats[] }>(`/api/usage/provider-stats${statsQuery}`),
    jsonFetch<{ models: ModelUsageStats[] }>(`/api/usage/model-stats${statsQuery}`),
    jsonFetch<{ logs: UsageLog[] }>(`/api/usage/logs${logsQuery}`),
    jsonFetch<{ logs: UsageLog[] }>(`/api/usage/logs${sourceLogsQuery}`),
    jsonFetch<{ models: ModelPricingEntry[] }>("/api/pricing/models"),
    jsonFetch<{ limits: ProviderLimitStatus[] }>("/api/provider-limits"),
  ]);
  return {
    summary: summary.summary || emptyUsageRollup(),
    trends: trends.trends || [],
    providers: providers.providers || [],
    models: models.models || [],
    logs: logs.logs || [],
    sourceLogs: sourceLogs.logs || [],
    pricing: pricing.models || [],
    limits: limits.limits || [],
  };
}

export async function loadUsageLogDetail(requestId: string): Promise<UsageLog> {
  const result = await jsonFetch<{ log: UsageLog }>(
    `/api/usage/logs/${encodeURIComponent(requestId)}`,
  );
  return result.log;
}

export async function backfillUsageCosts(): Promise<number> {
  const result = await jsonFetch<{ updated: number }>("/api/usage/backfill-costs", {
    method: "POST",
  });
  return result.updated;
}

export async function saveModelPricing(input: UpdateModelPricingInput): Promise<{
  model: ModelPricingEntry;
  backfilled: number;
}> {
  const result = await jsonFetch<{ model: ModelPricingEntry; backfilled: number }>(
    "/api/pricing/models",
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(input),
    },
  );
  return { model: result.model, backfilled: result.backfilled };
}

export async function deleteModelPricing(modelId: string): Promise<boolean> {
  const result = await jsonFetch<{ deleted: boolean }>(
    `/api/pricing/models/${encodeURIComponent(modelId)}`,
    { method: "DELETE" },
  );
  return result.deleted;
}

export async function loadSettingsDashboardData(): Promise<SettingsDashboardData> {
  const [config, router, tunnel, routerStatus, diagnostics, backups, buildInfo] = await Promise.all([
    jsonFetch<ConfigSnapshot>("/api/config"),
    jsonFetch<{ router: RouterConfigView }>("/api/router/config"),
    jsonFetch<ClientTunnelResponse>("/api/router/client-tunnel"),
    jsonFetch<RouterStatusResponse>("/api/router/status"),
    jsonFetch<RouterDiagnosticsResponse>("/api/router/diagnostics"),
    jsonFetch<{ backups: BackupManifest[] }>("/api/backups"),
    loadBuildInfo(),
  ]);
  return {
    config,
    router: router.router,
    tunnel,
    routerStatus,
    diagnostics,
    backups: backups.backups || [],
    buildInfo,
  };
}

export async function loadBuildInfo(): Promise<BuildInfo> {
  return jsonFetch<BuildInfo>("/version");
}

export async function updateUpstreamProxy(
  input: UpdateUpstreamProxyInput,
): Promise<UpstreamProxyView> {
  const result = await jsonFetch<{ upstreamProxy: UpstreamProxyView }>("/api/upstream-proxy", {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  return result.upstreamProxy;
}

export async function updateRouterConfig(input: UpdateRouterConfigInput): Promise<RouterConfigView> {
  const result = await jsonFetch<{ router: RouterConfigView }>("/api/router/config", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  return result.router;
}

export async function updateClientTunnel(input: {
  tunnelSubdomain?: string;
  tunnelStatus?: string;
}): Promise<ClientTunnelResponse> {
  return jsonFetch<ClientTunnelResponse>("/api/router/client-tunnel", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
}

export async function claimClientTunnel(): Promise<{ status: string; error?: string | null }> {
  return jsonFetch<{ status: string; error?: string | null }>("/api/router/client-tunnel/claim", {
    method: "POST",
  });
}

export async function startClientTunnel(): Promise<{ status?: TunnelRuntimeStatus | null; message: string }> {
  return jsonFetch<{ status?: TunnelRuntimeStatus | null; message: string }>(
    "/api/router/client-tunnel/lease",
    { method: "POST" },
  );
}

export async function stopClientTunnel(): Promise<ClientTunnelResponse> {
  return jsonFetch<ClientTunnelResponse>("/api/router/client-tunnel/stop", { method: "POST" });
}

export async function registerRouter(): Promise<unknown> {
  const result = await jsonFetch<{ registration: unknown }>("/api/router/register", {
    method: "POST",
  });
  return result.registration;
}

export async function heartbeatRouter(): Promise<RouterStatusResponse> {
  return jsonFetch<RouterStatusResponse>("/api/router/heartbeat", { method: "POST" });
}

export async function batchSyncRouterShares(): Promise<RouterBatchSyncResponse> {
  return jsonFetch<RouterBatchSyncResponse>("/api/router/batch-sync", { method: "POST" });
}

export async function createBackup(reason?: string): Promise<BackupManifest> {
  const result = await jsonFetch<{ backup: BackupManifest }>("/api/backups", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ reason }),
  });
  return result.backup;
}

export async function restoreBackup(id: string): Promise<BackupRestoreResult> {
  const result = await jsonFetch<{ result: BackupRestoreResult }>(
    `/api/backups/${encodeURIComponent(id)}/restore`,
    { method: "POST" },
  );
  return result.result;
}

export async function rotateApiToken(): Promise<string> {
  const result = await jsonFetch<{ apiToken: string }>("/api/auth/api-token", {
    method: "POST",
  });
  return result.apiToken;
}

export async function requestEmailLoginCode(email: string): Promise<EmailCodeRequestResponse> {
  return jsonFetch<EmailCodeRequestResponse>("/api/auth/email/request-code", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ email }),
  });
}

export async function verifyEmailLoginCode(input: {
  email: string;
  code: string;
}): Promise<LoginResponse> {
  return jsonFetch<LoginResponse>("/api/auth/email/verify-code", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
}

export async function loadUniversalProviders(): Promise<Record<string, UniversalProvider>> {
  const result = await jsonFetch<{ providers: Record<string, UniversalProvider> }>(
    "/api/universal-providers",
  );
  return result.providers || {};
}

export async function loadUniversalProviderPresets(): Promise<UniversalProviderPreset[]> {
  const result = await jsonFetch<{ presets: UniversalProviderPreset[] }>(
    "/api/universal-provider-presets",
  );
  return result.presets || [];
}

export async function saveUniversalProvider(provider: UniversalProvider): Promise<UniversalProvider> {
  const result = await jsonFetch<{ provider: UniversalProvider }>("/api/universal-providers", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ provider }),
  });
  return result.provider;
}

export async function deleteUniversalProvider(id: string): Promise<boolean> {
  const result = await jsonFetch<{ deleted: boolean }>(
    `/api/universal-providers/${encodeURIComponent(id)}`,
    { method: "DELETE" },
  );
  return result.deleted;
}

export async function syncUniversalProvider(id: string): Promise<UniversalProviderSyncResult> {
  const result = await jsonFetch<{ result: UniversalProviderSyncResult }>(
    `/api/universal-providers/${encodeURIComponent(id)}/sync`,
    { method: "POST" },
  );
  return result.result;
}

export async function exportUniversalProviders(): Promise<UniversalProvider[]> {
  const result = await jsonFetch<{ providers: UniversalProvider[] }>(
    "/api/universal-providers/export",
  );
  return result.providers || [];
}

export async function importUniversalProviders(providers: UniversalProvider[]): Promise<number> {
  const result = await jsonFetch<{ imported: number }>("/api/universal-providers/import", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ providers }),
  });
  return result.imported;
}

async function sharePost(id: string, action: string): Promise<ShareRecord> {
  const result = await jsonFetch<{ share: ShareRecord }>(
    `/api/shares/${encodeURIComponent(id)}/${action}`,
    { method: "POST" },
  );
  return result.share;
}

function usageQuery(filter: UsageStatsFilter, includeWindow: boolean): string {
  const params = new URLSearchParams();
  appendParam(params, "limit", filter.limit);
  appendParam(params, "fromMs", filter.fromMs);
  appendParam(params, "toMs", filter.toMs);
  if (includeWindow) appendParam(params, "windowMs", filter.windowMs);
  appendParam(params, "app", filter.app);
  appendParam(params, "providerId", filter.providerId);
  appendParam(params, "shareId", filter.shareId);
  appendParam(params, "userEmail", filter.userEmail);
  appendParam(params, "sessionId", filter.sessionId);
  appendParam(params, "dataSource", filter.dataSource);
  appendParam(params, "isHealthCheck", filter.isHealthCheck);
  appendParam(params, "streamStatus", filter.streamStatus);
  const query = params.toString();
  return query ? `?${query}` : "";
}

function appendParam(params: URLSearchParams, key: string, value: unknown) {
  if (value == null || value === "") return;
  params.set(key, String(value));
}

function emptyUsageRollup(): UsageRollup {
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
