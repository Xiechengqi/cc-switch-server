import { PROVIDER_TYPES } from "@/config/constants";
import type {
  DeepSeekAccountStatus,
  ManagedAuthProvider,
  ManagedAuthStatus,
} from "@/lib/api/auth";
import { resolveManagedAccountId } from "@/lib/authBinding";
import type { Provider } from "@/types";
import { getCodexBaseUrl } from "@/utils/providerConfigUtils";

export interface ProviderOption {
  id: string;
  name: string;
  /** true 表示该 provider 已被其他 active share 绑定，本表单要禁选 */
  disabled: boolean;
  /** 下拉框里跟在供应商名称后的辅助信息：账号 email/login 或请求地址。 */
  detail?: string | null;
}

export const SHARE_PROVIDER_AUTH_PROVIDERS = [
  PROVIDER_TYPES.GITHUB_COPILOT,
  PROVIDER_TYPES.CODEX_OAUTH,
  PROVIDER_TYPES.CLAUDE_OAUTH,
  PROVIDER_TYPES.GOOGLE_GEMINI_OAUTH,
  PROVIDER_TYPES.ANTIGRAVITY_OAUTH,
  PROVIDER_TYPES.CURSOR_OAUTH,
  PROVIDER_TYPES.KIRO_OAUTH,
] as const satisfies readonly ManagedAuthProvider[];

type ShareProviderAuthProvider = (typeof SHARE_PROVIDER_AUTH_PROVIDERS)[number];
type ShareAccountProvider =
  | ShareProviderAuthProvider
  | typeof PROVIDER_TYPES.DEEPSEEK_ACCOUNT;

const SHARE_PROVIDER_AUTH_PROVIDER_SET = new Set<string>(
  SHARE_PROVIDER_AUTH_PROVIDERS,
);
const SHARE_ACCOUNT_PROVIDER_SET = new Set<string>([
  ...SHARE_PROVIDER_AUTH_PROVIDERS,
  PROVIDER_TYPES.DEEPSEEK_ACCOUNT,
]);

export type ManagedAuthStatusByProvider = Partial<
  Record<ShareProviderAuthProvider, ManagedAuthStatus>
> & {
  [PROVIDER_TYPES.DEEPSEEK_ACCOUNT]?: DeepSeekAccountStatus;
};

export function buildProviderOption(
  provider: Provider,
  disabled: boolean,
  authStatuses?: ManagedAuthStatusByProvider,
): ProviderOption {
  return {
    id: provider.id,
    name: provider.name,
    disabled,
    detail: getProviderOptionDetail(provider, authStatuses),
  };
}

export function formatProviderOptionLabel(
  provider: ProviderOption,
  takenLabel?: string,
): string {
  const detail = normalizeDetail(provider.detail);
  const suffix = provider.disabled && takenLabel ? ` · ${takenLabel}` : "";
  return `${provider.name}${detail ? ` · ${detail}` : ""}${suffix}`;
}

function getProviderOptionDetail(
  provider: Provider,
  authStatuses?: ManagedAuthStatusByProvider,
): string | null {
  const accountLabel = getProviderAccountLabel(provider, authStatuses);
  if (accountLabel) return accountLabel;
  return getProviderRequestUrl(provider);
}

export function getProviderAccountLabel(
  provider: Provider,
  authStatuses?: ManagedAuthStatusByProvider,
): string | null {
  const accountProvider = getProviderAccountProvider(provider);
  if (!accountProvider) return null;
  return getAccountLabel(provider, accountProvider, authStatuses);
}

function getProviderAccountProvider(
  provider: Provider,
): ShareAccountProvider | null {
  const bindingAuthProvider = provider.meta?.authBinding?.authProvider;
  if (isShareAccountProvider(bindingAuthProvider)) {
    return bindingAuthProvider;
  }

  const providerType = provider.meta?.providerType;
  if (isShareAccountProvider(providerType)) {
    return providerType;
  }

  return null;
}

function isShareAccountProvider(
  value: string | null | undefined,
): value is ShareAccountProvider {
  return Boolean(value && SHARE_ACCOUNT_PROVIDER_SET.has(value));
}

function getAccountLabel(
  provider: Provider,
  authProvider: ShareAccountProvider,
  authStatuses?: ManagedAuthStatusByProvider,
): string | null {
  if (authProvider === PROVIDER_TYPES.DEEPSEEK_ACCOUNT) {
    const status = authStatuses?.deepseek_account;
    const accountId =
      resolveManagedAccountId(provider.meta, authProvider) ??
      status?.default_account_id ??
      null;
    const account = accountId
      ? status?.accounts.find((item) => item.id === accountId)
      : undefined;
    return normalizeDetail(account?.login ?? accountId);
  }

  if (!SHARE_PROVIDER_AUTH_PROVIDER_SET.has(authProvider)) return null;
  const status = authStatuses?.[authProvider];
  const accountId =
    resolveManagedAccountId(provider.meta, authProvider) ??
    status?.default_account_id ??
    null;
  const account = accountId
    ? status?.accounts.find((item) => item.id === accountId)
    : undefined;
  return normalizeDetail(account?.email ?? account?.login ?? accountId);
}

function getProviderRequestUrl(provider: Provider): string | null {
  const config = provider.settingsConfig ?? {};
  return firstNonEmptyString([
    getCodexBaseUrl(provider),
    config.baseUrl,
    config.baseURL,
    config.base_url,
    config.apiUrl,
    config.apiURL,
    config.api_url,
    config.url,
    config.endpoint,
    config.env?.ANTHROPIC_BASE_URL,
    config.env?.ANTHROPIC_API_URL,
    config.env?.OPENAI_BASE_URL,
    config.env?.GOOGLE_GEMINI_BASE_URL,
    config.env?.GEMINI_BASE_URL,
    config.env?.BASE_URL,
    config.options?.baseURL,
    config.options?.baseUrl,
  ]);
}

function firstNonEmptyString(values: unknown[]): string | null {
  for (const value of values) {
    const normalized = normalizeDetail(value);
    if (normalized) return normalized;
  }
  return null;
}

function normalizeDetail(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : null;
}
