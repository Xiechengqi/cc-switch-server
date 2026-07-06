import { useMemo, useState, useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import { GripVertical, ChevronDown, ChevronUp } from "lucide-react";
import { useTranslation } from "react-i18next";
import type {
  DraggableAttributes,
  DraggableSyntheticListeners,
} from "@dnd-kit/core";
import type { Provider } from "@/types";
import { authApi, type AppId } from "@/lib/api";
import type { ManagedAuthProvider, ManagedAuthStatus } from "@/lib/api";
import { PROVIDER_TYPES } from "@/config/constants";
import { cn } from "@/lib/utils";
import { ProviderActions } from "@/components/providers/ProviderActions";
import { ProviderIcon } from "@/components/ProviderIcon";
import UsageFooter from "@/components/UsageFooter";
import SubscriptionQuotaFooter from "@/components/SubscriptionQuotaFooter";
import CopilotQuotaFooter from "@/components/CopilotQuotaFooter";
import CodexOauthQuotaFooter from "@/components/CodexOauthQuotaFooter";
import ClaudeOauthQuotaFooter from "@/components/ClaudeOauthQuotaFooter";
import GeminiOauthQuotaFooter from "@/components/GeminiOauthQuotaFooter";
import KiroOauthQuotaFooter from "@/components/KiroOauthQuotaFooter";
import AntigravityOauthQuotaFooter from "@/components/AntigravityOauthQuotaFooter";
import CursorOauthQuotaFooter from "@/components/CursorOauthQuotaFooter";
import OllamaQuotaFooter from "@/components/OllamaQuotaFooter";
import { TEMPLATE_TYPES } from "@/config/constants";
import { isHermesReadOnlyProvider } from "@/config/hermesProviderPresets";
import { ProviderHealthBadge } from "@/components/providers/ProviderHealthBadge";
import { FailoverPriorityBadge } from "@/components/providers/FailoverPriorityBadge";
import {
  extractCodexBaseUrl,
  extractCodexExperimentalBearerToken,
} from "@/utils/providerConfigUtils";
import {
  canTestLinkProvider,
  canTestModelProvider,
  getProviderQuotaSource,
  isManagedOauthProvider,
} from "@/utils/providerMetaUtils";
import { useProviderHealth } from "@/lib/query/failover";
import { useUsageQuery } from "@/lib/query/queries";
import { resolveManagedAccountId } from "@/lib/authBinding";

interface DragHandleProps {
  attributes: DraggableAttributes;
  listeners: DraggableSyntheticListeners;
  isDragging: boolean;
}

interface ProviderCardProps {
  provider: Provider;
  isCurrent: boolean;
  appId: AppId;
  isInConfig?: boolean; // OpenCode: 是否已添加到 opencode.json
  isOmo?: boolean;
  isOmoSlim?: boolean;
  onSwitch: (provider: Provider) => void;
  onEdit: (provider: Provider) => void;
  onDelete: (provider: Provider) => void;
  onRemoveFromConfig?: (provider: Provider) => void;
  onDisableOmo?: () => void;
  onDisableOmoSlim?: () => void;
  onConfigureUsage: (provider: Provider) => void;
  onOpenWebsite: (url: string) => void;
  onDuplicate: (provider: Provider) => void;
  onTestLink?: (provider: Provider) => void;
  onTestModel?: (provider: Provider) => void;
  onOpenTerminal?: (provider: Provider) => void;
  isTestingLink?: boolean;
  isTestingModel?: boolean;
  isProxyRunning: boolean;
  isProxyTakeover?: boolean; // 代理接管模式（Live配置已被接管，切换为热切换）
  dragHandleProps?: DragHandleProps;
  isAutoFailoverEnabled?: boolean; // 是否开启自动故障转移
  failoverPriority?: number; // 故障转移优先级（1 = P1, 2 = P2, ...）
  isInFailoverQueue?: boolean; // 是否在故障转移队列中
  onToggleFailover?: (enabled: boolean) => void; // 切换故障转移队列
  activeProviderId?: string; // 代理当前实际使用的供应商 ID（用于故障转移模式下标注绿色边框）
  // OpenClaw: default model
  isDefaultModel?: boolean;
  onSetAsDefault?: () => void;
}

/** 判断是否为官方供应商（无自定义 base URL / API key，直连官方 API） */
function isOfficialProvider(provider: Provider, appId: AppId): boolean {
  if (provider.category === "official") {
    return true;
  }

  const config = provider.settingsConfig as Record<string, any>;
  if (appId === "claude") {
    const baseUrl = config?.env?.ANTHROPIC_BASE_URL;
    return !baseUrl || (typeof baseUrl === "string" && baseUrl.trim() === "");
  }
  if (appId === "codex") {
    // 无 OPENAI_API_KEY → 使用 Codex CLI 内置 OAuth（官方）
    const apiKey = config?.auth?.OPENAI_API_KEY;
    const bearerToken =
      typeof config?.config === "string"
        ? extractCodexExperimentalBearerToken(config.config)
        : undefined;
    return (
      !bearerToken &&
      (!apiKey || (typeof apiKey === "string" && apiKey.trim() === ""))
    );
  }
  if (appId === "gemini") {
    // 无 GEMINI_API_KEY 且无 GOOGLE_GEMINI_BASE_URL → Google OAuth 官方模式
    const apiKey = config?.env?.GEMINI_API_KEY;
    const baseUrl = config?.env?.GOOGLE_GEMINI_BASE_URL;
    return (
      (!apiKey || (typeof apiKey === "string" && apiKey.trim() === "")) &&
      (!baseUrl || (typeof baseUrl === "string" && baseUrl.trim() === ""))
    );
  }
  return false;
}

const extractConfiguredApiUrl = (provider: Provider) => {
  const config = provider.settingsConfig;

  if (config && typeof config === "object") {
    const envBase =
      (config as Record<string, any>)?.env?.ANTHROPIC_BASE_URL ||
      (config as Record<string, any>)?.env?.GOOGLE_GEMINI_BASE_URL;
    if (typeof envBase === "string" && envBase.trim()) {
      return envBase;
    }

    const baseUrl = (config as Record<string, any>)?.config;

    if (typeof baseUrl === "string" && baseUrl.includes("base_url")) {
      const extractedBaseUrl = extractCodexBaseUrl(baseUrl);
      if (extractedBaseUrl) {
        return extractedBaseUrl;
      }
    }
  }

  return null;
};

const extractApiUrl = (provider: Provider, fallbackText: string) => {
  const configuredApiUrl = extractConfiguredApiUrl(provider);
  if (provider.category !== "official" && configuredApiUrl) {
    return configuredApiUrl;
  }

  if (provider.notes?.trim()) {
    return provider.notes.trim();
  }

  if (provider.websiteUrl) {
    return provider.websiteUrl;
  }

  if (configuredApiUrl) {
    return configuredApiUrl;
  }

  return fallbackText;
};

const quotaSourceToAuthProvider = (
  quotaSource: ReturnType<typeof getProviderQuotaSource>,
): ManagedAuthProvider | null => {
  if (quotaSource === "copilot") return PROVIDER_TYPES.GITHUB_COPILOT;
  if (quotaSource === "codex_oauth") return PROVIDER_TYPES.CODEX_OAUTH;
  if (quotaSource === "claude_oauth") return PROVIDER_TYPES.CLAUDE_OAUTH;
  if (quotaSource === "google_gemini_oauth")
    return PROVIDER_TYPES.GOOGLE_GEMINI_OAUTH;
  if (quotaSource === "antigravity_oauth")
    return PROVIDER_TYPES.ANTIGRAVITY_OAUTH;
  if (quotaSource === "cursor_oauth") return PROVIDER_TYPES.CURSOR_OAUTH;
  if (quotaSource === "cursor_apikey") return null;
  if (quotaSource === "kiro_oauth") return PROVIDER_TYPES.KIRO_OAUTH;
  return null;
};

function useManagedOauthAccountLogin(
  provider: Provider,
  quotaSource: ReturnType<typeof getProviderQuotaSource>,
) {
  const authProvider = quotaSourceToAuthProvider(quotaSource);
  const { data: authStatus } = useQuery<ManagedAuthStatus>({
    queryKey: ["managed-auth-status", authProvider],
    queryFn: () => authApi.authGetStatus(authProvider!),
    enabled: authProvider !== null,
    staleTime: 30000,
  });

  if (!authProvider) {
    return null;
  }

  const accountId =
    resolveManagedAccountId(provider.meta, authProvider) ??
    authStatus?.default_account_id ??
    null;
  const account = accountId
    ? authStatus?.accounts.find((item) => item.id === accountId)
    : undefined;

  return account?.email || account?.login || null;
}

export function ProviderCard({
  provider,
  isCurrent,
  appId,
  isInConfig = true,
  isOmo = false,
  isOmoSlim = false,
  onSwitch,
  onEdit,
  onDelete,
  onRemoveFromConfig,
  onDisableOmo,
  onDisableOmoSlim,
  onConfigureUsage,
  onOpenWebsite,
  onDuplicate,
  onTestLink,
  onTestModel,
  onOpenTerminal,
  isTestingLink,
  isTestingModel,
  isProxyRunning,
  isProxyTakeover = false,
  dragHandleProps,
  isAutoFailoverEnabled = false,
  failoverPriority,
  isInFailoverQueue = false,
  onToggleFailover,
  activeProviderId,
  // OpenClaw: default model
  isDefaultModel,
  onSetAsDefault,
}: ProviderCardProps) {
  const { t } = useTranslation();

  // OMO and OMO Slim share the same card behavior
  const isAnyOmo = isOmo || isOmoSlim;
  const handleDisableAnyOmo = isOmoSlim ? onDisableOmoSlim : onDisableOmo;
  const isAdditiveMode = appId === "opencode" && !isAnyOmo;

  const { data: health } = useProviderHealth(provider.id, appId);

  const fallbackUrlText = t("provider.notConfigured", {
    defaultValue: "未配置接口地址",
  });
  const quotaSource = getProviderQuotaSource(provider, appId);
  const managedOauthAccountLogin = useManagedOauthAccountLogin(
    provider,
    quotaSource,
  );
  const oauthAccountLogin = managedOauthAccountLogin;

  const displayUrl = useMemo(() => {
    if (isManagedOauthProvider(provider, appId)) {
      return oauthAccountLogin
        ? t("provider.oauthAccountDisplay", {
            account: oauthAccountLogin,
            defaultValue: `OAuth account: ${oauthAccountLogin}`,
          })
        : t("provider.oauthAccountResolving", {
            defaultValue: "OAuth account",
          });
    }
    return extractApiUrl(provider, fallbackUrlText);
  }, [appId, oauthAccountLogin, provider, fallbackUrlText, t]);

  const isClickableUrl = useMemo(() => {
    if (isManagedOauthProvider(provider, appId)) {
      return false;
    }
    if (provider.notes?.trim()) {
      return false;
    }
    if (displayUrl === fallbackUrlText) {
      return false;
    }
    return true;
  }, [appId, provider, displayUrl, fallbackUrlText]);

  const usageEnabled = provider.meta?.usage_script?.enabled ?? false;
  const isOfficial = isOfficialProvider(provider, appId);
  const isManagedOauth = isManagedOauthProvider(provider, appId);
  const supportsOfficialSubscription =
    isOfficial && ["claude", "codex", "gemini"].includes(appId);
  const isOfficialSubscriptionUsage =
    provider.meta?.usage_script?.templateType ===
    TEMPLATE_TYPES.OFFICIAL_SUBSCRIPTION;
  const officialSubscriptionEnabled =
    supportsOfficialSubscription && usageEnabled && isOfficialSubscriptionUsage;
  // Hermes v12+ overlay entries live under the `providers:` dict and are
  // read-only here — writes have to go through Hermes Web UI.
  const isHermesReadOnly =
    appId === "hermes" && isHermesReadOnlyProvider(provider.settingsConfig);

  // 获取用量数据以判断是否有多套餐
  // 累加模式应用（OpenCode/OpenClaw/Hermes）：使用 isInConfig 代替 isCurrent
  const shouldAutoQuery =
    appId === "opencode" || appId === "openclaw" || appId === "hermes"
      ? isInConfig
      : isCurrent;
  const autoQueryInterval = shouldAutoQuery
    ? provider.meta?.usage_script?.autoQueryInterval || 0
    : 0;

  const { data: usage } = useUsageQuery(provider.id, appId, {
    enabled: usageEnabled && !isOfficial && !isOfficialSubscriptionUsage,
    autoQueryInterval,
  });

  const isTokenPlan =
    provider.meta?.usage_script?.templateType === "token_plan";
  const hasMultiplePlans =
    usage?.success && usage.data && usage.data.length > 1 && !isTokenPlan;

  const [isExpanded, setIsExpanded] = useState(false);

  useEffect(() => {
    if (hasMultiplePlans) {
      setIsExpanded(true);
    }
  }, [hasMultiplePlans]);

  const handleOpenWebsite = () => {
    if (!isClickableUrl) {
      return;
    }
    onOpenWebsite(displayUrl);
  };

  // 判断是否是"当前使用中"的供应商
  // - OMO/OMO Slim 供应商：使用 isCurrent
  // - OpenClaw：使用默认模型归属的 provider 作为当前项（蓝色边框）
  // - OpenCode（非 OMO）：不存在"当前"概念，返回 false
  // - 故障转移模式：优先使用代理实际使用的供应商，状态未就绪时回退到当前选中项
  // - 普通模式：isCurrent
  const failoverActiveProviderId = activeProviderId?.trim();
  const isActiveProvider = isAnyOmo
    ? isCurrent
    : appId === "openclaw"
      ? Boolean(isDefaultModel)
      : appId === "opencode"
        ? false
        : isAutoFailoverEnabled
          ? failoverActiveProviderId
            ? failoverActiveProviderId === provider.id
            : isCurrent
          : isCurrent;

  const shouldUseGreen =
    !isAnyOmo && (isProxyTakeover || isAutoFailoverEnabled) && isActiveProvider;
  const hasPersistentConfigHighlight = isAdditiveMode && isInConfig;
  const shouldUseBlue =
    (isAnyOmo && isActiveProvider) ||
    (!isAnyOmo &&
      !isProxyTakeover &&
      (isActiveProvider || hasPersistentConfigHighlight));

  return (
    <div
      className={cn(
        "relative overflow-hidden rounded-xl border border-border p-4 transition-all duration-300",
        "bg-card text-card-foreground group",
        isAutoFailoverEnabled || isProxyTakeover
          ? "hover:border-emerald-500/50"
          : "hover:border-border-active",
        shouldUseGreen &&
          "border-emerald-500/60 shadow-sm shadow-emerald-500/10",
        shouldUseBlue && "border-blue-500/60 shadow-sm shadow-blue-500/10",
        !(isActiveProvider || hasPersistentConfigHighlight) &&
          "hover:shadow-sm",
        dragHandleProps?.isDragging &&
          "cursor-grabbing border-primary shadow-lg scale-105 z-10",
      )}
    >
      <div
        className={cn(
          "absolute inset-0 bg-gradient-to-r to-transparent transition-opacity duration-500 pointer-events-none",
          shouldUseGreen && "from-emerald-500/10",
          shouldUseBlue && "from-blue-500/10",
          !shouldUseGreen && !shouldUseBlue && "from-primary/10",
          isActiveProvider || hasPersistentConfigHighlight
            ? "opacity-100"
            : "opacity-0",
        )}
      />
      <div className="relative flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex min-w-0 flex-1 items-center gap-2">
          <button
            type="button"
            className={cn(
              "-ml-1.5 flex-shrink-0 cursor-grab active:cursor-grabbing p-1.5",
              "text-muted-foreground/50 hover:text-muted-foreground transition-colors",
              dragHandleProps?.isDragging && "cursor-grabbing",
            )}
            aria-label={t("provider.dragHandle")}
            {...(dragHandleProps?.attributes ?? {})}
            {...(dragHandleProps?.listeners ?? {})}
          >
            <GripVertical className="h-4 w-4" />
          </button>

          <div className="h-8 w-8 flex-shrink-0 rounded-lg bg-muted flex items-center justify-center border border-border group-hover:scale-105 transition-transform duration-300">
            <ProviderIcon
              icon={provider.icon}
              name={provider.name}
              color={provider.iconColor}
              size={20}
            />
          </div>

          <div className="min-w-0 flex-1 space-y-1">
            <div className="flex flex-wrap items-center gap-2 min-h-7">
              <h3 className="text-base font-semibold leading-none">
                {provider.name}
              </h3>

              {isOmo && (
                <span className="inline-flex items-center rounded-md bg-violet-100 px-1.5 py-0.5 text-[10px] font-semibold text-violet-700 dark:bg-violet-900/40 dark:text-violet-300">
                  OMO
                </span>
              )}

              {isOmoSlim && (
                <span className="inline-flex items-center rounded-md bg-indigo-100 px-1.5 py-0.5 text-[10px] font-semibold text-indigo-700 dark:bg-indigo-900/40 dark:text-indigo-300">
                  Slim
                </span>
              )}

              {isProxyRunning && isInFailoverQueue && health && (
                <ProviderHealthBadge
                  consecutiveFailures={health.consecutive_failures}
                  isHealthy={health.is_healthy}
                />
              )}

              {isAutoFailoverEnabled &&
                isInFailoverQueue &&
                failoverPriority && (
                  <FailoverPriorityBadge priority={failoverPriority} />
                )}

              {provider.category === "third_party" &&
                provider.meta?.isPartner && (
                  <span
                    className="text-yellow-500 dark:text-yellow-400"
                    title={t("provider.officialPartner", {
                      defaultValue: "官方合作伙伴",
                    })}
                  >
                    ⭐
                  </span>
                )}

              {isHermesReadOnly && (
                <span
                  className="inline-flex items-center rounded-md bg-slate-200 px-1.5 py-0.5 text-[10px] font-semibold text-slate-700 dark:bg-slate-700/60 dark:text-slate-200"
                  title={t("provider.managedByHermesHint", {
                    defaultValue: "由 Hermes 管理，请在 Hermes Web UI 中编辑",
                  })}
                >
                  {t("provider.managedByHermes", {
                    defaultValue: "Hermes Managed",
                  })}
                </span>
              )}
            </div>

            {displayUrl && (
              <button
                type="button"
                onClick={handleOpenWebsite}
                className={cn(
                  "inline-flex max-w-full items-center overflow-hidden text-left text-sm",
                  isClickableUrl
                    ? "text-blue-500 transition-colors hover:underline dark:text-blue-400 cursor-pointer"
                    : "text-muted-foreground cursor-default",
                )}
                title={displayUrl}
                disabled={!isClickableUrl}
              >
                <span className="min-w-0 truncate">{displayUrl}</span>
              </button>
            )}
          </div>
        </div>

        <div className="flex w-full min-w-0 flex-col gap-2 sm:ml-auto sm:w-auto sm:max-w-[55%]">
          <div className="flex min-w-0 max-w-full flex-wrap items-center justify-end gap-x-1 gap-y-1">
            {quotaSource === "copilot" ? (
              <CopilotQuotaFooter
                meta={provider.meta}
                appId={appId}
                providerId={provider.id}
                inline={true}
                isCurrent={isCurrent}
              />
            ) : quotaSource === "codex_oauth" ? (
              <CodexOauthQuotaFooter
                meta={provider.meta}
                appId={appId}
                providerId={provider.id}
                inline={true}
                isCurrent={isCurrent}
              />
            ) : quotaSource === "claude_oauth" ? (
              <ClaudeOauthQuotaFooter
                meta={provider.meta}
                appId={appId}
                providerId={provider.id}
                inline={true}
                isCurrent={isCurrent}
              />
            ) : quotaSource === "google_gemini_oauth" ? (
              <GeminiOauthQuotaFooter
                meta={provider.meta}
                inline={true}
                appId={appId}
                providerId={provider.id}
                isCurrent={isCurrent}
              />
            ) : quotaSource === "antigravity_oauth" ? (
              <AntigravityOauthQuotaFooter
                meta={provider.meta}
                inline={true}
                appId={appId}
                providerId={provider.id}
                isCurrent={isCurrent}
              />
            ) : quotaSource === "cursor_oauth" ||
              quotaSource === "cursor_apikey" ? (
              <CursorOauthQuotaFooter
                meta={provider.meta}
                inline={true}
                appId={appId}
                providerId={provider.id}
                isCurrent={isCurrent}
              />
            ) : quotaSource === "kiro_oauth" ? (
              <KiroOauthQuotaFooter
                meta={provider.meta}
                inline={true}
                appId={appId}
                providerId={provider.id}
                isCurrent={isCurrent}
              />
            ) : quotaSource === "ollama_cloud" ? (
              <OllamaQuotaFooter
                meta={provider.meta}
                providerId={provider.id}
                appId={appId}
                inline={true}
                isCurrent={isCurrent}
              />
            ) : isOfficial ? (
              officialSubscriptionEnabled ? (
                <SubscriptionQuotaFooter
                  appId={appId}
                  inline={true}
                  isCurrent={isCurrent}
                  autoQueryInterval={
                    provider.meta?.usage_script?.autoQueryInterval ?? 0
                  }
                />
              ) : null
            ) : hasMultiplePlans ? (
              <div className="flex items-center gap-2 text-xs text-gray-600 dark:text-gray-400">
                <span className="font-medium">
                  {t("usage.multiplePlans", {
                    count: usage?.data?.length || 0,
                    defaultValue: `${usage?.data?.length || 0} 个套餐`,
                  })}
                </span>
              </div>
            ) : (
              <UsageFooter
                provider={provider}
                providerId={provider.id}
                appId={appId}
                usageEnabled={usageEnabled}
                isCurrent={isCurrent}
                isInConfig={isInConfig}
                inline={true}
              />
            )}
            {hasMultiplePlans && (
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  setIsExpanded(!isExpanded);
                }}
                className="p-1 rounded hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors text-gray-500 dark:text-gray-400 flex-shrink-0"
                title={
                  isExpanded
                    ? t("usage.collapse", { defaultValue: "收起" })
                    : t("usage.expand", { defaultValue: "展开" })
                }
              >
                {isExpanded ? (
                  <ChevronUp size={14} />
                ) : (
                  <ChevronDown size={14} />
                )}
              </button>
            )}
          </div>

          <div className="flex justify-end opacity-0 pointer-events-none group-hover:opacity-100 group-focus-within:opacity-100 group-hover:pointer-events-auto group-focus-within:pointer-events-auto max-sm:opacity-100 max-sm:pointer-events-auto transition-opacity duration-200">
            <ProviderActions
              appId={appId}
              isCurrent={isCurrent}
              isInConfig={isInConfig}
              isTestingLink={isTestingLink}
              isTestingModel={isTestingModel}
              isProxyTakeover={isProxyTakeover}
              isReadOnly={isHermesReadOnly}
              isOmo={isAnyOmo}
              onSwitch={() => onSwitch(provider)}
              onEdit={() => onEdit(provider)}
              onDuplicate={() => onDuplicate(provider)}
              onTestLink={
                onTestLink && canTestLinkProvider(provider, appId)
                  ? () => onTestLink(provider)
                  : undefined
              }
              onTestModel={
                onTestModel && canTestModelProvider(provider, appId)
                  ? () => onTestModel(provider)
                  : undefined
              }
              onConfigureUsage={
                (isOfficial && !supportsOfficialSubscription) ||
                isManagedOauth ||
                provider.meta?.providerType === PROVIDER_TYPES.OLLAMA_CLOUD
                  ? undefined
                  : () => onConfigureUsage(provider)
              }
              onDelete={() => onDelete(provider)}
              onRemoveFromConfig={
                onRemoveFromConfig
                  ? () => onRemoveFromConfig(provider)
                  : undefined
              }
              onDisableOmo={handleDisableAnyOmo}
              onOpenTerminal={
                onOpenTerminal ? () => onOpenTerminal(provider) : undefined
              }
              isAutoFailoverEnabled={isAutoFailoverEnabled}
              isInFailoverQueue={isInFailoverQueue}
              onToggleFailover={onToggleFailover}
              // OpenClaw: default model
              isDefaultModel={isDefaultModel}
              onSetAsDefault={onSetAsDefault}
            />
          </div>
        </div>
      </div>

      {isExpanded && hasMultiplePlans && (
        <div className="mt-4 pt-4 border-t border-border-default">
          <UsageFooter
            provider={provider}
            providerId={provider.id}
            appId={appId}
            usageEnabled={usageEnabled}
            isCurrent={isCurrent}
            isInConfig={isInConfig}
            inline={false}
          />
        </div>
      )}
    </div>
  );
}
