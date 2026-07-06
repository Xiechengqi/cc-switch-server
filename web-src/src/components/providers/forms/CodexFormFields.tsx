import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { FormLabel } from "@/components/ui/form";
import { Switch } from "@/components/ui/switch";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { toast } from "sonner";
import { ChevronDown, ChevronRight } from "lucide-react";
import EndpointSpeedTest from "./EndpointSpeedTest";
import { CodexOAuthSection } from "./CodexOAuthSection";
import { CursorOAuthSection } from "./CursorOAuthSection";
import { SingleModelMappingField } from "./SingleModelMappingField";
import { ApiKeySection, EndpointField } from "./shared";
import {
  fetchModelsForConfig,
  showFetchModelsError,
  type FetchedModel,
} from "@/lib/api/model-fetch";
import { CustomUserAgentField } from "./CustomUserAgentField";
import { LocalProxyRequestOverridesField } from "./LocalProxyRequestOverridesField";
import { cn } from "@/lib/utils";
import type {
  CodexApiFormat,
  CodexChatReasoning,
  ProviderCategory,
} from "@/types";

interface EndpointCandidate {
  url: string;
}

interface CodexFormFieldsProps {
  providerId?: string;
  // API Key
  codexApiKey: string;
  onApiKeyChange: (key: string) => void;
  category?: ProviderCategory;
  shouldShowApiKeyLink: boolean;
  websiteUrl: string;
  isPartner?: boolean;
  partnerPromotionKey?: string;
  isCodexOfficialPreset?: boolean;
  isCodexOauthAuthenticated?: boolean;
  selectedCodexAccountId?: string | null;
  onCodexAccountSelect?: (accountId: string | null) => void;
  codexImageGenerationEnabled?: boolean;
  onCodexImageGenerationChange?: (enabled: boolean) => void;
  isCursorOauthPreset?: boolean;
  isCursorApiKeyPreset?: boolean;
  selectedCursorAccountId?: string | null;
  onCursorAccountSelect?: (accountId: string | null) => void;

  // Base URL
  shouldShowSpeedTest: boolean;
  codexBaseUrl: string;
  onBaseUrlChange: (url: string) => void;
  isFullUrl: boolean;
  onFullUrlChange: (value: boolean) => void;
  isEndpointModalOpen: boolean;
  onEndpointModalToggle: (open: boolean) => void;
  onCustomEndpointsChange?: (endpoints: string[]) => void;
  autoSelect: boolean;
  onAutoSelectChange: (checked: boolean) => void;

  // API Format
  // Note: wire_api is always "responses" for Codex; apiFormat controls proxy-layer conversion
  apiFormat: CodexApiFormat;
  onApiFormatChange: (format: CodexApiFormat) => void;
  codexChatReasoning?: CodexChatReasoning;
  onCodexChatReasoningChange?: (value: CodexChatReasoning) => void;

  // Model Mapping
  singleUpstreamModel: string;
  onSingleUpstreamModelChange: (value: string) => void;

  // Speed Test Endpoints
  speedTestEndpoints: EndpointCandidate[];

  // Local proxy User-Agent override
  customUserAgent: string;
  onCustomUserAgentChange: (value: string) => void;
  localProxyHeadersOverride: string;
  onLocalProxyHeadersOverrideChange: (value: string) => void;
  localProxyBodyOverride: string;
  onLocalProxyBodyOverrideChange: (value: string) => void;
}

export function CodexFormFields({
  providerId,
  codexApiKey,
  onApiKeyChange,
  category,
  shouldShowApiKeyLink,
  websiteUrl,
  isPartner,
  partnerPromotionKey,
  isCodexOfficialPreset = false,
  isCodexOauthAuthenticated = false,
  selectedCodexAccountId,
  onCodexAccountSelect,
  codexImageGenerationEnabled,
  onCodexImageGenerationChange,
  isCursorOauthPreset = false,
  isCursorApiKeyPreset = false,
  selectedCursorAccountId,
  onCursorAccountSelect,
  shouldShowSpeedTest,
  codexBaseUrl,
  onBaseUrlChange,
  isFullUrl,
  onFullUrlChange,
  isEndpointModalOpen,
  onEndpointModalToggle,
  onCustomEndpointsChange,
  autoSelect,
  onAutoSelectChange,
  apiFormat,
  onApiFormatChange,
  codexChatReasoning = {},
  onCodexChatReasoningChange,
  singleUpstreamModel,
  onSingleUpstreamModelChange,
  speedTestEndpoints,
  customUserAgent,
  onCustomUserAgentChange,
  localProxyHeadersOverride,
  onLocalProxyHeadersOverrideChange,
  localProxyBodyOverride,
  onLocalProxyBodyOverrideChange,
}: CodexFormFieldsProps) {
  const { t } = useTranslation();

  const [fetchedModels, setFetchedModels] = useState<FetchedModel[]>([]);
  const [isFetchingModels, setIsFetchingModels] = useState(false);
  // 思考能力随 Chat 格式显示（仅 Chat Completions 转换路径用得上）；模型映射常驻
  //（填了才生成 catalog）。两者都已与「路由接管」概念解耦。
  const isChatFormat = apiFormat === "openai_chat";
  const needsLocalRouting = isChatFormat;
  const canEditReasoning = Boolean(onCodexChatReasoningChange);
  const supportsThinking =
    codexChatReasoning.supportsThinking === true ||
    codexChatReasoning.supportsEffort === true;
  const supportsEffort = codexChatReasoning.supportsEffort === true;
  const shouldShowModelMapping = needsLocalRouting && !isCodexOfficialPreset;

  // 高级区在有任何可见配置时自动展开（仅折叠→展开，不会自动折叠）：自定义 UA /
  // 请求覆盖 / 已填模型映射 / 原生 Responses（需维护 catalog）/ 已配置思考能力。
  const hasRequestOverrides = Boolean(
    localProxyHeadersOverride.trim() || localProxyBodyOverride.trim(),
  );
  const hasAnyAdvancedValue =
    !!customUserAgent ||
    hasRequestOverrides ||
    apiFormat === "openai_responses" ||
    supportsThinking ||
    supportsEffort;
  const [advancedExpanded, setAdvancedExpanded] = useState(hasAnyAdvancedValue);

  // 预设/编辑加载填充高级值后自动展开（仅从折叠→展开，不会自动折叠）
  useEffect(() => {
    if (hasAnyAdvancedValue) {
      setAdvancedExpanded(true);
    }
  }, [hasAnyAdvancedValue]);

  const handleReasoningThinkingChange = useCallback(
    (checked: boolean) => {
      if (!onCodexChatReasoningChange) return;
      onCodexChatReasoningChange({
        ...codexChatReasoning,
        supportsThinking: checked,
        supportsEffort: checked ? codexChatReasoning.supportsEffort : false,
      });
    },
    [codexChatReasoning, onCodexChatReasoningChange],
  );

  const handleReasoningEffortChange = useCallback(
    (checked: boolean) => {
      if (!onCodexChatReasoningChange) return;
      onCodexChatReasoningChange({
        ...codexChatReasoning,
        supportsThinking: checked ? true : codexChatReasoning.supportsThinking,
        supportsEffort: checked,
        effortParam: checked
          ? (codexChatReasoning.effortParam ?? "reasoning_effort")
          : "none",
      });
    },
    [codexChatReasoning, onCodexChatReasoningChange],
  );

  const handleFetchModels = useCallback(() => {
    if (!codexBaseUrl || !codexApiKey) {
      showFetchModelsError(null, t, {
        hasApiKey: !!codexApiKey,
        hasBaseUrl: !!codexBaseUrl,
      });
      return;
    }
    setIsFetchingModels(true);
    fetchModelsForConfig(
      codexBaseUrl,
      codexApiKey,
      isFullUrl,
      undefined,
      customUserAgent,
    )
      .then((models) => {
        setFetchedModels(models);
        if (models.length === 0) {
          toast.info(t("providerForm.fetchModelsEmpty"));
        } else {
          toast.success(
            t("providerForm.fetchModelsSuccess", { count: models.length }),
          );
        }
      })
      .catch((err) => {
        console.warn("[ModelFetch] Failed:", err);
        showFetchModelsError(err, t);
      })
      .finally(() => setIsFetchingModels(false));
  }, [codexBaseUrl, codexApiKey, isFullUrl, customUserAgent, t]);

  return (
    <>
      {isCodexOfficialPreset && !isCursorOauthPreset && (
        <CodexOAuthSection
          selectedAccountId={selectedCodexAccountId}
          onAccountSelect={onCodexAccountSelect}
          allowDefaultAccountOption={false}
          imageGenerationEnabled={codexImageGenerationEnabled}
          onImageGenerationChange={onCodexImageGenerationChange}
          showBankedResetPanel
        />
      )}

      {isCursorOauthPreset && (
        <CursorOAuthSection
          selectedAccountId={selectedCursorAccountId}
          onAccountSelect={onCursorAccountSelect}
        />
      )}

      {/* Codex API Key 输入框 */}
      {!isCursorOauthPreset && (
        <ApiKeySection
          id="codexApiKey"
          label="API Key"
          value={codexApiKey}
          onChange={onApiKeyChange}
          category={isCursorApiKeyPreset ? "third_party" : category}
          shouldShowLink={shouldShowApiKeyLink}
          websiteUrl={websiteUrl}
          isPartner={isPartner}
          partnerPromotionKey={partnerPromotionKey}
          placeholder={{
            official: t("providerForm.codexOfficialNoApiKey", {
              defaultValue: "官方供应商无需 API Key",
            }),
            thirdParty: t("providerForm.codexApiKeyAutoFill", {
              defaultValue: "输入 API Key，将自动填充到配置",
            }),
          }}
        />
      )}

      {isCodexOfficialPreset &&
        !isCursorOauthPreset &&
        !isCodexOauthAuthenticated && (
          <p className="text-xs text-destructive">
            {t("codexOauth.loginRequired", {
              defaultValue: "请先登录 ChatGPT 账号",
            })}
          </p>
        )}

      {/* Codex Base URL 输入框 */}
      {shouldShowSpeedTest && !isCodexOfficialPreset && (
        <EndpointField
          id="codexBaseUrl"
          label={t("codexConfig.apiUrlLabel")}
          value={codexBaseUrl}
          onChange={onBaseUrlChange}
          placeholder={t("providerForm.codexApiEndpointPlaceholder")}
          hint={t("providerForm.codexApiHint")}
          showFullUrlToggle
          isFullUrl={isFullUrl}
          onFullUrlChange={onFullUrlChange}
          onManageClick={() => onEndpointModalToggle(true)}
        />
      )}

      {/* 高级选项 —— 本地路由映射/模型映射/思考能力/自定义 UA；预设供应商通常无需展开 */}
      {(category !== "official" ||
        isCursorApiKeyPreset ||
        isCursorOauthPreset) && (
        <Collapsible
          open={advancedExpanded}
          onOpenChange={setAdvancedExpanded}
          className="rounded-lg border border-border-default p-4"
        >
          <CollapsibleTrigger asChild>
            <Button
              type="button"
              variant={null}
              size="sm"
              className="h-8 w-full justify-start gap-1.5 px-0 text-sm font-medium text-foreground hover:opacity-70"
            >
              {advancedExpanded ? (
                <ChevronDown className="h-4 w-4" />
              ) : (
                <ChevronRight className="h-4 w-4" />
              )}
              {t("providerForm.advancedOptionsToggle", {
                defaultValue: "高级选项",
              })}
            </Button>
          </CollapsibleTrigger>
          {!advancedExpanded && (
            <p className="mt-1 ml-1 text-xs text-muted-foreground">
              {t("codexConfig.advancedSectionHint", {
                defaultValue:
                  "包含上游格式、模型映射、思考能力与自定义 User-Agent。使用 Chat Completions 协议的供应商需开启路由接管才能使用。",
              })}
            </p>
          )}
          <CollapsibleContent className="space-y-3 pt-3">
            {/* 本地路由映射开关 —— 沿用 shouldShowSpeedTest 门控，cloud_provider 保持不可切换 */}
            {(shouldShowSpeedTest ||
              isCursorApiKeyPreset ||
              isCursorOauthPreset) && (
              <div className="flex items-center justify-between gap-4">
                <div className="space-y-1">
                  <FormLabel>
                    {t("codexConfig.localRoutingToggle", {
                      defaultValue: "需要本地路由映射",
                    })}
                  </FormLabel>
                  <Select
                    value={apiFormat}
                    onValueChange={(value) =>
                      onApiFormatChange(value as CodexApiFormat)
                    }
                  >
                    <SelectTrigger
                      id="codex-upstream-format"
                      className="w-full"
                    >
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="openai_chat">
                        {t("codexConfig.upstreamFormatChat", {
                          defaultValue: "Chat Completions（需开启路由）",
                        })}
                      </SelectItem>
                      <SelectItem value="openai_responses">
                        {t("codexConfig.upstreamFormatResponses", {
                          defaultValue: "Responses（原生）",
                        })}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                  <p className="text-xs leading-relaxed text-muted-foreground">
                    {t("codexConfig.upstreamFormatHint", {
                      defaultValue:
                        "供应商原生是 Responses API 就选 Responses（直连，不转换格式）；使用 Chat Completions 协议就选 Chat（需开启路由接管才能转换为 Chat Completions）。",
                    })}
                  </p>
                </div>
              </div>
            )}

            {isChatFormat && canEditReasoning && (
              <div
                className={cn(
                  "space-y-3",
                  shouldShowSpeedTest && "border-t border-border-default pt-3",
                )}
              >
                <div className="space-y-1">
                  <FormLabel>
                    {t("codexConfig.reasoningGroupTitle", {
                      defaultValue: "思考能力",
                    })}
                  </FormLabel>
                  <p className="text-xs leading-relaxed text-muted-foreground">
                    {t("codexConfig.reasoningSectionHint", {
                      defaultValue:
                        "预设供应商已自动配置；自定义供应商会按名称/地址自动推断。仅当自动识别不准时才需手动覆盖。",
                    })}
                  </p>
                </div>

                <div className="flex items-center justify-between gap-4">
                  <div className="space-y-1">
                    <FormLabel>
                      {t("codexConfig.reasoningModeToggle", {
                        defaultValue: "支持思考模式",
                      })}
                    </FormLabel>
                    <p className="text-xs leading-relaxed text-muted-foreground">
                      {t("codexConfig.reasoningModeHint", {
                        defaultValue:
                          "上游 Chat Completions 接口支持开启或关闭 thinking 时启用。Kimi、GLM、Qwen 等通常属于这一类。",
                      })}
                    </p>
                  </div>
                  <Switch
                    checked={supportsThinking}
                    onCheckedChange={handleReasoningThinkingChange}
                    aria-label={t("codexConfig.reasoningModeToggle", {
                      defaultValue: "支持思考模式",
                    })}
                  />
                </div>

                <div className="flex items-center justify-between gap-4 border-t border-border-default pt-3">
                  <div className="space-y-1">
                    <FormLabel>
                      {t("codexConfig.reasoningEffortToggle", {
                        defaultValue: "支持思考等级",
                      })}
                    </FormLabel>
                    <p className="text-xs leading-relaxed text-muted-foreground">
                      {t("codexConfig.reasoningEffortHint", {
                        defaultValue:
                          "上游支持 low/high/max 等思考深度控制时启用。启用后会自动启用思考模式，并把 Codex 的 reasoning.effort 转成上游 Chat 参数。",
                      })}
                    </p>
                  </div>
                  <Switch
                    checked={supportsEffort}
                    onCheckedChange={handleReasoningEffortChange}
                    aria-label={t("codexConfig.reasoningEffortToggle", {
                      defaultValue: "支持思考等级",
                    })}
                  />
                </div>
              </div>
            )}

            <div
              className={cn(
                "space-y-3",
                (shouldShowSpeedTest ||
                  (needsLocalRouting && isChatFormat && canEditReasoning)) &&
                  "border-t border-border-default pt-3",
              )}
            >
              <CustomUserAgentField
                id="codex-custom-user-agent"
                value={customUserAgent}
                onChange={onCustomUserAgentChange}
              />
              <div className="border-t border-border-default pt-3">
                <LocalProxyRequestOverridesField
                  headersJson={localProxyHeadersOverride}
                  bodyJson={localProxyBodyOverride}
                  onHeadersJsonChange={onLocalProxyHeadersOverrideChange}
                  onBodyJsonChange={onLocalProxyBodyOverrideChange}
                />
              </div>
            </div>

            {/* 模型映射：所有客户端请求模型统一转发到同一个上游真实模型 */}
            {shouldShowModelMapping && (
              <div className="border-t border-border-default pt-3">
                <SingleModelMappingField
                  id="codexSingleUpstreamModel"
                  value={singleUpstreamModel}
                  onChange={onSingleUpstreamModelChange}
                  fetchedModels={fetchedModels}
                  isLoading={isFetchingModels}
                  onFetchModels={handleFetchModels}
                />
              </div>
            )}

            <div
              className={cn(
                "space-y-3",
                (shouldShowSpeedTest ||
                  (isChatFormat && canEditReasoning)) &&
                  "border-t border-border-default pt-3",
              )}
            >
              <CustomUserAgentField
                id="codex-custom-user-agent"
                value={customUserAgent}
                onChange={onCustomUserAgentChange}
              />
              <div className="border-t border-border-default pt-3">
                <LocalProxyRequestOverridesField
                  headersJson={localProxyHeadersOverride}
                  bodyJson={localProxyBodyOverride}
                  onHeadersJsonChange={onLocalProxyHeadersOverrideChange}
                  onBodyJsonChange={onLocalProxyBodyOverrideChange}
                />
              </div>
            </div>
          </CollapsibleContent>
        </Collapsible>
      )}

      {/* 端点测速弹窗 - Codex */}
      {shouldShowSpeedTest && !isCodexOfficialPreset && isEndpointModalOpen && (
        <EndpointSpeedTest
          appId="codex"
          providerId={providerId}
          value={codexBaseUrl}
          onChange={onBaseUrlChange}
          initialEndpoints={speedTestEndpoints}
          visible={isEndpointModalOpen}
          onClose={() => onEndpointModalToggle(false)}
          autoSelect={autoSelect}
          onAutoSelectChange={onAutoSelectChange}
          onCustomEndpointsChange={onCustomEndpointsChange}
        />
      )}
    </>
  );
}
