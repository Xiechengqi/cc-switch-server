import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Info, ChevronDown, ChevronRight } from "lucide-react";
import { Button } from "@/components/ui/button";
import { toast } from "sonner";
import EndpointSpeedTest from "./EndpointSpeedTest";
import { AntigravityOAuthSection } from "./AntigravityOAuthSection";
import GeminiOAuthSection from "./GeminiOAuthSection";
import { ApiKeySection, EndpointField } from "./shared";
import { SingleModelMappingField } from "./SingleModelMappingField";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  fetchModelsForConfig,
  showFetchModelsError,
  type FetchedModel,
} from "@/lib/api/model-fetch";
import type { ProviderCategory } from "@/types";

interface EndpointCandidate {
  url: string;
}

interface GeminiFormFieldsProps {
  providerId?: string;
  // API Key
  shouldShowApiKey: boolean;
  apiKey: string;
  onApiKeyChange: (key: string) => void;
  category?: ProviderCategory;
  shouldShowApiKeyLink: boolean;
  websiteUrl: string;
  isPartner?: boolean;
  partnerPromotionKey?: string;
  isGeminiOfficialPreset?: boolean;
  isGeminiOauthAuthenticated?: boolean;
  selectedGeminiAccountId?: string | null;
  onGeminiAccountSelect?: (accountId: string | null) => void;
  isAntigravityOauthPreset?: boolean;
  isAntigravityOauthAuthenticated?: boolean;
  selectedAntigravityAccountId?: string | null;
  onAntigravityAccountSelect?: (accountId: string | null) => void;

  // Base URL
  shouldShowSpeedTest: boolean;
  baseUrl: string;
  onBaseUrlChange: (url: string) => void;
  isEndpointModalOpen: boolean;
  onEndpointModalToggle: (open: boolean) => void;
  onCustomEndpointsChange: (endpoints: string[]) => void;
  autoSelect: boolean;
  onAutoSelectChange: (checked: boolean) => void;

  // Model
  shouldShowModelField: boolean;
  model: string;
  onModelChange: (value: string) => void;

  // Speed Test Endpoints
  speedTestEndpoints: EndpointCandidate[];
}

export function GeminiFormFields({
  providerId,
  shouldShowApiKey,
  apiKey,
  onApiKeyChange,
  category,
  shouldShowApiKeyLink,
  websiteUrl,
  isPartner,
  partnerPromotionKey,
  isGeminiOfficialPreset = false,
  isGeminiOauthAuthenticated = false,
  selectedGeminiAccountId,
  onGeminiAccountSelect,
  isAntigravityOauthPreset = false,
  isAntigravityOauthAuthenticated = false,
  selectedAntigravityAccountId,
  onAntigravityAccountSelect,
  shouldShowSpeedTest,
  baseUrl,
  onBaseUrlChange,
  isEndpointModalOpen,
  onEndpointModalToggle,
  onCustomEndpointsChange,
  autoSelect,
  onAutoSelectChange,
  shouldShowModelField,
  model,
  onModelChange,
  speedTestEndpoints,
}: GeminiFormFieldsProps) {
  const { t } = useTranslation();

  const [fetchedModels, setFetchedModels] = useState<FetchedModel[]>([]);
  const [isFetchingModels, setIsFetchingModels] = useState(false);
  const [advancedExpanded, setAdvancedExpanded] = useState(
    Boolean(shouldShowModelField && model),
  );

  useEffect(() => {
    if (shouldShowModelField && model) {
      setAdvancedExpanded(true);
    }
  }, [model, shouldShowModelField]);

  const handleFetchModels = useCallback(() => {
    if (!baseUrl || !apiKey) {
      showFetchModelsError(null, t, {
        hasApiKey: !!apiKey,
        hasBaseUrl: !!baseUrl,
      });
      return;
    }
    setIsFetchingModels(true);
    fetchModelsForConfig(baseUrl, apiKey)
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
  }, [baseUrl, apiKey, t]);

  // Official OAuth presets are identified explicitly by providerType metadata.
  const isGoogleOfficial = isGeminiOfficialPreset;
  const isAntigravityOfficial = isAntigravityOauthPreset;
  const usesManagedOAuth = isGoogleOfficial || isAntigravityOfficial;

  return (
    <>
      {/* Google OAuth 提示 */}
      {isGoogleOfficial && (
        <>
          <div className="rounded-lg border border-blue-200 bg-blue-50 p-4 dark:border-blue-800 dark:bg-blue-950">
            <div className="flex gap-3">
              <Info className="h-5 w-5 flex-shrink-0 text-blue-600 dark:text-blue-400" />
              <div className="space-y-1">
                <p className="text-sm font-medium text-blue-900 dark:text-blue-100">
                  {t("provider.form.gemini.oauthTitle", {
                    defaultValue: "OAuth 认证模式",
                  })}
                </p>
                <p className="text-sm text-blue-700 dark:text-blue-300">
                  {t("provider.form.gemini.oauthHint", {
                    defaultValue:
                      "Google Official 使用 cc-switch 托管的 OAuth 账号，无需填写 API Key。",
                  })}
                </p>
              </div>
            </div>
          </div>

          <GeminiOAuthSection
            selectedAccountId={selectedGeminiAccountId}
            onAccountSelect={onGeminiAccountSelect}
            allowDefaultAccountOption={false}
          />

          {!isGeminiOauthAuthenticated && (
            <p className="text-sm text-amber-600 dark:text-amber-400">
              {t("geminiOauth.loginRequired", {
                defaultValue: "请先登录 Google Gemini 账号",
              })}
            </p>
          )}
        </>
      )}

      {isAntigravityOfficial && (
        <>
          <div className="rounded-lg border border-blue-200 bg-blue-50 p-4 dark:border-blue-800 dark:bg-blue-950">
            <div className="flex gap-3">
              <Info className="h-5 w-5 flex-shrink-0 text-blue-600 dark:text-blue-400" />
              <div className="space-y-1">
                <p className="text-sm font-medium text-blue-900 dark:text-blue-100">
                  {t("provider.form.gemini.antigravityOauthTitle", {
                    defaultValue: "Antigravity OAuth 认证模式",
                  })}
                </p>
                <p className="text-sm text-blue-700 dark:text-blue-300">
                  {t("provider.form.gemini.antigravityOauthHint", {
                    defaultValue:
                      "Antigravity OAuth 使用 cc-switch 托管的 Antigravity 账号，无需填写 API Key。",
                  })}
                </p>
              </div>
            </div>
          </div>

          <AntigravityOAuthSection
            selectedAccountId={selectedAntigravityAccountId}
            onAccountSelect={onAntigravityAccountSelect}
            allowDefaultAccountOption={false}
          />

          {!isAntigravityOauthAuthenticated && (
            <p className="text-sm text-amber-600 dark:text-amber-400">
              {t("antigravityOauth.loginRequired", {
                defaultValue: "请先登录 Antigravity 账号",
              })}
            </p>
          )}
        </>
      )}

      {/* API Key 输入框 */}
      {shouldShowApiKey && !usesManagedOAuth && (
        <ApiKeySection
          value={apiKey}
          onChange={onApiKeyChange}
          category={category}
          shouldShowLink={shouldShowApiKeyLink}
          websiteUrl={websiteUrl}
          isPartner={isPartner}
          partnerPromotionKey={partnerPromotionKey}
        />
      )}

      {/* Base URL 输入框（统一使用与 Codex 相同的样式与交互） */}
      {shouldShowSpeedTest && (
        <EndpointField
          id="baseUrl"
          label={t("providerForm.apiEndpoint", { defaultValue: "API 端点" })}
          value={baseUrl}
          onChange={onBaseUrlChange}
          placeholder={t("providerForm.apiEndpointPlaceholder", {
            defaultValue: "https://your-api-endpoint.com/",
          })}
          onManageClick={() => onEndpointModalToggle(true)}
        />
      )}

      {/* Model 映射配置 */}
      {shouldShowModelField && (
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
              {t("providerForm.advancedOptionsHint", {
                defaultValue: "包含模型映射等配置。大多数场景下保持默认即可。",
              })}
            </p>
          )}
          <CollapsibleContent className="pt-3">
            <SingleModelMappingField
              id="gemini-model"
              value={model}
              onChange={onModelChange}
              placeholder="gemini-3.5-flash"
              fetchedModels={fetchedModels}
              isLoading={isFetchingModels}
              onFetchModels={handleFetchModels}
            />
          </CollapsibleContent>
        </Collapsible>
      )}

      {/* 端点测速弹窗 */}
      {shouldShowSpeedTest && isEndpointModalOpen && (
        <EndpointSpeedTest
          appId="gemini"
          providerId={providerId}
          value={baseUrl}
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
