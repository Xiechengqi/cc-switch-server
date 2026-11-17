import presentation from "../../../../../assets/generated/provider-presentation.json";

/** Same frozen catalog as Router; an explicit icon never changes the provider category. */
export function resolveBundleIcon(familyId: string, icon?: string | null): string | undefined {
  // Older Zen presets persisted the generic placeholder as an explicit icon.
  if (icon?.trim() && !(familyId === "family.opencode_zen" && icon.trim() === "custom")) return icon.trim();
  return (presentation.families as Record<string, { icon: string }>)[familyId]?.icon;
}
