const iconMappings: Record<string, { icon: string; iconColor?: string }> = {
  claude: { icon: "claude", iconColor: "#D4915D" },
  anthropic: { icon: "anthropic", iconColor: "#D4915D" },
  openai: { icon: "openai", iconColor: "#111827" },
  chatgpt: { icon: "openai", iconColor: "#111827" },
  codex: { icon: "openai", iconColor: "#111827" },
  gemini: { icon: "gemini", iconColor: "#8E75B2" },
  google: { icon: "google", iconColor: "#4285F4" },
  deepseek: { icon: "deepseek", iconColor: "#1E88E5" },
  ollama: { icon: "ollama", iconColor: "#111827" },
  openrouter: { icon: "openrouter", iconColor: "#111827" },
  zhipu: { icon: "zhipu", iconColor: "#0F62FE" },
  glm: { icon: "zhipu", iconColor: "#0F62FE" },
  qwen: { icon: "qwen", iconColor: "#FF6A00" },
  bailian: { icon: "bailian", iconColor: "#624AFF" },
  alibaba: { icon: "alibaba", iconColor: "#FF6A00" },
  aliyun: { icon: "alibaba", iconColor: "#FF6A00" },
  kimi: { icon: "kimi", iconColor: "#6366F1" },
  moonshot: { icon: "kimi", iconColor: "#6366F1" },
  nvidia: { icon: "nvidia", iconColor: "#76B900" },
  aws: { icon: "aws", iconColor: "#FF9900" },
  azure: { icon: "azure", iconColor: "#0078D4" },
  cloudflare: { icon: "cloudflare", iconColor: "#F38020" },
  mistral: { icon: "mistral", iconColor: "#FF7000" },
  cohere: { icon: "cohere", iconColor: "#39594D" },
  perplexity: { icon: "perplexity", iconColor: "#20808D" },
  huggingface: { icon: "huggingface", iconColor: "#FFD21E" },
  novita: { icon: "novita", iconColor: "#111827" },
  baidu: { icon: "baidu", iconColor: "#2932E1" },
  tencent: { icon: "tencent", iconColor: "#00A4FF" },
  hunyuan: { icon: "hunyuan", iconColor: "#00A4FF" },
  minimax: { icon: "minimax", iconColor: "#FF6B6B" },
  xai: { icon: "xai", iconColor: "#111827" },
  grok: { icon: "grok", iconColor: "#111827" },
  cursor: { icon: "cursor" },
  kiro: { icon: "kiro" },
  copilot: { icon: "copilot", iconColor: "#111827" },
  githubcopilot: { icon: "githubcopilot", iconColor: "#111827" },
};

export function inferIconForText(...parts: Array<string | null | undefined>): {
  icon?: string;
  iconColor?: string;
} {
  const haystack = parts.filter(Boolean).join(" ").toLowerCase();
  for (const [key, config] of Object.entries(iconMappings)) {
    if (haystack.includes(key)) {
      return config;
    }
  }
  return {};
}

export function addIconsToPresets<T extends { name: string; icon?: string; iconColor?: string }>(
  presets: T[],
): T[] {
  return presets.map((preset) => {
    if (preset.icon) return preset;
    return {
      ...preset,
      ...inferIconForText(preset.name),
    };
  });
}
