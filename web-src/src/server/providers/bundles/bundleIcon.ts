import presentation from "../../../../../assets/generated/provider-presentation.json";

/** Same frozen catalog as Router; an explicit icon never changes the provider category. */
export function resolveBundleIcon(familyId: string, icon?: string | null): string | undefined {
  if (icon?.trim()) return icon.trim();
  return (presentation.families as Record<string, { icon: string }>)[familyId]?.icon;
}
