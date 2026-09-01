export type ProviderCategory =
  | "official"
  | "cn_official"
  | "cloud_provider"
  | "aggregator"
  | "third_party"
  | "custom";

export interface Provider {
  id: string;
  name: string;
  settingsConfig: Record<string, any>;
  websiteUrl?: string;
  category?: ProviderCategory;
  createdAt?: number;
  sortIndex?: number;
  notes?: string;
  meta?: ProviderMeta;
  icon?: string;
  iconColor?: string;
}

export type AuthBindingSource =
  | "provider_config"
  | "managed_account"
  | "account"
  | "account_store";

export interface AuthBinding {
  source: AuthBindingSource;
  authProvider?: string;
  accountId?: string;
  authIdentityGeneration?: number;
}

export type ClaudeApiKeyField = "ANTHROPIC_AUTH_TOKEN" | "ANTHROPIC_API_KEY";

/** Server-owned Provider metadata consumed by the current Web surfaces. */
export interface ProviderMeta {
  apiFormat?:
    | "anthropic"
    | "openai_chat"
    | "openai_responses"
    | "gemini_native";
  authBinding?: AuthBinding;
  apiKeyField?: ClaudeApiKeyField;
  codexFastMode?: boolean;
  codexImageGenerationEnabled?: boolean;
  grokImageGenerationEnabled?: boolean;
  grokImageEditEnabled?: boolean;
  grokVideoGenerationEnabled?: boolean;
  codexWebsocketEnabled?: boolean;
  codexResponsesKeepaliveIntervalMs?: number;
  codexRoutingHintEnabled?: boolean;
  customUserAgent?: string;
  providerType?: string;
  githubAccountId?: string;
}

/**
 * Server Web UI settings. Compatibility-only fields returned by older stores
 * are retained by the backend merge and are deliberately not written by Web.
 */
export interface Settings {
  oauthQuotaRefreshIntervalMinutes?: number;
  oauthQuotaRefreshTimeoutSeconds?: number;
  language?: "en" | "zh" | "zh-TW" | "ja";
  backupIntervalHours?: number;
  backupRetainCount?: number;
  shareRouterDomain?: string;
  upgradePolicy?: UpgradePolicy;
}

export interface UpgradePolicy {
  delegateUpgradeToRouterOwner: boolean;
  autoUpgradeEnabled: boolean;
  autoUpgradeCheckIntervalMinutes: number;
}
