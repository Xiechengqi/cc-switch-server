// Provider 类型常量
export const PROVIDER_TYPES = {
  GITHUB_COPILOT: "github_copilot",
  CODEX_OAUTH: "codex_oauth",
  CLAUDE_OAUTH: "claude_oauth",
  GOOGLE_GEMINI_OAUTH: "google_gemini_oauth",
  ANTIGRAVITY_OAUTH: "antigravity_oauth",
  AGY_OAUTH: "agy_oauth",
  CURSOR_OAUTH: "cursor_oauth",
  CURSOR_APIKEY: "cursor_apikey",
  KIRO_OAUTH: "kiro_oauth",
  DEEPSEEK_ACCOUNT: "deepseek_account",
  OLLAMA_CLOUD: "ollama_cloud",
} as const;

// 用量脚本模板类型常量
export const TEMPLATE_TYPES = {
  CUSTOM: "custom",
  GENERAL: "general",
  NEW_API: "newapi",
  GITHUB_COPILOT: "github_copilot",
  TOKEN_PLAN: "token_plan",
  BALANCE: "balance",
  OFFICIAL_SUBSCRIPTION: "official_subscription",
} as const;

export type TemplateType = (typeof TEMPLATE_TYPES)[keyof typeof TEMPLATE_TYPES];

// Temporary Codex Banked Reset campaign entry. Keep all UI gated by this flag so
// the limited-time feature can be hidden before the isolated implementation is removed.
export const ENABLE_CODEX_BANKED_RESET = true;

// OpenAI currently rejects Codex CLI OAuth redirect_uri values outside the
// registered localhost callback. Keep this false unless that upstream changes.
export const ENABLE_CODEX_CLI_REMOTE_CALLBACK = false;
