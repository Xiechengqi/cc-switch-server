import { inferIconForText } from "@/config/iconInference";
import { AppKind, ProviderPresetSummary, StoredProvider } from "@/lib/api";

export function appIcon(app: AppKind): { icon: string; color?: string } {
  switch (app) {
    case "claude":
      return { icon: "claude", color: "#D4915D" };
    case "codex":
      return { icon: "openai", color: "#111827" };
    case "gemini":
      return { icon: "gemini", color: "#8E75B2" };
  }
}

export function storedProviderIcon(provider: StoredProvider): { icon?: string; color?: string } {
  const explicitIcon = stringValue(provider.provider.icon);
  const explicitColor = stringValue(provider.provider.iconColor);
  if (explicitIcon) {
    return { icon: explicitIcon, color: explicitColor };
  }
  const inferred = inferIconForText(
    provider.provider.name,
    provider.providerTypeId,
    provider.providerType,
    stringValue(provider.provider.category),
    stringValue(provider.provider.meta?.providerType),
    stringValue(provider.provider.meta?.authBinding?.authProvider),
  );
  if (inferred.icon) {
    return { icon: inferred.icon, color: inferred.iconColor };
  }
  return appIcon(provider.app);
}

export function presetIcon(preset: ProviderPresetSummary): { icon?: string; color?: string } {
  const inferred = inferIconForText(preset.name, preset.providerType, preset.apiFormat, preset.baseUrl);
  return { icon: inferred.icon, color: inferred.iconColor };
}

function stringValue(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}
