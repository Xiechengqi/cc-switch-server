export interface IconMetadata {
  name: string;
  displayName: string;
  defaultColor?: string;
}

export const iconMetadata: Record<string, IconMetadata> = {
  claude: { name: "claude", displayName: "Claude", defaultColor: "#D4915D" },
  anthropic: {
    name: "anthropic",
    displayName: "Anthropic",
    defaultColor: "#D4915D",
  },
  openai: { name: "openai", displayName: "OpenAI", defaultColor: "#111827" },
  gemini: { name: "gemini", displayName: "Gemini", defaultColor: "#8E75B2" },
  deepseek: {
    name: "deepseek",
    displayName: "DeepSeek",
    defaultColor: "#1E88E5",
  },
  ollama: { name: "ollama", displayName: "Ollama", defaultColor: "#111827" },
  openrouter: {
    name: "openrouter",
    displayName: "OpenRouter",
    defaultColor: "#111827",
  },
  zhipu: { name: "zhipu", displayName: "Zhipu", defaultColor: "#0F62FE" },
  qwen: { name: "qwen", displayName: "Qwen", defaultColor: "#FF6A00" },
  alibaba: { name: "alibaba", displayName: "Alibaba", defaultColor: "#FF6A00" },
  bailian: { name: "bailian", displayName: "Bailian", defaultColor: "#624AFF" },
  kimi: { name: "kimi", displayName: "Kimi", defaultColor: "#6366F1" },
  nvidia: { name: "nvidia", displayName: "NVIDIA", defaultColor: "#76B900" },
  aws: { name: "aws", displayName: "AWS", defaultColor: "#FF9900" },
  azure: { name: "azure", displayName: "Azure", defaultColor: "#0078D4" },
  google: { name: "google", displayName: "Google", defaultColor: "#4285F4" },
  cloudflare: {
    name: "cloudflare",
    displayName: "Cloudflare",
    defaultColor: "#F38020",
  },
  mistral: { name: "mistral", displayName: "Mistral", defaultColor: "#FF7000" },
  cohere: { name: "cohere", displayName: "Cohere", defaultColor: "#39594D" },
  perplexity: {
    name: "perplexity",
    displayName: "Perplexity",
    defaultColor: "#20808D",
  },
  huggingface: {
    name: "huggingface",
    displayName: "Hugging Face",
    defaultColor: "#FFD21E",
  },
  novita: { name: "novita", displayName: "Novita", defaultColor: "#111827" },
  baidu: { name: "baidu", displayName: "Baidu", defaultColor: "#2932E1" },
  tencent: { name: "tencent", displayName: "Tencent", defaultColor: "#00A4FF" },
  hunyuan: { name: "hunyuan", displayName: "Hunyuan", defaultColor: "#00A4FF" },
  minimax: { name: "minimax", displayName: "MiniMax", defaultColor: "#FF6B6B" },
  xai: { name: "xai", displayName: "xAI", defaultColor: "#111827" },
  grok: { name: "grok", displayName: "Grok", defaultColor: "#111827" },
  cursor: { name: "cursor", displayName: "Cursor" },
  kiro: { name: "kiro", displayName: "Kiro" },
  copilot: { name: "copilot", displayName: "Copilot", defaultColor: "#111827" },
  githubcopilot: {
    name: "githubcopilot",
    displayName: "GitHub Copilot",
    defaultColor: "#111827",
  },
  github: { name: "github", displayName: "GitHub", defaultColor: "#111827" },
  googlecloud: {
    name: "googlecloud",
    displayName: "Google Cloud",
    defaultColor: "#4285F4",
  },
  doubao: { name: "doubao", displayName: "Doubao", defaultColor: "#1E37FC" },
  siliconflow: {
    name: "siliconflow",
    displayName: "SiliconFlow",
    defaultColor: "#6E29F6",
  },
  stepfun: { name: "stepfun", displayName: "StepFun", defaultColor: "#005AFF" },
  meta: { name: "meta", displayName: "Meta", defaultColor: "#0082FB" },
  huawei: { name: "huawei", displayName: "Huawei", defaultColor: "#C7000B" },
  newapi: { name: "newapi", displayName: "NewAPI", defaultColor: "#C738FB" },
  subrouter: {
    name: "subrouter",
    displayName: "SubRouter",
    defaultColor: "#111827",
  },
  bytedance: {
    name: "bytedance",
    displayName: "ByteDance",
    defaultColor: "#3C8CFF",
  },
  chatglm: { name: "chatglm", displayName: "ChatGLM", defaultColor: "#0F62FE" },
  gemma: { name: "gemma", displayName: "Gemma", defaultColor: "#4285F4" },
  "modelscope-color": {
    name: "modelscope-color",
    displayName: "ModelScope",
    defaultColor: "#624AFF",
  },
  wenxin: { name: "wenxin", displayName: "Wenxin", defaultColor: "#2932E1" },
  yi: { name: "yi", displayName: "01.AI Yi", defaultColor: "#111827" },
  zeroone: { name: "zeroone", displayName: "01.AI", defaultColor: "#111827" },
  palm: { name: "palm", displayName: "PaLM", defaultColor: "#4285F4" },
  stability: {
    name: "stability",
    displayName: "Stability AI",
    defaultColor: "#111827",
  },
  midjourney: {
    name: "midjourney",
    displayName: "Midjourney",
    defaultColor: "#111827",
  },
  vercel: { name: "vercel", displayName: "Vercel", defaultColor: "#111827" },
  ucloud: { name: "ucloud", displayName: "UCloud", defaultColor: "#2B70FF" },
  notion: { name: "notion", displayName: "Notion", defaultColor: "#111827" },
  opencode: {
    name: "opencode",
    displayName: "OpenCode",
    defaultColor: "#211E1E",
  },
  openclaw: {
    name: "openclaw",
    displayName: "OpenClaw",
    defaultColor: "#ff4d4d",
  },
  hermes: { name: "hermes", displayName: "Hermes" },
  "opencode-logo-light": {
    name: "opencode-logo-light",
    displayName: "OpenCode",
    defaultColor: "#111827",
  },
  "aihubmix-color": {
    name: "aihubmix-color",
    displayName: "AIHubMix",
    defaultColor: "#2563EB",
  },
  aicoding: {
    name: "aicoding",
    displayName: "AICoding",
    defaultColor: "#2563EB",
  },
  algocode: {
    name: "algocode",
    displayName: "AlgoCode",
    defaultColor: "#2563EB",
  },
  catcoder: {
    name: "catcoder",
    displayName: "CatCoder",
    defaultColor: "#FF7A1A",
  },
  claw: { name: "claw", displayName: "Claw", defaultColor: "#111827" },
  cubence: { name: "cubence", displayName: "Cubence", defaultColor: "#2563EB" },
  "longcat-color": {
    name: "longcat-color",
    displayName: "LongCat",
    defaultColor: "#111827",
  },
  aicodemirror: {
    name: "aicodemirror",
    displayName: "AICodeMirror",
    defaultColor: "#2563EB",
  },
  crazyrouter: {
    name: "crazyrouter",
    displayName: "CrazyRouter",
    defaultColor: "#7C3AED",
  },
  lioncc: { name: "lioncc", displayName: "LionCC", defaultColor: "#F59E0B" },
  micu: { name: "micu", displayName: "MiCu", defaultColor: "#2563EB" },
  packycode: {
    name: "packycode",
    displayName: "PackyCode",
    defaultColor: "#111827",
  },
  rc: { name: "rc", displayName: "RC", defaultColor: "#111827" },
  sssaicode: {
    name: "sssaicode",
    displayName: "SSSAI Code",
    defaultColor: "#2563EB",
  },
  xiaomimimo: {
    name: "xiaomimimo",
    displayName: "Xiaomi MiMo",
    defaultColor: "#FF6900",
  },
};

export function getIconMetadata(name: string): IconMetadata | undefined {
  return iconMetadata[name.toLowerCase()];
}

export function searchIcons(query: string): string[] {
  const lowerQuery = query.toLowerCase();
  return Object.values(iconMetadata)
    .filter(
      (meta) =>
        meta.name.includes(lowerQuery) ||
        meta.displayName.toLowerCase().includes(lowerQuery),
    )
    .map((meta) => meta.name);
}
