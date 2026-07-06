/**
 * 预设供应商配置模板
 */
import { ProviderCategory, ProviderTestConfig } from "../types";

export interface TemplateValueConfig {
  label: string;
  placeholder: string;
  defaultValue?: string;
  editorValue: string;
}

/**
 * 预设供应商的视觉主题配置
 */
export interface PresetTheme {
  /** 图标类型：'claude' | 'codex' | 'gemini' | 'deepseek' | 'generic' */
  icon?: "claude" | "codex" | "gemini" | "deepseek" | "generic";
  /** 背景色（选中状态），支持 Tailwind 类名或 hex 颜色 */
  backgroundColor?: string;
  /** 文字色（选中状态），支持 Tailwind 类名或 hex 颜色 */
  textColor?: string;
}

export interface ProviderPreset {
  name: string;
  nameKey?: string; // i18n key for localized display name
  websiteUrl: string;
  // 新增：第三方/聚合等可单独配置获取 API Key 的链接
  apiKeyUrl?: string;
  settingsConfig: object;
  isOfficial?: boolean; // 标识是否为官方预设
  isPartner?: boolean; // 标识是否为商业合作伙伴
  primePartner?: boolean; // 置顶合作伙伴（顶级）：徽章显示为心形
  partnerPromotionKey?: string; // 合作伙伴促销信息的 i18n key
  category?: ProviderCategory; // 新增：分类
  // 新增：指定该预设所使用的 API Key 字段名（默认 ANTHROPIC_AUTH_TOKEN）
  apiKeyField?: "ANTHROPIC_AUTH_TOKEN" | "ANTHROPIC_API_KEY";
  // 新增：模板变量定义，用于动态替换配置中的值
  templateValues?: Record<string, TemplateValueConfig>; // editorValue 存储编辑器中的实时输入值
  // 新增：请求地址候选列表（用于地址管理/测速）
  endpointCandidates?: string[];
  // 新增：视觉主题配置
  theme?: PresetTheme;
  // 图标配置
  icon?: string; // 图标名称
  iconColor?: string; // 图标颜色

  // Claude API 格式（仅 Claude 供应商使用）
  // - "anthropic" (默认): Anthropic Messages API 格式，直接透传
  // - "openai_chat": OpenAI Chat Completions 格式，需要格式转换
  // - "openai_responses": OpenAI Responses API 格式，需要格式转换
  // - "gemini_native": Gemini Native generateContent API 格式，需要格式转换
  apiFormat?:
    | "anthropic"
    | "openai_chat"
    | "openai_responses"
    | "gemini_native";

  // 供应商类型标识（用于特殊供应商检测）
  // - "github_copilot": GitHub Copilot 供应商（需要 OAuth 认证）
  // - "codex_oauth": OpenAI Codex via ChatGPT Plus/Pro 反代（需要 OAuth 认证）
  // - "claude_oauth": Claude 官方订阅 OAuth（Anthropic 官方）
  // - "cursor_oauth": Cursor OAuth 订阅反代 Anthropic/Codex API
  // - "cursor_apikey": Cursor API Key 反代 Anthropic/Codex API
  // - "kiro_oauth": Kiro OAuth 账号反代 Anthropic API
  // - "deepseek_account": DeepSeek 账号
  // - "ollama_cloud": Ollama API Key OpenAI-compatible API
  providerType?:
    | "github_copilot"
    | "codex_oauth"
    | "claude_oauth"
    | "google_gemini_oauth"
    | "antigravity_oauth"
    | "agy_oauth"
    | "cursor_oauth"
    | "cursor_apikey"
    | "kiro_oauth"
    | "deepseek_account"
    | "ollama_cloud";

  // 是否需要 OAuth 认证（而非 API Key）
  requiresOAuth?: boolean;

  // 是否在 UI 中隐藏该预设（预设仍存在，仅不在列表中显示）
  hidden?: boolean;

  // 获取模型列表使用的完整 URL（覆写自动候选逻辑）
  // 缺省时后端基于 baseURL 自动尝试 /v1/models、/models 以及剥离已知兼容子路径后的变体。
  modelsUrl?: string;
  // 供应商单独的模型测试配置（预设默认值，创建时初始化）
  testConfig?: ProviderTestConfig;
}

export const providerPresets: ProviderPreset[] = [
  {
    name: "Claude Official",
    websiteUrl: "https://www.anthropic.com/claude-code",
    settingsConfig: {
      env: {},
    },
    isOfficial: true, // 明确标识为官方预设
    category: "official",
    providerType: "claude_oauth",
    requiresOAuth: true,
    theme: {
      icon: "claude",
      backgroundColor: "#D97757",
      textColor: "#FFFFFF",
    },
    icon: "anthropic",
    iconColor: "#D4915D",
  },
  {
    name: "OpenAI OAuth",
    websiteUrl: "https://chatgpt.com/codex",
    settingsConfig: {
      env: {
        // base_url 由代理后端强制重写为 chatgpt.com/backend-api/codex
        // 用户无需配置
        ANTHROPIC_BASE_URL: "https://chatgpt.com/backend-api/codex",
        ANTHROPIC_MODEL: "gpt-5.5",
        ANTHROPIC_DEFAULT_HAIKU_MODEL: "gpt-5.4",
        ANTHROPIC_DEFAULT_SONNET_MODEL: "gpt-5.5",
        ANTHROPIC_DEFAULT_OPUS_MODEL: "gpt-5.5",
      },
    },
    isOfficial: true,
    category: "official",
    apiFormat: "openai_responses",
    providerType: "codex_oauth",
    requiresOAuth: true,
    theme: {
      icon: "codex",
      backgroundColor: "#1F2937",
      textColor: "#FFFFFF",
    },
    icon: "openai",
    iconColor: "#00A67E",
  },
  {
    name: "Kiro OAuth",
    websiteUrl: "https://kiro.dev",
    settingsConfig: {
      env: {
        ANTHROPIC_BASE_URL: "https://q.us-east-1.amazonaws.com",
        ANTHROPIC_MODEL: "claude-sonnet-4-8",
        ANTHROPIC_DEFAULT_HAIKU_MODEL: "claude-haiku-4-5",
        ANTHROPIC_DEFAULT_SONNET_MODEL_NAME: "claude-sonnet-4-8",
        ANTHROPIC_DEFAULT_SONNET_MODEL: "claude-sonnet-4-8",
        ANTHROPIC_DEFAULT_OPUS_MODEL_NAME: "claude-opus-4-8",
        ANTHROPIC_DEFAULT_OPUS_MODEL: "claude-opus-4-8",
      },
    },
    isOfficial: true,
    category: "official",
    providerType: "kiro_oauth",
    requiresOAuth: true,
    theme: {
      backgroundColor: "#111827",
      textColor: "#FFFFFF",
    },
    icon: "kiro",
  },
  {
    name: "Ollama API Key",
    websiteUrl: "https://ollama.com",
    settingsConfig: {
      env: {
        ANTHROPIC_BASE_URL: "https://ollama.com",
        ANTHROPIC_AUTH_TOKEN: "",
        ANTHROPIC_MODEL: "kimi-k2.7-code",
        ANTHROPIC_DEFAULT_HAIKU_MODEL: "kimi-k2.7-code",
        ANTHROPIC_DEFAULT_SONNET_MODEL: "kimi-k2.7-code",
        ANTHROPIC_DEFAULT_OPUS_MODEL: "kimi-k2.7-code",
        ANTHROPIC_DEFAULT_FABLE_MODEL: "kimi-k2.7-code",
      },
      modelMapping: {
        mode: "single",
        upstreamModel: "kimi-k2.7-code",
      },
    },
    isOfficial: false,
    category: "third_party",
    apiFormat: "openai_chat",
    providerType: "ollama_cloud",
    requiresOAuth: false,
    modelsUrl: "https://ollama.com/v1/models",
    testConfig: {
      enabled: true,
      testModel: "gpt-oss:20b",
    },
    theme: {
      backgroundColor: "#111111",
      textColor: "#FFFFFF",
    },
    icon: "ollama",
  },
  {
    name: "Cursor OAuth",
    websiteUrl: "https://cursor.com",
    settingsConfig: {
      env: {
        ANTHROPIC_BASE_URL: "https://api2.cursor.sh",
        ANTHROPIC_MODEL: "composer-2.5",
        ANTHROPIC_DEFAULT_HAIKU_MODEL: "composer-2.5",
        ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME: "Claude Haiku",
        ANTHROPIC_DEFAULT_SONNET_MODEL: "composer-2.5",
        ANTHROPIC_DEFAULT_SONNET_MODEL_NAME: "Claude Sonnet",
        ANTHROPIC_DEFAULT_OPUS_MODEL: "composer-2.5",
        ANTHROPIC_DEFAULT_OPUS_MODEL_NAME: "Claude Opus",
        ANTHROPIC_DEFAULT_FABLE_MODEL: "composer-2.5",
        ANTHROPIC_DEFAULT_FABLE_MODEL_NAME: "Claude Fable",
      },
      modelMapping: {
        mode: "single",
        upstreamModel: "composer-2.5",
      },
    },
    isOfficial: true,
    category: "official",
    providerType: "cursor_oauth",
    requiresOAuth: true,
    theme: {
      backgroundColor: "#111111",
      textColor: "#FFFFFF",
    },
    icon: "cursor",
  },
  {
    name: "Cursor API Key",
    websiteUrl: "https://cursor.com/dashboard/cloud-agents",
    apiKeyUrl: "https://cursor.com/dashboard/cloud-agents",
    settingsConfig: {
      env: {
        ANTHROPIC_BASE_URL: "https://api.cursor.com",
        ANTHROPIC_AUTH_TOKEN: "",
        ANTHROPIC_MODEL: "composer-2.5",
        ANTHROPIC_DEFAULT_HAIKU_MODEL: "composer-2.5",
        ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME: "Claude Haiku",
        ANTHROPIC_DEFAULT_SONNET_MODEL: "composer-2.5",
        ANTHROPIC_DEFAULT_SONNET_MODEL_NAME: "Claude Sonnet",
        ANTHROPIC_DEFAULT_OPUS_MODEL: "composer-2.5",
        ANTHROPIC_DEFAULT_OPUS_MODEL_NAME: "Claude Opus",
        ANTHROPIC_DEFAULT_FABLE_MODEL: "composer-2.5",
        ANTHROPIC_DEFAULT_FABLE_MODEL_NAME: "Claude Fable",
      },
      modelMapping: {
        mode: "single",
        upstreamModel: "composer-2.5",
      },
    },
    isOfficial: true,
    category: "official",
    providerType: "cursor_apikey",
    theme: {
      backgroundColor: "#111111",
      textColor: "#FFFFFF",
    },
    icon: "cursor",
  },
  {
    name: "Antigravity OAuth",
    websiteUrl: "https://antigravity.google",
    settingsConfig: {
      env: {
        ANTHROPIC_BASE_URL: "https://daily-cloudcode-pa.googleapis.com",
        ANTHROPIC_MODEL: "claude-sonnet-4-6",
        ANTHROPIC_DEFAULT_HAIKU_MODEL: "gemini-3.5-flash-low",
        ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME: "Gemini 3.5 Flash (Low)",
        ANTHROPIC_DEFAULT_SONNET_MODEL: "claude-sonnet-4-6",
        ANTHROPIC_DEFAULT_SONNET_MODEL_NAME: "Claude Sonnet 4.6",
        ANTHROPIC_DEFAULT_OPUS_MODEL: "claude-opus-4-6-thinking",
        ANTHROPIC_DEFAULT_OPUS_MODEL_NAME: "Claude Opus 4.6 (Thinking)",
      },
    },
    isOfficial: true,
    category: "official",
    providerType: "antigravity_oauth",
    requiresOAuth: true,
    apiFormat: "gemini_native",
    theme: {
      icon: "gemini",
      backgroundColor: "#1A73E8",
      textColor: "#FFFFFF",
    },
    icon: "gemini",
    iconColor: "#1A73E8",
  },
  {
    name: "Antigravity CLI (agy)",
    websiteUrl: "https://antigravity.google",
    settingsConfig: {
      env: {
        ANTHROPIC_BASE_URL: "https://daily-cloudcode-pa.googleapis.com",
        ANTHROPIC_MODEL: "claude-sonnet-4-6",
        ANTHROPIC_DEFAULT_HAIKU_MODEL: "gemini-3.5-flash-low",
        ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME: "Gemini 3.5 Flash (Low)",
        ANTHROPIC_DEFAULT_SONNET_MODEL: "claude-sonnet-4-6",
        ANTHROPIC_DEFAULT_SONNET_MODEL_NAME: "Claude Sonnet 4.6",
        ANTHROPIC_DEFAULT_OPUS_MODEL: "claude-opus-4-6-thinking",
        ANTHROPIC_DEFAULT_OPUS_MODEL_NAME: "Claude Opus 4.6 (Thinking)",
      },
    },
    isOfficial: true,
    category: "official",
    providerType: "agy_oauth",
    requiresOAuth: true,
    apiFormat: "gemini_native",
    theme: {
      icon: "gemini",
      backgroundColor: "#111827",
      textColor: "#FFFFFF",
    },
    icon: "gemini",
    iconColor: "#111827",
  },
  {
    name: "GitHub Copilot",
    websiteUrl: "https://github.com/features/copilot",
    settingsConfig: {
      env: {
        ANTHROPIC_BASE_URL: "https://api.githubcopilot.com",
        ANTHROPIC_MODEL: "claude-sonnet-5",
        ANTHROPIC_DEFAULT_HAIKU_MODEL: "claude-haiku-4.5",
        ANTHROPIC_DEFAULT_SONNET_MODEL: "claude-sonnet-5",
        ANTHROPIC_DEFAULT_OPUS_MODEL: "claude-sonnet-5",
      },
    },
    category: "third_party",
    apiFormat: "openai_chat",
    providerType: "github_copilot",
    requiresOAuth: true,
    icon: "github",
    iconColor: "#000000",
  },
  {
    name: "DeepSeek Official",
    websiteUrl: "https://chat.deepseek.com",
    settingsConfig: {
      env: {
        ANTHROPIC_BASE_URL: "https://chat.deepseek.com",
        ANTHROPIC_MODEL: "deepseek-v4-flash",
        ANTHROPIC_DEFAULT_HAIKU_MODEL: "deepseek-v4-flash",
        ANTHROPIC_DEFAULT_SONNET_MODEL: "deepseek-v4-flash",
        ANTHROPIC_DEFAULT_OPUS_MODEL: "deepseek-v4-pro",
      },
    },
    category: "cn_official",
    providerType: "deepseek_account",
    requiresOAuth: true,
    theme: {
      icon: "deepseek",
      backgroundColor: "#4D6BFE",
      textColor: "#FFFFFF",
    },
    icon: "deepseek",
    iconColor: "#1E88E5",
  },
  {
    name: "AWS Bedrock (AKSK)",
    websiteUrl: "https://aws.amazon.com/bedrock/",
    settingsConfig: {
      env: {
        ANTHROPIC_BASE_URL:
          "https://bedrock-runtime.${AWS_REGION}.amazonaws.com",
        AWS_ACCESS_KEY_ID: "${AWS_ACCESS_KEY_ID}",
        AWS_SECRET_ACCESS_KEY: "${AWS_SECRET_ACCESS_KEY}",
        AWS_REGION: "${AWS_REGION}",
        ANTHROPIC_MODEL: "global.anthropic.claude-opus-4-8",
        ANTHROPIC_DEFAULT_HAIKU_MODEL:
          "global.anthropic.claude-haiku-4-5-20251001-v1:0",
        ANTHROPIC_DEFAULT_SONNET_MODEL: "global.anthropic.claude-sonnet-5",
        ANTHROPIC_DEFAULT_OPUS_MODEL: "global.anthropic.claude-opus-4-8",
        CLAUDE_CODE_USE_BEDROCK: "1",
      },
    },
    category: "cloud_provider",
    templateValues: {
      AWS_REGION: {
        label: "AWS Region",
        placeholder: "us-west-2",
        editorValue: "us-west-2",
      },
      AWS_ACCESS_KEY_ID: {
        label: "Access Key ID",
        placeholder: "AKIA...",
        editorValue: "",
      },
      AWS_SECRET_ACCESS_KEY: {
        label: "Secret Access Key",
        placeholder: "your-secret-key",
        editorValue: "",
      },
    },
    icon: "aws",
    iconColor: "#FF9900",
  },
  {
    name: "AWS Bedrock (API Key)",
    websiteUrl: "https://aws.amazon.com/bedrock/",
    settingsConfig: {
      apiKey: "",
      env: {
        ANTHROPIC_BASE_URL:
          "https://bedrock-runtime.${AWS_REGION}.amazonaws.com",
        AWS_REGION: "${AWS_REGION}",
        ANTHROPIC_MODEL: "global.anthropic.claude-opus-4-8",
        ANTHROPIC_DEFAULT_HAIKU_MODEL:
          "global.anthropic.claude-haiku-4-5-20251001-v1:0",
        ANTHROPIC_DEFAULT_SONNET_MODEL: "global.anthropic.claude-sonnet-5",
        ANTHROPIC_DEFAULT_OPUS_MODEL: "global.anthropic.claude-opus-4-8",
        CLAUDE_CODE_USE_BEDROCK: "1",
      },
    },
    category: "cloud_provider",
    templateValues: {
      AWS_REGION: {
        label: "AWS Region",
        placeholder: "us-west-2",
        editorValue: "us-west-2",
      },
    },
    icon: "aws",
    iconColor: "#FF9900",
  },
  {
    name: "OpenRouter",
    websiteUrl: "https://openrouter.ai",
    apiKeyUrl: "https://openrouter.ai/keys",
    settingsConfig: {
      env: {
        ANTHROPIC_BASE_URL: "https://openrouter.ai/api",
        ANTHROPIC_AUTH_TOKEN: "",
        ANTHROPIC_MODEL: "anthropic/claude-sonnet-4.6",
        ANTHROPIC_DEFAULT_HAIKU_MODEL: "anthropic/claude-haiku-4.5",
        ANTHROPIC_DEFAULT_SONNET_MODEL: "anthropic/claude-sonnet-4.6",
        ANTHROPIC_DEFAULT_OPUS_MODEL: "anthropic/claude-opus-4.7",
      },
    },
    category: "aggregator",
    icon: "openrouter",
    iconColor: "#6566F1",
  },
  {
    name: "Nvidia",
    websiteUrl: "https://build.nvidia.com",
    apiKeyUrl: "https://build.nvidia.com/settings/api-keys",
    settingsConfig: {
      env: {
        ANTHROPIC_BASE_URL: "https://integrate.api.nvidia.com",
        ANTHROPIC_AUTH_TOKEN: "",
        ANTHROPIC_MODEL: "moonshotai/kimi-k2.5",
        ANTHROPIC_DEFAULT_HAIKU_MODEL: "moonshotai/kimi-k2.5",
        ANTHROPIC_DEFAULT_SONNET_MODEL: "moonshotai/kimi-k2.5",
        ANTHROPIC_DEFAULT_OPUS_MODEL: "moonshotai/kimi-k2.5",
      },
    },
    category: "aggregator",
    apiFormat: "openai_chat",
    icon: "nvidia",
    iconColor: "#000000",
  },
  {
    name: "DeepSeek(API Key)",
    websiteUrl: "https://platform.deepseek.com",
    apiKeyUrl: "https://platform.deepseek.com/api_keys",
    settingsConfig: {
      env: {
        ANTHROPIC_BASE_URL: "https://api.deepseek.com/anthropic",
        ANTHROPIC_AUTH_TOKEN: "",
        ANTHROPIC_MODEL: "deepseek-v4-flash",
        ANTHROPIC_DEFAULT_HAIKU_MODEL: "deepseek-v4-flash",
        ANTHROPIC_DEFAULT_SONNET_MODEL: "deepseek-v4-flash",
        ANTHROPIC_DEFAULT_OPUS_MODEL: "deepseek-v4-pro",
      },
    },
    category: "cn_official",
    endpointCandidates: ["https://api.deepseek.com/anthropic"],
    icon: "deepseek",
    iconColor: "#1E88E5",
  },
];
