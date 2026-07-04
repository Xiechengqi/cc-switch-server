export interface IconMetadata {
  name: string;
  displayName: string;
  defaultColor?: string;
  source: "desktop";
}

export const iconMetadata: Record<string, IconMetadata> = {
  claude: { name: "claude", displayName: "Claude", defaultColor: "#D4915D", source: "desktop" },
  anthropic: { name: "anthropic", displayName: "Anthropic", defaultColor: "#D4915D", source: "desktop" },
  openai: { name: "openai", displayName: "OpenAI", defaultColor: "#111827", source: "desktop" },
  gemini: { name: "gemini", displayName: "Gemini", defaultColor: "#8E75B2", source: "desktop" },
  deepseek: { name: "deepseek", displayName: "DeepSeek", defaultColor: "#1E88E5", source: "desktop" },
  ollama: { name: "ollama", displayName: "Ollama", defaultColor: "#111827", source: "desktop" },
  openrouter: { name: "openrouter", displayName: "OpenRouter", defaultColor: "#111827", source: "desktop" },
  zhipu: { name: "zhipu", displayName: "Zhipu", defaultColor: "#0F62FE", source: "desktop" },
  qwen: { name: "qwen", displayName: "Qwen", defaultColor: "#FF6A00", source: "desktop" },
  alibaba: { name: "alibaba", displayName: "Alibaba", defaultColor: "#FF6A00", source: "desktop" },
  bailian: { name: "bailian", displayName: "Bailian", defaultColor: "#624AFF", source: "desktop" },
  kimi: { name: "kimi", displayName: "Kimi", defaultColor: "#6366F1", source: "desktop" },
  nvidia: { name: "nvidia", displayName: "NVIDIA", defaultColor: "#76B900", source: "desktop" },
  aws: { name: "aws", displayName: "AWS", defaultColor: "#FF9900", source: "desktop" },
  azure: { name: "azure", displayName: "Azure", defaultColor: "#0078D4", source: "desktop" },
  google: { name: "google", displayName: "Google", defaultColor: "#4285F4", source: "desktop" },
  cloudflare: { name: "cloudflare", displayName: "Cloudflare", defaultColor: "#F38020", source: "desktop" },
  mistral: { name: "mistral", displayName: "Mistral", defaultColor: "#FF7000", source: "desktop" },
  cohere: { name: "cohere", displayName: "Cohere", defaultColor: "#39594D", source: "desktop" },
  perplexity: { name: "perplexity", displayName: "Perplexity", defaultColor: "#20808D", source: "desktop" },
  huggingface: { name: "huggingface", displayName: "Hugging Face", defaultColor: "#FFD21E", source: "desktop" },
  novita: { name: "novita", displayName: "Novita", defaultColor: "#111827", source: "desktop" },
  baidu: { name: "baidu", displayName: "Baidu", defaultColor: "#2932E1", source: "desktop" },
  tencent: { name: "tencent", displayName: "Tencent", defaultColor: "#00A4FF", source: "desktop" },
  hunyuan: { name: "hunyuan", displayName: "Hunyuan", defaultColor: "#00A4FF", source: "desktop" },
  minimax: { name: "minimax", displayName: "MiniMax", defaultColor: "#FF6B6B", source: "desktop" },
  xai: { name: "xai", displayName: "xAI", defaultColor: "#111827", source: "desktop" },
  grok: { name: "grok", displayName: "Grok", defaultColor: "#111827", source: "desktop" },
  cursor: { name: "cursor", displayName: "Cursor", source: "desktop" },
  kiro: { name: "kiro", displayName: "Kiro", source: "desktop" },
  copilot: { name: "copilot", displayName: "Copilot", defaultColor: "#111827", source: "desktop" },
  githubcopilot: { name: "githubcopilot", displayName: "GitHub Copilot", defaultColor: "#111827", source: "desktop" },
};
