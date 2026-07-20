import { invokeCommand, isTauriRuntime } from "@/lib/runtime";

export type ManagedAuthProvider =
  | "github_copilot"
  | "codex_oauth"
  | "grok_oauth"
  | "claude_oauth"
  | "google_gemini_oauth"
  | "antigravity_oauth"
  | "cursor_oauth"
  | "kiro_oauth";

export interface DeepSeekAccount {
  id: string;
  login: string;
  authenticated_at: number;
  is_default: boolean;
  has_password: boolean;
}

export interface DeepSeekAccountStatus {
  authenticated: boolean;
  default_account_id: string | null;
  accounts: DeepSeekAccount[];
}

export interface ManagedAuthAccount {
  id: string;
  provider: ManagedAuthProvider;
  login: string;
  email?: string | null;
  avatar_url: string | null;
  authenticated_at: number;
  is_default: boolean;
  github_domain: string;
  workspaces?: Array<{ id: string; name: string }>;
  selected_workspace_id?: string | null;
  subscriptionExpiry: ManagedAuthSubscriptionExpiry;
}

export type ManagedAuthSubscriptionExpiryCapability =
  | "automatic"
  | "automatic_or_manual"
  | "manual_required"
  | "research_pending"
  | "not_applicable";

export interface ManagedAuthSubscriptionExpiry {
  capability: ManagedAuthSubscriptionExpiryCapability;
  manualExpiresAt: string | null;
  effectiveExpiresAt: string | null;
  source: "automatic" | "manual" | null;
  kind: "subscription" | "billing_period" | null;
}

export interface ManagedAuthStatus {
  provider: ManagedAuthProvider;
  authenticated: boolean;
  default_account_id: string | null;
  migration_error?: string | null;
  accounts: ManagedAuthAccount[];
}

export interface ManagedAuthDeviceCodeResponse {
  provider: ManagedAuthProvider;
  device_code: string;
  user_code: string;
  verification_uri: string;
  expires_in: number;
  interval: number;
}

export interface ImportGrokAuthJsonResponse {
  ok: boolean;
  account: ManagedAuthAccount;
}

export interface ImportCursorLocalAuthResponse {
  ok: boolean;
  account: ManagedAuthAccount;
  source: "ide_state_vscdb" | "cursor_agent_auth_json" | string;
  path?: string | null;
  profileError?: string | null;
}

export interface ImportKiroCredentialsResponse {
  ok: boolean;
  account: ManagedAuthAccount;
  source?: string | null;
}

/**
 * `claude_oauth` / `grok_oauth` 在 web 模式（client URL 访问、非桌面 Tauri）下走手动粘贴回调，
 * 用户复制授权码或 callback URL 后调用 `authSubmitOauthCode` 提交。其它 provider 缺乏对应的 out-of-band 回调端点，
 * 维持原来的"web 模式禁用"行为。
 */
const WEB_PASTE_CAPABLE_PROVIDERS = new Set<ManagedAuthProvider>([
  "claude_oauth",
  "grok_oauth",
]);

const LOCAL_CALLBACK_AUTH_PROVIDERS = new Set<ManagedAuthProvider>([
  "claude_oauth",
  "grok_oauth",
  "google_gemini_oauth",
  "antigravity_oauth",
]);

function isLoopbackHostname(hostname: string): boolean {
  const value = hostname.trim().toLowerCase();
  return (
    value === "localhost" ||
    value.endsWith(".localhost") ||
    value === "127.0.0.1" ||
    value === "0.0.0.0" ||
    value === "::1" ||
    value === "[::1]"
  );
}

const DIRECT_SERVER_PORTS = new Set(["15721", "15722"]);

/**
 * Browser is talking to cc-switch-server directly (loopback, /etc/hosts alias, or
 * the well-known server bind port), not through a router client-tunnel hostname.
 */
export function isDirectServerWebAccess(): boolean {
  if (typeof window === "undefined") return true;
  if (isLoopbackHostname(window.location.hostname)) return true;
  const port = window.location.port;
  return DIRECT_SERVER_PORTS.has(port);
}

export function isLocalCallbackAuthProvider(
  authProvider: ManagedAuthProvider,
): boolean {
  return LOCAL_CALLBACK_AUTH_PROVIDERS.has(authProvider);
}

/**
 * 判断当前会话是否处于"远程 web 模式"——即通过 client URL 访问，cc-switch 进程
 * 的本机 localhost 端口不可达。用来决定 OAuth 流程要不要走 web-paste 分支。
 */
export function isRemoteWebMode(): boolean {
  if (isTauriRuntime()) return false;
  if (typeof window === "undefined") return false;
  return !isDirectServerWebAccess();
}

/**
 * Provider 是否支持 web-paste 流程（手动粘贴 platform.claude.com 上的授权码）。
 * 不支持的 provider 在 web 模式下继续被 `shouldBlockLocalCallbackAuthInClientWeb`
 * 拦截。
 */
export function supportsWebPasteFlow(
  authProvider: ManagedAuthProvider,
): boolean {
  return WEB_PASTE_CAPABLE_PROVIDERS.has(authProvider);
}

export function shouldBlockLocalCallbackAuthInClientWeb(
  authProvider: ManagedAuthProvider,
): boolean {
  if (!isLocalCallbackAuthProvider(authProvider) || isTauriRuntime()) {
    return false;
  }
  if (typeof window === "undefined") {
    return false;
  }
  if (!isRemoteWebMode()) {
    return false;
  }
  // claude_oauth 在 web 模式下走 web-paste 流程，不再拦截。
  return !supportsWebPasteFlow(authProvider);
}

export function localCallbackAuthBlockedMessage(): string {
  return "当前通过 client URL 访问，无法添加需要 localhost 回调的 OAuth 账号。请在 cc-switch 桌面端本机添加该账号后再回到 client URL 使用。Codex/Copilot/Kiro/Cursor 等非 localhost 回调登录不受影响。";
}

export async function authStartLogin(
  authProvider: ManagedAuthProvider,
  githubDomain?: string,
  /**
   * `"web_paste"` only meaningful for `claude_oauth` and only in remote web mode.
   * Backend treats anything else (including undefined) as the classic localhost
   * callback flow.
   */
  oauthFlowMode?: "web_paste" | "localhost" | "cli" | "device",
  codexCallbackUrl?: string | null,
  kiroLoginProvider?: "google" | "github" | null,
): Promise<ManagedAuthDeviceCodeResponse> {
  if (shouldBlockLocalCallbackAuthInClientWeb(authProvider)) {
    throw new Error(localCallbackAuthBlockedMessage());
  }
  return invokeCommand<ManagedAuthDeviceCodeResponse>("auth_start_login", {
    authProvider,
    githubDomain: githubDomain || null,
    oauthFlowMode: oauthFlowMode || null,
    codexCallbackUrl: codexCallbackUrl || null,
    kiroLoginProvider: kiroLoginProvider || null,
  });
}

/**
 * Web-paste 模式专用：用户在 platform.claude.com 上复制 authorization code 后
 * 调用此函数完成 token 换取。`deviceCode` 即 `authStartLogin` 返回的 state。
 */
export async function authSubmitOauthCode(
  authProvider: ManagedAuthProvider,
  deviceCode: string,
  code: string,
): Promise<ManagedAuthAccount> {
  return invokeCommand<ManagedAuthAccount>("auth_submit_oauth_code", {
    authProvider,
    deviceCode,
    code,
  });
}

export async function authPollForAccount(
  authProvider: ManagedAuthProvider,
  deviceCode: string,
  githubDomain?: string,
): Promise<ManagedAuthAccount | null> {
  return invokeCommand<ManagedAuthAccount | null>("auth_poll_for_account", {
    authProvider,
    deviceCode,
    githubDomain: githubDomain || null,
  });
}

export async function authCancelLogin(
  authProvider: ManagedAuthProvider,
  deviceCode: string,
): Promise<void> {
  await invokeCommand("auth_cancel_login", {
    authProvider,
    deviceCode,
  });
}

export async function authListAccounts(
  authProvider: ManagedAuthProvider,
): Promise<ManagedAuthAccount[]> {
  return invokeCommand<ManagedAuthAccount[]>("auth_list_accounts", {
    authProvider,
  });
}

export async function authGetStatus(
  authProvider: ManagedAuthProvider,
): Promise<ManagedAuthStatus> {
  return invokeCommand<ManagedAuthStatus>("auth_get_status", {
    authProvider,
  });
}

export async function authRemoveAccount(
  authProvider: ManagedAuthProvider,
  accountId: string,
): Promise<void> {
  return invokeCommand("auth_remove_account", {
    authProvider,
    accountId,
  });
}

export async function authSetDefaultAccount(
  authProvider: ManagedAuthProvider,
  accountId: string,
): Promise<void> {
  return invokeCommand("auth_set_default_account", {
    authProvider,
    accountId,
  });
}

export async function authSetWorkspace(
  authProvider: ManagedAuthProvider,
  accountId: string,
  workspaceId: string,
): Promise<ManagedAuthAccount> {
  return invokeCommand<ManagedAuthAccount>("auth_set_workspace", {
    authProvider,
    accountId,
    workspaceId,
  });
}

export async function authSetManualSubscriptionExpiry(
  authProvider: ManagedAuthProvider,
  accountId: string,
  expiresAt: string | null,
): Promise<ManagedAuthAccount> {
  return invokeCommand<ManagedAuthAccount>(
    "auth_set_manual_subscription_expiry",
    {
      authProvider,
      accountId,
      expiresAt,
    },
  );
}

export async function authLogout(
  authProvider: ManagedAuthProvider,
): Promise<void> {
  return invokeCommand("auth_logout", {
    authProvider,
  });
}

export async function importGrokAuthJson(
  authJson: unknown,
): Promise<ImportGrokAuthJsonResponse> {
  return invokeCommand<ImportGrokAuthJsonResponse>("grok_import_auth_json", {
    authJson,
  });
}

export async function importCursorLocalAuth(): Promise<ImportCursorLocalAuthResponse> {
  return invokeCommand<ImportCursorLocalAuthResponse>(
    "cursor_import_local_auth",
  );
}

export async function importKiroCredentials(
  credentials: unknown,
): Promise<ImportKiroCredentialsResponse> {
  return invokeCommand<ImportKiroCredentialsResponse>(
    "kiro_import_credentials_json",
    { credentials },
  );
}

export async function importKiroLocalCredentials(
  path?: string | null,
): Promise<ImportKiroCredentialsResponse> {
  return invokeCommand<ImportKiroCredentialsResponse>(
    "kiro_import_local_credentials",
    { path: path || null },
  );
}

export async function importKiroApiKey(
  apiKey: string,
  region?: string | null,
): Promise<ImportKiroCredentialsResponse> {
  return invokeCommand<ImportKiroCredentialsResponse>("kiro_import_api_key", {
    apiKey,
    region: region || null,
  });
}

export async function deepseekAccountAdd(params: {
  email?: string | null;
  mobile?: string | null;
  password: string;
}): Promise<DeepSeekAccount> {
  return invokeCommand<DeepSeekAccount>("deepseek_account_add", {
    email: params.email || null,
    mobile: params.mobile || null,
    password: params.password,
  });
}

export async function deepseekAccountList(): Promise<DeepSeekAccount[]> {
  return invokeCommand<DeepSeekAccount[]>("deepseek_account_list");
}

export async function deepseekAccountStatus(): Promise<DeepSeekAccountStatus> {
  return invokeCommand<DeepSeekAccountStatus>("deepseek_account_status");
}

export async function deepseekAccountRemove(accountId: string): Promise<void> {
  return invokeCommand("deepseek_account_remove", { accountId });
}

export async function deepseekAccountSetDefault(
  accountId: string,
): Promise<void> {
  return invokeCommand("deepseek_account_set_default", { accountId });
}

export const authApi = {
  authStartLogin,
  authCancelLogin,
  authSubmitOauthCode,
  authPollForAccount,
  authListAccounts,
  authGetStatus,
  authRemoveAccount,
  authSetDefaultAccount,
  authSetWorkspace,
  authLogout,
  importGrokAuthJson,
  importCursorLocalAuth,
  importKiroCredentials,
  importKiroLocalCredentials,
  importKiroApiKey,
  deepseekAccountAdd,
  deepseekAccountList,
  deepseekAccountStatus,
  deepseekAccountRemove,
  deepseekAccountSetDefault,
};
