import { invokeCommand } from "@/lib/runtime";
import {
  normalizeShareRecord,
  normalizeShareRecords,
} from "@/utils/shareRecordNormalize";

/** Wire representation of one provider binding per supported app. */
export type ShareBindings = Partial<
  Record<"claude" | "codex" | "gemini", string>
>;

export type ShareAppAccess = {
  sharedWithEmails: string[];
  marketAccessMode: "selected" | "all";
};

export type ShareAccessByApp = Partial<
  Record<keyof ShareBindings, ShareAppAccess>
>;

export type ShareTokenPeriod =
  | "lifetime"
  | "day"
  | "week"
  | "sevenDays"
  | "calendarMonth"
  | "thirtyDays";

export type ShareUserPolicy = {
  parallelLimit?: number;
  tokenLimit?: number;
  tokenPeriod: ShareTokenPeriod;
  tokenPeriodAnchorAtMs?: number;
  expiresAt?: number;
};

export type ShareUserUsageBucket = {
  startedAtMs: number;
  tokensUsed: number;
  requestsCount: number;
};

export type ShareAnchoredUsageBucket = ShareUserUsageBucket & {
  period: Extract<ShareTokenPeriod, "sevenDays" | "thirtyDays">;
  anchorAtMs: number;
};

export type ShareUserUsage = {
  lifetime: ShareUserUsageBucket;
  day: ShareUserUsageBucket;
  week: ShareUserUsageBucket;
  calendarMonth: ShareUserUsageBucket;
  anchored?: ShareAnchoredUsageBucket;
};

export type ShareUserGrant = {
  email: string;
  role: "owner" | "shareto";
  active: boolean;
  policy: ShareUserPolicy;
  usage?: ShareUserUsage;
  createdAtMs?: number;
  updatedAtMs?: number;
  revokedAtMs?: number;
  revision?: number;
  manager?: "owner" | "manual" | "routerShareMarket";
  entitlementId?: string;
};

export type ShareUserGrantMap = Record<string, ShareUserGrant>;

export type ShareAppSettings = {
  forSale: "Yes" | "No" | "Free";
  marketAccessMode: "selected" | "all";
  sharedWithEmails: string[];
  tokenLimit: number;
  parallelLimit: number;
  expiresAt: string;
};

export type ShareAppSettingsByApp = Partial<
  Record<keyof ShareBindings, ShareAppSettings>
>;

export interface ShareRecord {
  id: string;
  capacityPoolId: string;
  name: string;
  ownerEmail: string;
  sharedWithEmails: string[];
  marketAccessMode: "selected" | "all";
  accessByApp?: ShareAccessByApp;
  appSettings?: ShareAppSettingsByApp;
  forSaleOfficialPricePercentByApp: Record<string, number>;
  description?: string | null;
  forSale: "Yes" | "No" | "Free";
  /** One to three entries, with at most one provider per app. */
  bindings: ShareBindings;
  apiKey: string;
  settingsConfig?: string | null;
  tokenLimit: number;
  parallelLimit: number;
  tokensUsed: number;
  requestsCount: number;
  expiresAt: string;
  shareSlug?: string | null;
  subdomain?: string | null;
  tunnelUrl?: string | null;
  status: string;
  autoStart: boolean;
  createdAt: string;
  lastUsedAt?: string | null;
  configRevision: number;
  routerSyncedRevision: number;
  descriptorGeneration: number;
  descriptorFingerprint?: string | null;
  routerSyncedDescriptorGeneration: number;
  routerSyncedDescriptorFingerprint?: string | null;
  routerLastSyncError?: string | null;
  userGrants: ShareUserGrantMap;
}

export interface CreateShareParams {
  /** New Shares start with one binding; more are attached after explicit reuse confirmation. */
  bindings: ShareBindings;
  description?: string;
  forSale: "Yes" | "No" | "Free";
  tokenLimit: number;
  parallelLimit: number;
  expiresInSecs: number;
  subdomain?: string;
  sharedWithEmails?: string[];
  marketAccessMode?: "selected" | "all";
  accessByApp?: ShareAccessByApp;
  appSettings?: ShareAppSettingsByApp;
  userGrants?: ShareUserGrantMap;
}

export interface ShareReuseCandidate {
  shareId: string;
  shareName: string;
  subdomain?: string | null;
  apps: Array<keyof ShareBindings>;
  configRevision: number;
}

export interface ShareBindingMutationParams {
  shareId: string;
  app: keyof ShareBindings;
  providerId: string;
  expectedConfigRevision: number;
}

export interface RemoveShareBindingResult {
  ok: boolean;
  deletedShare: boolean;
  share?: ShareRecord | null;
}

export const SHARE_APP_TYPES: ReadonlyArray<keyof ShareBindings> = [
  "claude",
  "codex",
  "gemini",
];

/** Return every app bound to this Share. */
export function shareSupportedApps(
  share: Pick<ShareRecord, "bindings"> | null | undefined,
): Array<keyof ShareBindings> {
  if (!share) return [];
  return SHARE_APP_TYPES.filter((app) => {
    const pid = share.bindings?.[app];
    return typeof pid === "string" && pid.length > 0;
  });
}

/**
 * The first app in stable protocol order, used only for compact UI fallbacks.
 */
export function sharePrimaryApp(
  share: Pick<ShareRecord, "bindings"> | null | undefined,
): keyof ShareBindings | null {
  return shareSupportedApps(share)[0] ?? null;
}

/** 主 app 的 provider id（与 sharePrimaryApp 对应）。 */
export function sharePrimaryProviderId(
  share: Pick<ShareRecord, "bindings"> | null | undefined,
): string | null {
  const app = sharePrimaryApp(share);
  return app ? (share?.bindings?.[app] ?? null) : null;
}

export interface PublicTokenMarket {
  id: string;
  displayName: string;
  email: string;
  subdomain: string;
  publicBaseUrl: string;
  marketKind: "usage";
  status: string;
}

export interface UpdateShareAclParams {
  shareId: string;
  sharedWithEmails: string[];
  marketAccessMode: "selected" | "all";
  accessByApp?: ShareAccessByApp;
  appSettings?: ShareAppSettingsByApp;
  userGrants?: ShareUserGrantMap;
}

/** Complete settings payload saved atomically from a Provider edit page. */
export interface SaveProviderShareParams {
  shareId: string;
  expectedConfigRevision: number;
  subdomain: string;
  description?: string;
  forSale: "Yes" | "No" | "Free";
  marketAccessMode: "selected" | "all";
  sharedWithEmails: string[];
  accessByApp: ShareAccessByApp;
  appSettings: ShareAppSettingsByApp;
  forSaleOfficialPricePercentByApp: Record<string, number>;
  tokenLimit: number;
  parallelLimit: number;
  expiresAt: string;
  userGrants: ShareUserGrantMap;
}

export interface UpdateShareTokenLimitParams {
  shareId: string;
  tokenLimit: number;
}

export interface UpdateShareParallelLimitParams {
  shareId: string;
  parallelLimit: number;
}

export interface UpdateShareSubdomainParams {
  shareId: string;
  subdomain: string;
}

export interface UpdateShareDescriptionParams {
  shareId: string;
  description?: string;
}

export interface UpdateShareForSaleParams {
  shareId: string;
  forSale: "Yes" | "No" | "Free";
}

export interface UpdateShareForSaleOfficialPricePercentParams {
  shareId: string;
  pricing: Record<string, number>;
}

export interface UpdateShareExpirationParams {
  shareId: string;
  expiresAt: string;
}

export interface TunnelInfo {
  tunnelUrl: string;
  subdomain: string;
  remotePort: number;
  healthy: boolean;
  status?: string;
  kind?: string;
  generation?: number;
  desiredGeneration?: number;
  transportState?: string | null;
  startReason?: string | null;
}

export interface ShareTunnelStatus {
  info?: TunnelInfo | null;
  lastError?: string | null;
  requiresOwnerLogin: boolean;
}

export interface TunnelConfig {
  domain: string;
}

export interface ConnectInfo {
  tunnelUrl: string;
  subdomain: string;
}

export interface ClientTunnelConfig {
  ownerEmail: string;
  subdomain: string;
  enabled: boolean;
  autoStart: boolean;
  tunnelUrl?: string | null;
  expectedUrl?: string | null;
}

export interface ClientTunnelState {
  config: ClientTunnelConfig;
  status: ShareTunnelStatus;
}

export interface ClientTunnelUpdateParams {
  subdomain: string;
  enabled: boolean;
  autoStart: boolean;
}

export type ShareHealthLevel = "healthy" | "warning" | "unhealthy";

export interface ShareHealthLink {
  status: ShareHealthLevel;
  domain?: string;
  registered?: boolean;
  lastHeartbeatMs?: number | null;
  lastError?: string | null;
  subdomain?: string;
  claimStatus?: "unclaimed" | "claimed" | "conflict" | "error" | string;
  connectivityStatus?: "disconnected" | "connecting" | "connected" | string;
  expectedUrl?: string | null;
  activeUrl?: string | null;
  tunnelUrl?: string | null;
}

export interface ShareHealthItem {
  id: string;
  name: string;
  status: ShareHealthLevel;
  shareStatus: string;
  enabled: boolean;
  routerLastSyncError?: string | null;
  routerLastSyncedAtMs?: number | null;
  tunnelStatus?: string | null;
  tunnelError?: string | null;
}

export interface ShareHealthStatus {
  overall: ShareHealthLevel;
  issueCount: number;
  shareIssueCount: number;
  router: ShareHealthLink;
  clientTunnel: ShareHealthLink;
  shares: ShareHealthItem[];
}

async function getShareHealthStatus(): Promise<ShareHealthStatus> {
  return invokeCommand<ShareHealthStatus>("get_share_health_status");
}

async function invokeShareRecord(
  command: string,
  args: Record<string, unknown>,
): Promise<ShareRecord> {
  const raw = await invokeCommand<unknown>(command, args);
  const normalized = normalizeShareRecord(raw);
  if (!normalized) {
    throw new Error("Invalid share response");
  }
  return normalized;
}

async function create(params: CreateShareParams): Promise<ShareRecord> {
  return invokeShareRecord("create_share", { params });
}

async function listReuseCandidates(
  app: keyof ShareBindings,
  providerId: string,
): Promise<ShareReuseCandidate[]> {
  const response = await invokeCommand<{
    ok: boolean;
    candidates: ShareReuseCandidate[];
  }>("list_share_reuse_candidates", { app, providerId });
  return response.candidates ?? [];
}

async function addBinding(
  params: ShareBindingMutationParams,
): Promise<ShareRecord> {
  return invokeShareRecord("add_share_binding", { params });
}

async function removeBinding(
  params: ShareBindingMutationParams,
): Promise<RemoveShareBindingResult> {
  const response = await invokeCommand<RemoveShareBindingResult>(
    "remove_share_binding",
    { params },
  );
  return {
    ...response,
    share: response.share ? normalizeShareRecord(response.share) : null,
  };
}

async function remove(shareId: string): Promise<void> {
  return invokeCommand("delete_share", { shareId });
}

async function pause(shareId: string): Promise<void> {
  return invokeCommand("pause_share", { shareId });
}

async function resume(shareId: string): Promise<void> {
  return invokeCommand("resume_share", { shareId });
}

async function enable(shareId: string): Promise<ShareRecord> {
  return invokeShareRecord("enable_share", { shareId });
}

async function disable(shareId: string): Promise<void> {
  return invokeCommand("disable_share", { shareId });
}

async function resetUsage(shareId: string): Promise<ShareRecord> {
  return invokeShareRecord("reset_share_usage", { shareId });
}

async function updateTokenLimit(
  params: UpdateShareTokenLimitParams,
): Promise<ShareRecord> {
  return invokeShareRecord("update_share_token_limit", { params });
}

async function updateParallelLimit(
  params: UpdateShareParallelLimitParams,
): Promise<ShareRecord> {
  return invokeShareRecord("update_share_parallel_limit", { params });
}

async function updateSubdomain(
  params: UpdateShareSubdomainParams,
): Promise<ShareRecord> {
  return invokeShareRecord("update_share_subdomain", { params });
}

async function updateDescription(
  params: UpdateShareDescriptionParams,
): Promise<ShareRecord> {
  return invokeShareRecord("update_share_description", { params });
}

async function updateForSale(
  params: UpdateShareForSaleParams,
): Promise<ShareRecord> {
  return invokeShareRecord("update_share_for_sale", { params });
}

async function updateForSaleOfficialPricePercent(
  params: UpdateShareForSaleOfficialPricePercentParams,
): Promise<ShareRecord> {
  return invokeShareRecord("update_share_for_sale_official_price_percent", {
    params,
  });
}

async function updateExpiration(
  params: UpdateShareExpirationParams,
): Promise<ShareRecord> {
  return invokeShareRecord("update_share_expiration", { params });
}

async function updateAcl(params: UpdateShareAclParams): Promise<ShareRecord> {
  return invokeShareRecord("update_share_acl", { params });
}

async function saveProviderShare(
  params: SaveProviderShareParams,
): Promise<ShareRecord> {
  return invokeShareRecord("save_provider_share", { params });
}

export interface ImportSharesResult {
  imported: number;
  skippedExisting: string[];
  skippedProviderMissing: string[];
}

async function exportAll(): Promise<ShareRecord[]> {
  const raw = await invokeCommand<unknown>("export_all_shares");
  return normalizeShareRecords(raw);
}

async function importMany(shares: ShareRecord[]): Promise<ImportSharesResult> {
  return invokeCommand<ImportSharesResult>("import_shares", { shares });
}

async function listTokenMarkets(): Promise<PublicTokenMarket[]> {
  return invokeCommand<PublicTokenMarket[]>("list_token_markets");
}

async function list(): Promise<ShareRecord[]> {
  const raw = await invokeCommand<unknown>("list_shares");
  return normalizeShareRecords(raw);
}

async function getDetail(shareId: string): Promise<ShareRecord | null> {
  const raw = await invokeCommand<unknown>("get_share_detail", { shareId });
  return normalizeShareRecord(raw);
}

async function startTunnel(shareId: string): Promise<ShareRecord> {
  return invokeShareRecord("start_share_tunnel", { shareId });
}

async function stopTunnel(shareId: string): Promise<void> {
  return invokeCommand("stop_share_tunnel", { shareId });
}

async function getTunnelStatus(shareId: string): Promise<ShareTunnelStatus> {
  const raw = await invokeCommand<
    ShareTunnelStatus & {
      runtimeStatus?: {
        tunnelUrl?: string | null;
        subdomain?: string | null;
        remotePort?: number | null;
        status?: string | null;
        lastError?: string | null;
      } | null;
    }
  >("get_tunnel_status", { shareId });
  return normalizeShareTunnelStatus(raw);
}

function normalizeShareTunnelStatus(
  raw: ShareTunnelStatus & {
    runtimeStatus?: {
      tunnelUrl?: string | null;
      subdomain?: string | null;
      remotePort?: number | null;
      status?: string | null;
      lastError?: string | null;
    } | null;
  },
): ShareTunnelStatus {
  if (raw.info) {
    return {
      info: raw.info,
      lastError: raw.lastError ?? null,
      requiresOwnerLogin: raw.requiresOwnerLogin ?? false,
    };
  }
  const runtime = raw.runtimeStatus;
  if (runtime?.tunnelUrl) {
    const status = runtime.status?.trim().toLowerCase() ?? "";
    return {
      info: {
        tunnelUrl: runtime.tunnelUrl,
        subdomain: runtime.subdomain?.trim() || "",
        remotePort: runtime.remotePort ?? 0,
        healthy:
          status === "connected" ||
          status === "running" ||
          status === "active" ||
          status === "renewing" ||
          status === "renewal_retrying",
      },
      lastError: raw.lastError ?? runtime.lastError ?? null,
      requiresOwnerLogin: raw.requiresOwnerLogin ?? false,
    };
  }
  return {
    info: null,
    lastError: raw.lastError ?? null,
    requiresOwnerLogin: raw.requiresOwnerLogin ?? false,
  };
}

async function getConnectInfo(shareId: string): Promise<ConnectInfo> {
  return invokeCommand<ConnectInfo>("get_share_connect_info", { shareId });
}

async function configureTunnel(config: TunnelConfig): Promise<void> {
  return invokeCommand("configure_tunnel", { config });
}

async function getClientTunnel(): Promise<ClientTunnelState> {
  return invokeCommand<ClientTunnelState>("get_client_tunnel");
}

async function checkClientTunnelSubdomain(
  subdomain: string,
): Promise<{ ok: boolean; available: boolean; reason?: string | null }> {
  return invokeCommand("check_client_tunnel_subdomain", { subdomain });
}

async function suggestClientTunnelSubdomain(): Promise<{
  subdomain: string;
  available: boolean;
  checked: boolean;
  attempts: number;
}> {
  return invokeCommand("suggest_client_tunnel_subdomain");
}

async function suggestShareSlug(): Promise<{
  subdomain: string;
  available: boolean;
  checked: boolean;
  attempts: number;
}> {
  return invokeCommand("suggest_share_slug");
}

async function checkRouterReachable(): Promise<{ reachable: boolean }> {
  return invokeCommand("check_router_reachable");
}

async function claimClientTunnel(
  params: ClientTunnelUpdateParams,
): Promise<ClientTunnelState> {
  return invokeCommand<ClientTunnelState>("claim_client_tunnel", { params });
}

async function updateClientTunnel(
  params: ClientTunnelUpdateParams,
): Promise<ClientTunnelState> {
  return invokeCommand<ClientTunnelState>("update_client_tunnel", { params });
}

async function startClientTunnel(): Promise<TunnelInfo> {
  return invokeCommand<TunnelInfo>("start_client_tunnel");
}

async function stopClientTunnel(): Promise<void> {
  return invokeCommand("stop_client_tunnel");
}

async function getClientTunnelStatus(): Promise<ShareTunnelStatus> {
  return invokeCommand<ShareTunnelStatus>("get_client_tunnel_status");
}

export const shareApi = {
  create,
  listReuseCandidates,
  addBinding,
  removeBinding,
  delete: remove,
  pause,
  resume,
  enable,
  disable,
  resetUsage,
  updateTokenLimit,
  updateParallelLimit,
  updateSubdomain,
  updateDescription,
  updateForSale,
  updateForSaleOfficialPricePercent,
  updateExpiration,
  updateAcl,
  saveProviderShare,
  exportAll,
  importMany,
  listTokenMarkets,
  list,
  getDetail,
  startTunnel,
  stopTunnel,
  getTunnelStatus,
  getConnectInfo,
  configureTunnel,
  getClientTunnel,
  claimClientTunnel,
  checkClientTunnelSubdomain,
  checkRouterReachable,
  suggestClientTunnelSubdomain,
  suggestShareSlug,
  updateClientTunnel,
  startClientTunnel,
  stopClientTunnel,
  getClientTunnelStatus,
  getShareHealthStatus,
};

export const createShare = create;
export const deleteShare = remove;
export const listShares = list;
export const getShareDetail = getDetail;
export const startShareTunnel = startTunnel;
export const stopShareTunnel = stopTunnel;
export const getShareConnectInfo = getConnectInfo;
