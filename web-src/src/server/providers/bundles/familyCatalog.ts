import {
  familyById,
  profileById,
  providerRegistry,
  type CoreProviderApp,
  type ProviderFamilySpec,
} from "@/server/providerRegistry";
import { createDraftForProfile } from "@/server/providers/editor/providerDraft";
import { BUNDLE_TEST_APP_ORDER } from "./bundleDraft";

export type FamilyCategoryId = "custom" | "subscription" | "api_key";

// Keep the historical Group name as an alias so existing callers do not need
// to change just because the picker gained product-facing categories.
export type FamilyGroupId = FamilyCategoryId;

export type FamilyAuthKind = "oauth" | "api_key" | "aws" | "custom";

export const FAMILY_GROUP_ORDER: FamilyCategoryId[] = [
  "custom",
  "subscription",
  "api_key",
];

/**
 * Pinned above everything else and pre-selected. Bringing your own endpoint is
 * the one path that works no matter which vendor someone is on, so it is the
 * safest thing to land on; the subscription accounts sit right underneath and
 * cost one click for anyone who wanted those instead.
 */
export const CUSTOM_FAMILY_IDS = ["family.custom_http"] as const;

const CUSTOM_FAMILY_ID_SET = new Set<string>(CUSTOM_FAMILY_IDS);

/**
 * Product-facing order and membership. This is intentionally explicit: a
 * few API-key-looking providers are subscription accounts in the product,
 * while some managed-account providers belong in the API Key fallback group.
 */
export const SUBSCRIPTION_FAMILY_IDS = [
  "family.claude_oauth",
  "family.openai_oauth",
  "family.google_oauth",
  "family.antigravity_oauth",
  "family.antigravity_cli",
  "family.grok_oauth",
  "family.cursor_oauth",
  "family.cursor_api_key",
  "family.ollama_cloud",
  "family.kiro_oauth",
  "family.github_copilot",
  "family.kimi_code",
  "family.qoder_cosy",
  "family.kimi_coding_api_key",
  "family.zhipu_glm_cn",
  "family.zhipu_glm_global",
  "family.bailian_coding_plan_cn",
  "family.bailian_coding_plan_global",
  "family.minimax_cn",
  "family.minimax_global",
  "family.volcengine_coding_plan",
  "family.xiaomi_mimo_token_plan",
  "family.xiaomi_mimo_token_plan_sgp",
] as const;

const SUBSCRIPTION_FAMILY_ID_SET = new Set<string>(SUBSCRIPTION_FAMILY_IDS);
const SUBSCRIPTION_FAMILY_ORDER = new Map<string, number>(
  SUBSCRIPTION_FAMILY_IDS.map((familyId, index) => [familyId, index]),
);

export function recommendedFamilyId(
  families: readonly ProviderFamilySpec[] = providerRegistry.families,
): string {
  const exists = (familyId: string) =>
    families.some((family) => family.familyId === familyId);
  return (
    CUSTOM_FAMILY_IDS.find(exists) ??
    SUBSCRIPTION_FAMILY_IDS.find(exists) ??
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

export function familyCategoryId(familyId: string): FamilyCategoryId {
  if (CUSTOM_FAMILY_ID_SET.has(familyId)) return "custom";
  return SUBSCRIPTION_FAMILY_ID_SET.has(familyId) ? "subscription" : "api_key";
}

export function familyGroupId(familyId: string): FamilyGroupId {
  return familyCategoryId(familyId);
}

/**
 * Registry labels are contract strings, so they stay English. "Custom" is the one
 * Family named after what it does rather than after a vendor, which makes it the
 * one label worth translating; everything else is a brand name and should not be.
 */
const FAMILY_LABEL_KEYS: Record<string, string> = {
  "family.custom_http": "providerBundle.familyCustom",
};

export function familyLabel(
  family: ProviderFamilySpec,
  t: (key: string, options?: { defaultValue: string }) => string,
): string {
  const labelKey = FAMILY_LABEL_KEYS[family.familyId];
  return labelKey ? t(labelKey, { defaultValue: family.label }) : family.label;
}

/**
 * Search runs over the untranslated label, so a reader who only ever sees 自定义
 * would otherwise find nothing by typing it.
 */
const FAMILY_SEARCH_ALIASES: Record<string, string[]> = {
  "family.custom_http": ["custom http", "自定义", "自訂", "カスタム"],
  "family.kimi_code": ["kimi code oauth", "kimi code"],
  "family.kimi_coding_api_key": ["kimi code api key"],
  "family.qoder_cosy": ["qoder cosy"],
  "family.bailian_coding_plan_cn": ["alibaba", "alibaba china"],
  "family.bailian_coding_plan_global": [
    "alibaba",
    "alibaba global",
    "alibaba singapore",
  ],
};

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
    ...(FAMILY_SEARCH_ALIASES[family.familyId] ?? []),
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
    if (groupId === "subscription") {
      grouped.sort(
        (left, right) =>
          (SUBSCRIPTION_FAMILY_ORDER.get(left.familyId) ??
            Number.MAX_SAFE_INTEGER) -
          (SUBSCRIPTION_FAMILY_ORDER.get(right.familyId) ??
            Number.MAX_SAFE_INTEGER),
      );
    }
    return grouped.length ? [{ groupId, families: grouped }] : [];
  });
}

export function preferredManagedAccount<
  T extends { id: string; is_default: boolean; authIdentityGeneration: number },
>(accounts: readonly T[]): T | undefined {
  return accounts.find((account) => account.is_default) ?? accounts[0];
}
