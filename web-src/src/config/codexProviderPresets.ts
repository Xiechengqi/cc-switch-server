/**
 * Codex 预设供应商配置模板
 */
import { ProviderCategory, ProviderTestConfig } from "../types";
import type {
  CodexApiFormat,
  CodexCatalogModel,
  CodexChatReasoning,
  SingleModelMapping,
} from "../types";
import type { PresetTheme } from "./claudeProviderPresets";

export interface CodexProviderPreset {
  name: string;
  nameKey?: string; // i18n key for localized display name
  websiteUrl: string;
  // 第三方供应商可提供单独的获取 API Key 链接
  apiKeyUrl?: string;
  auth: Record<string, any>; // 将写入 ~/.codex/auth.json
  config: string; // 将写入 ~/.codex/config.toml（TOML 字符串）
  isOfficial?: boolean; // 标识是否为官方预设
  isPartner?: boolean; // 标识是否为商业合作伙伴
  primePartner?: boolean; // 置顶合作伙伴（顶级）：徽章显示为心形
  partnerPromotionKey?: string; // 合作伙伴促销信息的 i18n key
  category?: ProviderCategory; // 新增：分类
  isCustomTemplate?: boolean; // 标识是否为自定义模板
  // 新增：请求地址候选列表（用于地址管理/测速）
  endpointCandidates?: string[];
  // 新增：视觉主题配置
  theme?: PresetTheme;
  // 图标配置
  icon?: string; // 图标名称
  iconColor?: string; // 图标颜色
  // Codex API 格式
  apiFormat?: CodexApiFormat;
  // 特殊供应商类型
  providerType?:
    | "codex_oauth"
    | "cursor_oauth"
    | "cursor_apikey"
    | "ollama_cloud";
  requiresOAuth?: boolean;
  // Codex Chat 本地路由模式下的模型目录
  modelCatalog?: CodexCatalogModel[];
  modelMapping?: SingleModelMapping;
  // Codex Responses -> Chat Completions reasoning capability defaults
  codexChatReasoning?: CodexChatReasoning;
  // 供应商单独的模型测试配置（预设默认值，创建时初始化）
  testConfig?: ProviderTestConfig;
}

/**
 * 生成第三方供应商的 auth.json
 */
export function generateThirdPartyAuth(apiKey: string): Record<string, any> {
  return {
    OPENAI_API_KEY: apiKey || "",
  };
}

/**
 * 生成第三方供应商的 config.toml
 */
export function generateThirdPartyConfig(
  providerName: string,
  baseUrl: string,
  modelName = "gpt-5.5",
): string {
  const tomlString = (value: string) => JSON.stringify(value);

  return `model_provider = "custom"
model = ${tomlString(modelName)}
model_reasoning_effort = "high"
disable_response_storage = true

[model_providers.custom]
name = ${tomlString(providerName)}
base_url = ${tomlString(baseUrl)}
wire_api = "responses"
requires_openai_auth = true`;
}

function modelCatalog(
  models: Array<
    | string
    | {
        model: string;
        upstreamModel?: string;
        displayName?: string;
        contextWindow?: number;
      }
  >,
): CodexCatalogModel[] {
  return models.map((entry) =>
    typeof entry === "string"
      ? { model: entry }
      : {
          model: entry.model,
          upstreamModel: entry.upstreamModel,
          displayName: entry.displayName,
          contextWindow: entry.contextWindow,
        },
  );
}

const CURSOR_CODEX_UPSTREAM_MODEL = "composer-2.5";
const cursorCodexModelCatalog = modelCatalog([
  {
    model: "gpt-5.5",
    upstreamModel: CURSOR_CODEX_UPSTREAM_MODEL,
    displayName: "GPT-5.5",
    contextWindow: 128000,
  },
  {
    model: "gpt-5.5-low",
    upstreamModel: CURSOR_CODEX_UPSTREAM_MODEL,
    displayName: "GPT-5.5 Low",
    contextWindow: 128000,
  },
  {
    model: "gpt-5.5-medium",
    upstreamModel: CURSOR_CODEX_UPSTREAM_MODEL,
    displayName: "GPT-5.5 Medium",
    contextWindow: 128000,
  },
  {
    model: "gpt-5.5-high",
    upstreamModel: CURSOR_CODEX_UPSTREAM_MODEL,
    displayName: "GPT-5.5 High",
    contextWindow: 128000,
  },
  {
    model: "gpt-5.5-xhigh",
    upstreamModel: CURSOR_CODEX_UPSTREAM_MODEL,
    displayName: "GPT-5.5 XHigh",
    contextWindow: 128000,
  },
  {
    model: "gpt-5.5-minimal",
    upstreamModel: CURSOR_CODEX_UPSTREAM_MODEL,
    displayName: "GPT-5.5 Minimal",
    contextWindow: 128000,
  },
  {
    model: "gpt-5.4",
    upstreamModel: CURSOR_CODEX_UPSTREAM_MODEL,
    displayName: "GPT-5.4",
    contextWindow: 128000,
  },
  {
    model: "gpt-5.4-mini",
    upstreamModel: CURSOR_CODEX_UPSTREAM_MODEL,
    displayName: "GPT-5.4 Mini",
    contextWindow: 128000,
  },
  {
    model: "gpt-5.4-nano",
    upstreamModel: CURSOR_CODEX_UPSTREAM_MODEL,
    displayName: "GPT-5.4 Nano",
    contextWindow: 128000,
  },
]);

export const codexProviderPresets: CodexProviderPreset[] = [
  {
    name: "OpenAI OAuth",
    websiteUrl: "https://chatgpt.com/codex",
    isOfficial: true,
    category: "official",
    auth: {},
    config: `model = "gpt-5.5"`,
    providerType: "codex_oauth",
    theme: {
      icon: "codex",
      backgroundColor: "#1F2937", // gray-800
      textColor: "#FFFFFF",
    },
    icon: "openai",
    iconColor: "#00A67E",
  },
  {
    name: "Cursor API Key",
    websiteUrl: "https://cursor.com/dashboard/cloud-agents",
    apiKeyUrl: "https://cursor.com/dashboard/cloud-agents",
    isOfficial: true,
    category: "official",
    auth: generateThirdPartyAuth(""),
    config: generateThirdPartyConfig(
      "cursor",
      "https://api.cursor.com",
      "gpt-5.5",
    ),
    modelMapping: {
      mode: "single",
      upstreamModel: "composer-2.5",
    },
    providerType: "cursor_apikey",
    apiFormat: "openai_chat",
    modelCatalog: cursorCodexModelCatalog,
    theme: {
      icon: "codex",
      backgroundColor: "#111111",
      textColor: "#FFFFFF",
    },
    icon: "cursor",
  },
  {
    name: "Cursor OAuth",
    websiteUrl: "https://cursor.com",
    isOfficial: true,
    category: "official",
    auth: {},
    config: generateThirdPartyConfig(
      "cursor",
      "https://api2.cursor.sh",
      "gpt-5.5",
    ),
    modelMapping: {
      mode: "single",
      upstreamModel: "composer-2.5",
    },
    providerType: "cursor_oauth",
    requiresOAuth: true,
    apiFormat: "openai_chat",
    modelCatalog: cursorCodexModelCatalog,
    theme: {
      icon: "codex",
      backgroundColor: "#111111",
      textColor: "#FFFFFF",
    },
    icon: "cursor",
  },
  {
    name: "Ollama API Key",
    websiteUrl: "https://ollama.com",
    isOfficial: false,
    category: "third_party",
    auth: generateThirdPartyAuth(""),
    config: generateThirdPartyConfig(
      "ollama",
      "https://ollama.com",
      "kimi-k2.7-code",
    ),
    modelMapping: {
      mode: "single",
      upstreamModel: "kimi-k2.7-code",
    },
    providerType: "ollama_cloud",
    requiresOAuth: false,
    apiFormat: "openai_chat",
    modelCatalog: modelCatalog([
      {
        model: "kimi-k2.7-code",
        displayName: "Kimi K2.7 Code",
        contextWindow: 262144,
      },
    ]),
    codexChatReasoning: {
      supportsThinking: true,
      supportsEffort: true,
      thinkingParam: "thinking",
      effortParam: "reasoning_effort",
      outputFormat: "reasoning_content",
    },
    theme: {
      icon: "codex",
      backgroundColor: "#111111",
      textColor: "#FFFFFF",
    },
    testConfig: {
      enabled: true,
      testModel: "gpt-oss:20b",
    },
    icon: "ollama",
  },
  {
    name: "OpenRouter",
    websiteUrl: "https://openrouter.ai",
    apiKeyUrl: "https://openrouter.ai/keys",
    auth: generateThirdPartyAuth(""),
    config: generateThirdPartyConfig(
      "openrouter",
      "https://openrouter.ai/api/v1",
      "gpt-5.4",
    ),
    category: "aggregator",
    icon: "openrouter",
    iconColor: "#6566F1",
  },
  {
    name: "Nvidia",
    websiteUrl: "https://build.nvidia.com",
    apiKeyUrl: "https://build.nvidia.com/settings/api-keys",
    auth: generateThirdPartyAuth(""),
    config: generateThirdPartyConfig(
      "nvidia",
      "https://integrate.api.nvidia.com/v1",
      "moonshotai/kimi-k2.5",
    ),
    endpointCandidates: ["https://integrate.api.nvidia.com/v1"],
    apiFormat: "openai_chat",
    modelCatalog: modelCatalog([
      {
        model: "moonshotai/kimi-k2.5",
        displayName: "Kimi K2.5",
        contextWindow: 262144,
      },
    ]),
    codexChatReasoning: {
      supportsThinking: true,
      supportsEffort: false,
      thinkingParam: "thinking",
      effortParam: "none",
      outputFormat: "reasoning_content",
    },
    category: "aggregator",
    icon: "nvidia",
    iconColor: "#000000",
  },
  {
    name: "DeepSeek(API Key)",
    websiteUrl: "https://platform.deepseek.com",
    apiKeyUrl: "https://platform.deepseek.com/api_keys",
    auth: generateThirdPartyAuth(""),
    config: generateThirdPartyConfig(
      "deepseek",
      "https://api.deepseek.com",
      "deepseek-v4-flash",
    ),
    endpointCandidates: ["https://api.deepseek.com"],
    apiFormat: "openai_chat",
    modelCatalog: modelCatalog([
      {
        model: "deepseek-v4-flash",
        displayName: "DeepSeek V4 Flash",
        contextWindow: 1000000,
      },
      {
        model: "deepseek-v4-pro",
        displayName: "DeepSeek V4 Pro",
        contextWindow: 1000000,
      },
    ]),
    codexChatReasoning: {
      supportsThinking: true,
      supportsEffort: true,
      thinkingParam: "thinking",
      effortParam: "reasoning_effort",
      effortValueMode: "deepseek",
      outputFormat: "reasoning_content",
    },
    category: "cn_official",
    icon: "deepseek",
    iconColor: "#1E88E5",
  },
];
