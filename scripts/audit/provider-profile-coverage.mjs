export const requiredProviderTypes = Object.freeze([
  ["claude", "Anthropic official / API key", ["claude"]],
  ["claude_auth", "Claude bearer-only relay", ["claude"]],
  ["claude_oauth", "Claude Official OAuth", ["claude"]],
  ["codex", "OpenAI/Codex compatible", ["codex"]],
  ["codex_oauth", "OpenAI ChatGPT OAuth", ["claude", "codex"]],
  ["gemini", "Google Gemini API key", ["gemini"]],
  ["gemini_cli", "Google Gemini OAuth / CLI", ["gemini", "claude"]],
  ["openrouter", "OpenRouter", ["claude", "codex", "gemini"]],
  ["github_copilot", "GitHub Copilot", ["claude"]],
  ["deepseek_account", "DeepSeek account", ["claude"]],
  ["kiro_oauth", "Kiro OAuth", ["claude", "codex"]],
  ["cursor_oauth", "Cursor OAuth", ["claude", "codex"]],
  ["cursor_apikey", "Cursor API key", ["claude", "codex"]],
  ["antigravity_oauth", "Antigravity OAuth", ["claude", "gemini"]],
  ["agy_oauth", "Antigravity CLI / agy", ["claude", "gemini"]],
  ["ollama_cloud", "Ollama API key", ["claude", "codex"]],
]);

export const serverCompatibilityProviderTypes = Object.freeze([
  ["aws_bedrock", "AWS Bedrock compatibility schema", ["claude"]],
  ["nvidia", "Nvidia OpenAI-compatible API", ["claude", "codex"]],
  ["deepseek_api", "DeepSeek API key", ["claude", "codex"]],
  ["grok_oauth", "Grok/xAI OAuth reverse proxy", ["claude", "codex", "gemini"]],
  ["kimi_code", "Kimi Code OAuth", ["claude", "codex", "gemini"]],
]);

export function requiredProviderProfilePairs() {
  return [
    ...requiredProviderTypes,
    ...serverCompatibilityProviderTypes,
  ].flatMap(([providerType, , apps]) =>
    apps.map((app) => ({ providerType, app })),
  );
}

export function assertRequiredProviderProfileCoverage(registry) {
  const seen = new Set();
  for (const { providerType, app } of requiredProviderProfilePairs()) {
    const key = `${app}:${providerType}`;
    if (seen.has(key)) {
      throw new Error(
        `Duplicate required Provider Profile coverage pair ${key}`,
      );
    }
    seen.add(key);

    const profiles = registry.profiles.filter(
      (profile) =>
        profile.app === app &&
        profile.compatibilityProviderType === providerType &&
        profile.visibility === "visible" &&
        profile.creationPolicy === "create_allowed",
    );
    if (profiles.length === 0) {
      throw new Error(
        `Missing visible create_allowed Provider Profile for ${key}`,
      );
    }
    for (const profile of profiles) {
      if (
        profile.credentialPolicy?.mode === "managed_account" &&
        profile.credentialPolicy.accountProviderType !== providerType
      ) {
        throw new Error(
          `Managed Provider Profile ${profile.profileId} binds ${profile.credentialPolicy.accountProviderType}, expected ${providerType}`,
        );
      }
    }
  }
}
