import { invokeCommand } from "@/lib/runtime";

/**
 * 一个 share 在每个 app_type 上各自绑定的 provider id。
 * P8 多 app share：键固定从 "claude" | "codex" | "gemini" 三个 app 里挑，缺省 = 该 app
 * 未绑定，对应请求会被拒绝并 emit share-needs-rebind。
 */
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

export type ShareSaleMarketKind = "token" | "share";

export type ShareAppSettings = {
  forSale: "Yes" | "No" | "Free";
  saleMarketKind: ShareSaleMarketKind;
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
  name: string;
  ownerEmail: string;
  sharedWithEmails: string[];
  marketAccessMode: "selected" | "all";
  accessByApp?: ShareAccessByApp;
  appSettings?: ShareAppSettingsByApp;
  forSaleOfficialPricePercentByApp: Record<string, number>;
  description?: string | null;
  forSale: "Yes" | "No" | "Free";
  saleMarketKind?: ShareSaleMarketKind;
  /** P8: 每个 app_type 的 provider 绑定。三个 slot 各自独立，0..3 个 entry。 */
  bindings: ShareBindings;
  /**
   * P17 动态绑定：列表里的 app 表示该 slot 跟随 cc-switch 当前激活的 provider；
   * 当用户在 cc-switch 切换该 app 的 provider 时，这条 share 的 binding 会被
   * 后端自动同步过去。不在列表里的 app 视为固定绑定。缺省视作空集合。
   */
  dynamicApps?: string[];
  apiKey: string;
  settingsConfig?: string | null;
  tokenLimit: number;
  parallelLimit: number;
  tokensUsed: number;
  requestsCount: number;
  expiresAt: string;
  subdomain?: string | null;
  tunnelUrl?: string | null;
  status: string;
  autoStart: boolean;
  createdAt: string;
  lastUsedAt?: string | null;
}

export interface CreateShareParams {
  ownerEmail: string;
  /**
   * P8 多 app share：创建时一次性提交 0..3 个 binding。完全为空也允许，用户可后续
   * 在 Edit 弹窗里逐个挂 provider。与 `dynamicApps` 互斥（同一 app 不能两边都出现）。
   */
  bindings: ShareBindings;
  /**
   * P17 动态绑定：列入的 app 在创建时自动绑定到 cc-switch 当前激活的 provider，
   * 之后用户在 cc-switch 切换该 app 的 provider，本 share 的 binding 会跟着改。
   */
  dynamicApps?: string[];
  description?: string;
  forSale: "Yes" | "No" | "Free";
  saleMarketKind?: ShareSaleMarketKind;
  tokenLimit: number;
  parallelLimit: number;
  expiresInSecs: number;
  subdomain?: string;
}

export interface UpdateShareProviderBindingParams {
  shareId: string;
  /** 目标 slot 的 app_type（claude / codex / gemini）。 */
  appType: "claude" | "codex" | "gemini";
  /**
   * 新 provider id。`null` / 省略 = 清空该 slot（解绑），share 在该 app 上将不再可用。
   * `dynamic = true` 时该字段可省略，后端会解析当前 app 选中的 provider 并写入动态绑定。
   */
  providerId?: string | null;
  /** 动态绑定当前选中的 provider。 */
  dynamic?: boolean;
}

export interface ShareBindingHistoryEntry {
  id: number;
  oldProviderId: string | null;
  /** `null` 表示这是一次解绑事件（slot 被清空）。 */
  newProviderId: string | null;
  appType: string;
  changedAt: string;
}

export const SHARE_APP_TYPES: ReadonlyArray<keyof ShareBindings> = [
  "claude",
  "codex",
  "gemini",
];

/**
 * 返回该 share 已绑定的 app_type 列表（按 claude > codex > gemini 顺序）。
 */
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
 * "主 app"：用于卡片摘要、列表行、表单默认聚焦等单值场景。
 * 优先级与后端 ShareRecord::primary_app 保持一致。
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

export interface PublicMarket {
  id: string;
  displayName: string;
  email: string;
  subdomain: string;
  publicBaseUrl: string;
  marketKind?: "usage" | "share" | string;
  status: string;
}

export interface UpdateShareAclParams {
  shareId: string;
  sharedWithEmails: string[];
  marketAccessMode: "selected" | "all";
  accessByApp?: ShareAccessByApp;
  appSettings?: ShareAppSettingsByApp;
  saleMarketKind?: ShareSaleMarketKind;
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

export interface UpdateShareOwnerEmailParams {
  shareId: string;
  ownerEmail: string;
}

export interface TransferShareOwnerParams {
  shareId: string;
  targetEmail: string;
}

export interface TunnelInfo {
  tunnelUrl: string;
  subdomain: string;
  remotePort: number;
  healthy: boolean;
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
}

export interface ClientTunnelState {
  config: ClientTunnelConfig;
  status: ShareTunnelStatus;
}

export interface ClientTunnelUpdateParams {
  ownerEmail: string;
  subdomain: string;
  enabled: boolean;
  autoStart: boolean;
}

async function create(params: CreateShareParams): Promise<ShareRecord> {
  return invokeCommand<ShareRecord>("create_share", { params });
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

async function enable(shareId: string): Promise<TunnelInfo> {
  return invokeCommand<TunnelInfo>("enable_share", { shareId });
}

async function disable(shareId: string): Promise<void> {
  return invokeCommand("disable_share", { shareId });
}

async function resetUsage(shareId: string): Promise<ShareRecord> {
  return invokeCommand<ShareRecord>("reset_share_usage", { shareId });
}

async function updateTokenLimit(
  params: UpdateShareTokenLimitParams,
): Promise<ShareRecord> {
  return invokeCommand<ShareRecord>("update_share_token_limit", { params });
}

async function updateParallelLimit(
  params: UpdateShareParallelLimitParams,
): Promise<ShareRecord> {
  return invokeCommand<ShareRecord>("update_share_parallel_limit", { params });
}

async function updateSubdomain(
  params: UpdateShareSubdomainParams,
): Promise<ShareRecord> {
  return invokeCommand<ShareRecord>("update_share_subdomain", { params });
}

async function updateDescription(
  params: UpdateShareDescriptionParams,
): Promise<ShareRecord> {
  return invokeCommand<ShareRecord>("update_share_description", { params });
}

async function updateForSale(
  params: UpdateShareForSaleParams,
): Promise<ShareRecord> {
  return invokeCommand<ShareRecord>("update_share_for_sale", { params });
}

async function updateForSaleOfficialPricePercent(
  params: UpdateShareForSaleOfficialPricePercentParams,
): Promise<ShareRecord> {
  return invokeCommand<ShareRecord>(
    "update_share_for_sale_official_price_percent",
    {
      params,
    },
  );
}

async function updateExpiration(
  params: UpdateShareExpirationParams,
): Promise<ShareRecord> {
  return invokeCommand<ShareRecord>("update_share_expiration", { params });
}

async function updateOwnerEmail(
  params: UpdateShareOwnerEmailParams,
): Promise<ShareRecord> {
  return invokeCommand<ShareRecord>("update_share_owner_email", { params });
}

async function transferOwner(
  params: TransferShareOwnerParams,
): Promise<ShareRecord> {
  return invokeCommand<ShareRecord>("transfer_share_owner", { params });
}

async function updateAcl(params: UpdateShareAclParams): Promise<ShareRecord> {
  return invokeCommand<ShareRecord>("update_share_acl", { params });
}

async function updateProviderBinding(
  params: UpdateShareProviderBindingParams,
): Promise<ShareRecord> {
  return invokeCommand<ShareRecord>("update_share_provider_binding", {
    params,
  });
}

async function listBindingHistory(
  shareId: string,
  limit?: number,
): Promise<ShareBindingHistoryEntry[]> {
  return invokeCommand<ShareBindingHistoryEntry[]>(
    "list_share_binding_history",
    {
      shareId,
      limit,
    },
  );
}

export interface ImportSharesResult {
  imported: number;
  skippedExisting: string[];
  skippedProviderMissing: string[];
}

async function exportAll(): Promise<ShareRecord[]> {
  return invokeCommand<ShareRecord[]>("export_all_shares");
}

async function importMany(shares: ShareRecord[]): Promise<ImportSharesResult> {
  return invokeCommand<ImportSharesResult>("import_shares", { shares });
}

async function listMarkets(): Promise<PublicMarket[]> {
  return invokeCommand<PublicMarket[]>("list_share_markets");
}

async function authorizeMarket(
  shareId: string,
  marketEmail: string,
): Promise<ShareRecord> {
  return invokeCommand<ShareRecord>("authorize_share_market", {
    shareId,
    marketEmail,
  });
}

async function list(): Promise<ShareRecord[]> {
  return invokeCommand<ShareRecord[]>("list_shares");
}

async function getDetail(shareId: string): Promise<ShareRecord | null> {
  return invokeCommand<ShareRecord | null>("get_share_detail", { shareId });
}

async function startTunnel(shareId: string): Promise<TunnelInfo> {
  return invokeCommand<TunnelInfo>("start_share_tunnel", { shareId });
}

async function stopTunnel(shareId: string): Promise<void> {
  return invokeCommand("stop_share_tunnel", { shareId });
}

async function getTunnelStatus(shareId: string): Promise<ShareTunnelStatus> {
  return invokeCommand<ShareTunnelStatus>("get_tunnel_status", { shareId });
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
  updateOwnerEmail,
  transferOwner,
  updateAcl,
  updateProviderBinding,
  listBindingHistory,
  exportAll,
  importMany,
  listMarkets,
  authorizeMarket,
  list,
  getDetail,
  startTunnel,
  stopTunnel,
  getTunnelStatus,
  getConnectInfo,
  configureTunnel,
  getClientTunnel,
  claimClientTunnel,
  updateClientTunnel,
  startClientTunnel,
  stopClientTunnel,
  getClientTunnelStatus,
};

export const createShare = create;
export const deleteShare = remove;
export const listShares = list;
export const getShareDetail = getDetail;
export const startShareTunnel = startTunnel;
export const stopShareTunnel = stopTunnel;
export const getShareConnectInfo = getConnectInfo;
