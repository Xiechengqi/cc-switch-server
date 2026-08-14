export const requiredProviderTypes = Object.freeze([
  ["claude", "Anthropic official / API key", ["claude"]],
  ["claude_auth", "Claude bearer-only relay", ["claude"]],
  ["claude_oauth", "Claude Official OAuth", ["claude"]],
  ["codex", "OpenAI/Codex compatible", ["codex"]],
  ["codex_oauth", "OpenAI ChatGPT OAuth", ["claude", "codex"]],
  ["gemini", "Google Gemini API key", ["gemini"]],
  ["gemini_cli", "Google Gemini OAuth / CLI", ["gemini", "claude"]],
  ["openrouter", "OpenRouter", ["claude", "codex", "gemini"]],
  ["github_copilot", "GitHub Copilot", ["claude", "codex"]],
  ["deepseek_account", "DeepSeek account", ["claude"]],
  ["kiro_oauth", "Kiro OAuth", ["claude", "codex"]],
  ["cursor_oauth", "Cursor OAuth", ["claude", "codex", "gemini"]],
  ["cursor_apikey", "Cursor API key", ["claude", "codex", "gemini"]],
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
  ["qoder_cosy", "Qoder COSY", ["claude", "codex", "gemini"]],
]);

export function requiredProviderProfilePairs() {
  return [
    ...requiredProviderTypes,
    ...serverCompatibilityProviderTypes,
  ].flatMap(([providerType, , apps]) =>
    apps.map((app) => ({ providerType, app })),
  );
}

function customRecipeCompatibilityProviderType(binding) {
  switch (binding?.upstreamProtocol) {
    case "anthropic_messages":
      return binding.authScheme === "bearer" ? "claude_auth" : "claude";
    case "open_ai_chat":
    case "open_ai_responses":
      return "codex";
    case "gemini_native":
      return "gemini";
    case "bedrock":
      return "aws_bedrock";
    default:
      return null;
  }
}

export function assertRequiredProviderCoverage(registry) {
  const recipeCoverage = new Map();
  for (const recipe of registry.customRecipes ?? []) {
    const profile = registry.profiles.find(
      (candidate) => candidate.profileId === recipe.profileId,
    );
    if (
      !profile ||
      profile.formComposition !== "custom" ||
      profile.visibility !== "visible" ||
      profile.creationPolicy !== "create_allowed" ||
      profile.driverBinding?.kind !== "custom"
    ) {
      throw new Error(
        `Custom HTTP recipe ${recipe.recipeId} does not reference a visible create_allowed Custom Profile`,
      );
    }
    const policy = registry.customPolicies?.find(
      (candidate) =>
        candidate.customPolicyId === profile.driverBinding.customPolicyId,
    );
    if (
      !policy?.protocols?.includes(recipe.binding?.upstreamProtocol) ||
      !policy.authSchemes?.includes(recipe.binding?.authScheme)
    ) {
      throw new Error(
        `Custom HTTP recipe ${recipe.recipeId} uses a binding rejected by ${profile.profileId}`,
      );
    }
    const resolvedProviderType = customRecipeCompatibilityProviderType(
      recipe.binding,
    );
    if (resolvedProviderType !== recipe.compatibilityProviderType) {
      throw new Error(
        `Custom HTTP recipe ${recipe.recipeId} declares ${recipe.compatibilityProviderType}, resolved ${resolvedProviderType ?? "unsupported"}`,
      );
    }
    const allowedModelPolicies = profile.allowedModelPolicies?.length
      ? profile.allowedModelPolicies
      : [profile.modelPolicy];
    if (!allowedModelPolicies.includes(recipe.modelPolicy)) {
      throw new Error(
        `Custom HTTP recipe ${recipe.recipeId} uses a model policy rejected by ${profile.profileId}`,
      );
    }
    recipeCoverage.set(recipe.recipeId, {
      app: profile.app,
      providerType: resolvedProviderType,
    });
  }

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
    const recipes = (registry.customRecipes ?? []).filter((recipe) => {
      const coverage = recipeCoverage.get(recipe.recipeId);
      return coverage?.app === app && coverage.providerType === providerType;
    });
    if (profiles.length === 0 && recipes.length === 0) {
      throw new Error(
        `Missing visible create_allowed Provider Profile or Custom HTTP recipe for ${key}`,
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
