import { invokeCommand, jsonFetch } from "@/lib/runtime";
import type { Provider } from "@/types";
import type { CoreProviderApp } from "@/server/providerRegistry";

export interface ProviderSortUpdate {
  id: string;
  sortIndex: number;
}

export interface ProviderInferenceTestResponse {
  ok: boolean;
  operation: "image_generation" | "image_edit" | "video_generation";
  statusCode: number;
  latencyMs: number;
  contentType?: string;
  bodyText: string;
  bodyTruncated: boolean;
}

export type ProviderUpstreamProtocol =
  "anthropic_messages" | "open_ai_chat" | "open_ai_responses" | "gemini_native";

export type ProviderAuthScheme =
  | "none"
  | "api_key"
  | "bearer"
  | "oauth"
  | "aws_sig_v4"
  | "custom_header"
  | "query";

export type ProviderModelPolicy = "passthrough" | "single";
export type ProviderModelPolicyScope = "global" | "per_app";
export type ProviderModelPolicySource =
  "bundle_global" | "app_independent" | "profile_fixed";

export interface ProviderTransportOverrides {
  timeoutMs?: number;
  streamFirstByteTimeoutMs?: number;
  streamIdleTimeoutMs?: number;
}

export interface ProviderRequestDefaults {
  requestTimeoutSeconds: number;
  streamFirstByteTimeoutSeconds: number;
  streamIdleTimeoutSeconds: number;
}

export interface ProviderHealthCheckConfig {
  timeoutSeconds: number;
  maxRetries: number;
  degradedThresholdSeconds: number;
  testModels: Record<CoreProviderApp, string>;
}

export interface ProviderCustomBinding {
  upstreamProtocol: ProviderUpstreamProtocol;
  authScheme: ProviderAuthScheme;
}

export interface ProviderIdentityView {
  status:
    | "bound"
    | "profile_upgrade_available"
    | "adoption_available"
    | "legacy_compat"
    | "needs_attention";
  suggestedProfileId?: string;
  currentProfileSchemaRevision?: number;
  warning?: string;
}

export type CodingPlanQuotaAdapter =
  "kimi" | "zhipu" | "minimax" | "volcengine" | "unavailable";

export interface ProviderRuntimeCodingPlan {
  contractRevision: number;
  fixedOrigin: string;
  protocol: ProviderUpstreamProtocol;
  inferenceCredentialSlot: string;
  inferenceAuthScheme: ProviderAuthScheme;
  routes: Partial<
    Record<
      | "claude_messages"
      | "claude_count_tokens"
      | "codex_chat_completions"
      | "codex_responses",
      string
    >
  >;
  models: Array<{
    id: string;
    displayName: string;
    contextWindow: number;
    inputModalities: Array<"text" | "image">;
  }>;
  quota: {
    adapter: CodingPlanQuotaAdapter;
    endpoint?: string;
    credentialSlots: Array<{
      role: "inference_credential" | "access_key_id" | "secret_access_key";
      slot: string;
    }>;
    cacheTtlMs: number;
    staleTtlMs: number;
  };
  cacheTokens: "input_includes_cached" | "input_excludes_cached";
  stream: {
    format: "anthropic_sse" | "open_ai_chat_sse" | "open_ai_responses_sse";
    terminalEvent: string;
    errorBeforeTerminalIsFatal: boolean;
  };
  error: {
    envelope: "anthropic" | "open_ai";
    retrySameCredentialOnceOn401: boolean;
    retryAfterCommit: boolean;
  };
  pricing: {
    evidence: "flat_rate_subscription_no_usd";
    source: string;
    capturedAt: string;
  };
}

export interface ProviderRuntimePlan {
  providerKey: { app: CoreProviderApp; providerId: string };
  providerRevision: number;
  profileId: string;
  profileSchemaRevision: number;
  driverId: string;
  driverContractRevision: number;
  endpoint: string;
  upstreamProtocol: string;
  outboundIdentityPolicy: unknown;
  authRef: unknown;
  modelPolicy:
    { mode: "passthrough" } | { mode: "single"; upstreamModel: string };
  codingPlan?: ProviderRuntimeCodingPlan;
  testModel?: string;
  probePolicyFingerprint: string;
  awsRegion?: string;
  mediaPolicy?: unknown;
  transportPolicy: {
    timeoutMs: number;
    streamFirstByteTimeoutMs?: number;
    streamIdleTimeoutMs?: number;
    redirectPolicy: string;
    directConnection: boolean;
  };
  extraHeaders?: Array<{ name: string; credentialSlot: string }>;
  driverOptions: Record<string, unknown>;
  configurationState: "ready" | "legacy_compat" | "needs_attention";
  warnings?: string[];
  runtimeFingerprint: string;
}

export type CodingPlanQuotaState =
  "supported" | "stale" | "unknown" | "unavailable";

export interface CodingPlanQuotaWindow {
  kind: "five_hour" | "weekly" | "monthly";
  scope?: string;
  utilization: number;
  resetsAtMs?: number;
  used?: number;
  limit?: number;
  unit?: string;
}

export interface CodingPlanQuotaSnapshot {
  providerKey: { app: CoreProviderApp; providerId: string };
  providerRevision: number;
  credentialGeneration: number;
  runtimeFingerprint: string;
  profileId: string;
  source: "live" | "fresh_cache" | "stale_cache" | "contract";
  quota: {
    state: CodingPlanQuotaState;
    windows: CodingPlanQuotaWindow[];
    plan?: string;
    observedAtMs?: number;
    staleSinceMs?: number;
    reason?: string;
  };
}

export type OllamaCloudSnapshotSource =
  "live" | "fresh_cache" | "stale_cache" | "configuration";
export type OllamaCloudSnapshotStatus =
  "complete" | "partial" | "stale" | "error" | "unconfigured";
export type OllamaCloudSectionState =
  "available" | "stale" | "error" | "unavailable";
export type OllamaCloudErrorKind =
  | "authentication"
  | "rate_limited"
  | "transient"
  | "invalid_response"
  | "not_configured";

export interface OllamaCloudSection<T> {
  state: OllamaCloudSectionState;
  data?: T;
  observedAtMs?: number;
  staleSinceMs?: number;
  errorKind?: OllamaCloudErrorKind;
  reason?: string;
  retryAfterMs?: number;
}

export interface OllamaCloudAccountView {
  id?: string;
  email?: string;
  name?: string;
  firstName?: string;
  lastName?: string;
  avatarUrl?: string;
  plan?: string;
  createdAtMs?: number;
}

export interface OllamaCloudModelUsage {
  name: string;
  requestCount: number;
}

export interface OllamaCloudUsageWindow {
  kind: "session" | "weekly";
  utilization: number;
  models: OllamaCloudModelUsage[];
  modelsTruncated: boolean;
}

export interface OllamaCloudActivityView {
  cost?: string;
  period?: {
    kind: string;
    startingAtMs?: number;
    endingAtMs?: number;
  };
  models: OllamaCloudModelUsage[];
  modelsTruncated: boolean;
}

export interface OllamaCloudUsageView {
  limits: OllamaCloudUsageWindow[];
  activity?: OllamaCloudActivityView;
}

export interface OllamaCloudSnapshot {
  providerKey: { app: CoreProviderApp; providerId: string };
  providerRevision: number;
  credentialSourceKey: { app: CoreProviderApp; providerId: string };
  credentialGeneration: number;
  source: OllamaCloudSnapshotSource;
  status: OllamaCloudSnapshotStatus;
  account: OllamaCloudSection<OllamaCloudAccountView>;
  usage: OllamaCloudSection<OllamaCloudUsageView>;
}

export interface ProviderResource {
  app: "claude" | "codex" | "gemini";
  provider: Provider;
  providerType: string;
  providerTypeId: string;
  revision: number;
  profileId?: string;
  profileSchemaRevision?: number;
  customBinding?: ProviderCustomBinding;
  identity: ProviderIdentityView;
  orderIndex?: number;
  credentialConfigured: boolean;
  credentialSlots: string[];
  cursorAccount?: {
    label?: string;
    email?: string;
    name?: string;
    credentialName?: string;
    subscriptionLevel?: string;
  };
  runtime?: ProviderRuntimePlan;
}

export interface ProviderBundleView {
  id: string;
  familyId: string;
  revision: number;
  name: string;
  websiteUrl?: string;
  notes?: string;
  icon?: string;
  iconColor?: string;
  modelPolicyScope: ProviderModelPolicyScope;
  testApp: CoreProviderApp;
  testModel?: string;
  surfaceTestModels: Partial<Record<CoreProviderApp, string>>;
  transport: ProviderTransportOverrides;
  supportedApps: CoreProviderApp[];
  enabledApps: CoreProviderApp[];
  credentialConfigured: boolean;
  credentialSlots: string[];
  surfaces: Partial<Record<CoreProviderApp, ProviderResource>>;
}

export interface ProviderBundleSurfaceWriteDraft {
  app: CoreProviderApp;
  enabled: boolean;
  profileId: string;
  modelPolicy?: ProviderModelPolicy;
  upstreamModel?: string;
  endpoint?: string;
  driverOptions?: {
    apiKeyField?: string;
    customUserAgent?: string;
    codexFastMode?: boolean;
    codexImageGenerationEnabled?: boolean;
    grokImageGenerationEnabled?: boolean;
    grokImageEditEnabled?: boolean;
    grokVideoGenerationEnabled?: boolean;
    codexWebsocketEnabled?: boolean;
    codexResponsesKeepaliveIntervalMs?: number;
    codexRoutingHintEnabled?: boolean;
  };
  extraHeaders?: string[];
  customBinding?: ProviderCustomBinding;
  credentialPatches?: ProviderCredentialPatches;
}

export interface ProviderBundleWriteDraft {
  id: string;
  familyId: string;
  name: string;
  websiteUrl?: string;
  notes?: string;
  icon?: string;
  iconColor?: string;
  modelPolicyScope: ProviderModelPolicyScope;
  modelPolicy?: ProviderModelPolicy;
  upstreamModel?: string;
  testApp: CoreProviderApp;
  testModel?: string;
  surfaceTestModels?: Partial<Record<CoreProviderApp, string>>;
  transport?: ProviderTransportOverrides;
  managedAccount?: {
    accountId: string;
    authIdentityGeneration: number;
  };
  awsRegion?: string;
  surfaces: ProviderBundleSurfaceWriteDraft[];
  credentialPatches?: ProviderCredentialPatches;
  expectedRevision?: number;
  clientRequestId?: string;
}

export interface ProviderBundleReferencePreview {
  bundleId: string;
  revision: number;
  shareIds: string[];
  blocked: boolean;
}

export type ProviderCredentialPatch =
  | { action: "keep" }
  | { action: "replace"; value: string }
  | { action: "clear" };

export type ProviderCredentialPatches = Record<string, ProviderCredentialPatch>;

export const providersApi = {
  async testGrokMedia(
    providerId: string,
    operation: ProviderInferenceTestResponse["operation"],
  ): Promise<ProviderInferenceTestResponse> {
    return jsonFetch(`/api/providers/${encodeURIComponent(providerId)}/inference-test`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ operation }),
    });
  },
  async getRequestDefaults(): Promise<ProviderRequestDefaults> {
    return await invokeCommand("get_provider_request_defaults");
  },

  async saveRequestDefaults(
    defaults: ProviderRequestDefaults,
  ): Promise<boolean> {
    return await invokeCommand("save_provider_request_defaults", { defaults });
  },

  async getHealthCheckConfig(): Promise<ProviderHealthCheckConfig> {
    return await invokeCommand("get_provider_health_check_config");
  },

  async saveHealthCheckConfig(
    config: ProviderHealthCheckConfig,
  ): Promise<boolean> {
    return await invokeCommand("save_provider_health_check_config", { config });
  },

  async getBundles(): Promise<ProviderBundleView[]> {
    return await invokeCommand("get_provider_bundles");
  },

  async updateBundleSortOrder(updates: ProviderSortUpdate[]): Promise<boolean> {
    return await invokeCommand("update_provider_bundles_sort_order", {
      updates,
    });
  },

  async upsertBundle(
    bundle: ProviderBundleWriteDraft,
  ): Promise<ProviderBundleView> {
    return await invokeCommand("upsert_provider_bundle", { bundle });
  },

  async getBundleDeletePreview(
    id: string,
  ): Promise<ProviderBundleReferencePreview> {
    return await invokeCommand("get_provider_bundle_delete_preview", { id });
  },

  async deleteBundle(id: string, expectedRevision: number): Promise<boolean> {
    return await invokeCommand("delete_provider_bundle", {
      id,
      expectedRevision,
    });
  },

  async getAll(appId: CoreProviderApp): Promise<Record<string, Provider>> {
    return await invokeCommand("get_providers", { app: appId });
  },

  async getResources(appId: CoreProviderApp): Promise<ProviderResource[]> {
    return await invokeCommand("get_provider_resources", { app: appId });
  },

  async getCodingPlanQuota(
    app: CoreProviderApp,
    providerId: string,
  ): Promise<CodingPlanQuotaSnapshot> {
    return await invokeCommand("get_coding_plan_quota", { app, providerId });
  },

  async refreshCodingPlanQuota(
    app: CoreProviderApp,
    providerId: string,
  ): Promise<CodingPlanQuotaSnapshot> {
    return await invokeCommand(
      "refresh_coding_plan_quota",
      { app, providerId },
      { cache: "no-store" },
    );
  },

  async getProviderAccountUsage(
    app: CoreProviderApp,
    providerId: string,
  ): Promise<OllamaCloudSnapshot> {
    return await invokeCommand("get_provider_account_usage", {
      app,
      providerId,
    });
  },

  async refreshProviderAccountUsage(
    app: CoreProviderApp,
    providerId: string,
  ): Promise<OllamaCloudSnapshot> {
    return await invokeCommand(
      "refresh_provider_account_usage",
      { app, providerId },
      { cache: "no-store" },
    );
  },

  async getCredential(
    appId: CoreProviderApp,
    providerId: string,
    slot: string,
  ): Promise<string> {
    return await invokeCommand(
      "get_provider_credential",
      {
        app: appId,
        providerId,
        slot,
      },
      { cache: "no-store" },
    );
  },

};
