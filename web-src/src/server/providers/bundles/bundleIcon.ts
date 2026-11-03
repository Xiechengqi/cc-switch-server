/** Display defaults also cover existing Bundles without rewriting their data. */
export function resolveBundleIcon(familyId: string, icon?: string | null): string | undefined {
  if (icon?.trim()) return icon;
  if (familyId === "family.custom_http") return "custom";
  if (familyId === "family.workbuddy_oauth") return "workbuddy";
  if (familyId === "family.codebuddy_oauth") return "codebuddy";
  return undefined;
}
