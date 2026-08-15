import {
  familyById,
  profileById,
  providerRegistry,
  type CoreProviderApp,
  type ProviderFamilySpec,
} from "@/server/providerRegistry";
import { createDraftForProfile } from "@/server/providers/editor/providerDraft";
import { BUNDLE_TEST_APP_ORDER } from "./bundleDraft";

export type FamilyGroupId =
  | "official_oauth"
  | "official_key"
  | "china_plan"
  | "aggregator_cloud"
  | "experimental_bridge"
  | "custom";

export type FamilyAuthKind = "oauth" | "api_key" | "aws" | "custom";

export const FAMILY_GROUP_ORDER: FamilyGroupId[] = [
  "official_oauth",
  "official_key",
  "china_plan",
  "aggregator_cloud",
  "experimental_bridge",
  "custom",
];

const FAMILY_GROUP_MEMBERS: Record<FamilyGroupId, readonly string[]> = {
  official_oauth: [
    "family.claude_oauth",
    "family.openai_oauth",
    "family.google_oauth",
    "family.grok_oauth",
  ],
  official_key: [
    "family.anthropic_api_key",
    "family.openai_api_key",
    "family.gemini_api_key",
  ],
  china_plan: [
    "family.kimi_code",
    "family.kimi_coding_api_key",
    "family.zhipu_glm_cn",
    "family.zhipu_glm_global",
    "family.minimax_cn",
    "family.minimax_global",
    "family.volcengine_coding_plan",
    "family.xiaomi_mimo_token_plan",
    "family.xiaomi_mimo_token_plan_sgp",
  ],
  aggregator_cloud: [
    "family.openrouter",
    "family.nvidia",
    "family.ollama_cloud",
    "family.aws_bedrock_aksk",
    "family.aws_bedrock_api_key",
    "family.github_copilot",
    "family.deepseek_api",
  ],
  experimental_bridge: [
    "family.cursor_oauth",
    "family.cursor_api_key",
    "family.kiro_oauth",
    "family.antigravity_oauth",
    "family.antigravity_cli",
    "family.deepseek_account",
    "family.qoder_cosy",
  ],
  custom: ["family.custom_http"],
};

const FAMILY_GROUP_BY_ID = new Map(
  FAMILY_GROUP_ORDER.flatMap((groupId) =>
    FAMILY_GROUP_MEMBERS[groupId].map((familyId) => [familyId, groupId]),
  ),
);

export function recommendedFamilyId(
  families: readonly ProviderFamilySpec[] = providerRegistry.families,
): string {
  return (
    FAMILY_GROUP_MEMBERS.official_oauth.find((familyId) =>
      families.some((family) => family.familyId === familyId),
    ) ??
    families[0]?.familyId ??
    "family.claude_oauth"
  );
}

export function recommendedFamily(
  families: readonly ProviderFamilySpec[] = providerRegistry.families,
): ProviderFamilySpec {
  return (
    familyById(recommendedFamilyId(families)) ??
    families[0] ??
    providerRegistry.families[0]!
  );
}

export function familyGroupId(familyId: string): FamilyGroupId {
  return FAMILY_GROUP_BY_ID.get(familyId) ?? "aggregator_cloud";
}

export function familyAuthKind(family: ProviderFamilySpec): FamilyAuthKind {
  const profile = profileById(family.credentialProfileId);
  if (profile?.formComposition === "custom") return "custom";
  if (profile?.formComposition === "aws") return "aws";
  if (profile?.credentialPolicy.mode === "managed_account") return "oauth";
  return "api_key";
}

export function familyIsExperimental(family: ProviderFamilySpec): boolean {
  return family.surfaces.some((surface) => {
    const profile = profileById(surface.profileId);
    return profile?.maturity === "experimental";
  });
}

export function familySupportedApps(
  family: ProviderFamilySpec,
): CoreProviderApp[] {
  return BUNDLE_TEST_APP_ORDER.filter((app) =>
    family.surfaces.some((surface) => surface.app === app),
  );
}

export function familySearchText(family: ProviderFamilySpec): string {
  const profile = profileById(family.credentialProfileId);
  const preset = profile ? createDraftForProfile(profile) : undefined;
  return [
    family.familyId,
    family.label,
    family.credentialProfileId,
    familyAuthKind(family),
    familyGroupId(family.familyId),
    preset?.websiteUrl,
    ...familySupportedApps(family),
    ...family.surfaces.map((surface) => surface.profileId),
  ]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();
}

export function filterFamilies(
  families: readonly ProviderFamilySpec[],
  query: string,
  app?: CoreProviderApp | "all",
): ProviderFamilySpec[] {
  const normalizedQuery = query.trim().toLowerCase();
  return families.filter((family) => {
    if (app && app !== "all" && !familySupportedApps(family).includes(app)) {
      return false;
    }
    return (
      !normalizedQuery || familySearchText(family).includes(normalizedQuery)
    );
  });
}

export function groupFamilies(
  families: readonly ProviderFamilySpec[],
): Array<{ groupId: FamilyGroupId; families: ProviderFamilySpec[] }> {
  return FAMILY_GROUP_ORDER.flatMap((groupId) => {
    const grouped = families.filter(
      (family) => familyGroupId(family.familyId) === groupId,
    );
    return grouped.length ? [{ groupId, families: grouped }] : [];
  });
}

export function preferredManagedAccount<
  T extends { id: string; is_default: boolean; authIdentityGeneration: number },
>(accounts: readonly T[]): T | undefined {
  return accounts.find((account) => account.is_default) ?? accounts[0];
}
