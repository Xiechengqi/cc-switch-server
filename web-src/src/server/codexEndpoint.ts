import { parse as parseToml } from "smol-toml";

type TomlObject = Record<string, unknown>;

function objectValue(value: unknown): TomlObject | undefined {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as TomlObject)
    : undefined;
}

function nonEmptyString(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const trimmed = value.trim();
  return trimmed || undefined;
}

/** Resolve the endpoint used by the active Codex model provider. */
export function extractCodexBaseUrl(
  configText: string | undefined | null,
): string | undefined {
  if (!configText?.trim()) return undefined;
  try {
    const parsed = parseToml(configText) as TomlObject;
    const providers = objectValue(parsed.model_providers);
    const selectedProvider = nonEmptyString(parsed.model_provider);
    if (providers && selectedProvider) {
      const selected = objectValue(providers[selectedProvider]);
      const selectedUrl = nonEmptyString(selected?.base_url);
      if (selectedUrl) return selectedUrl;
    }

    const topLevelUrl = nonEmptyString(parsed.base_url);
    if (topLevelUrl) return topLevelUrl;

    const candidates = providers
      ? Object.values(providers)
          .map((provider) =>
            nonEmptyString(objectValue(provider)?.base_url),
          )
          .filter((value): value is string => Boolean(value))
      : [];
    return candidates.length === 1 ? candidates[0] : undefined;
  } catch {
    return undefined;
  }
}

export function getCodexBaseUrl(
  provider:
    | { settingsConfig?: Record<string, unknown> }
    | undefined
    | null,
): string | undefined {
  const config = provider?.settingsConfig?.config;
  return extractCodexBaseUrl(typeof config === "string" ? config : undefined);
}
