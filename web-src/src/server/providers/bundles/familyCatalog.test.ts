import { describe, expect, it } from "vitest";

import { familyById, providerRegistry } from "@/server/providerRegistry";
import {
  FAMILY_GROUP_ORDER,
  SUBSCRIPTION_FAMILY_IDS,
  familyAuthKind,
  familyCategoryId,
  familyGroupId,
  familyIsExperimental,
  familySearchText,
  familySupportedApps,
  filterFamilies,
  groupFamilies,
  preferredManagedAccount,
  recommendedFamily,
  recommendedFamilyId,
} from "./familyCatalog";

describe("familyCatalog", () => {
  it("recommends Claude OAuth as the default Family", () => {
    expect(recommendedFamilyId()).toBe("family.claude_oauth");
    expect(recommendedFamily().familyId).toBe("family.claude_oauth");
  });

  it("places every visible Family into exactly one of the two categories", () => {
    const groups = groupFamilies(providerRegistry.families);
    const groupedIds = groups.flatMap((group) =>
      group.families.map((family) => family.familyId),
    );
    expect(FAMILY_GROUP_ORDER).toEqual(["subscription", "api_key"]);
    expect(new Set(groupedIds).size).toBe(groupedIds.length);
    expect([...groupedIds].sort()).toEqual(
      [...providerRegistry.families.map((family) => family.familyId)].sort(),
    );
    expect(
      groups
        .find((group) => group.groupId === "subscription")
        ?.families.map((family) => family.familyId),
    ).toEqual([...SUBSCRIPTION_FAMILY_IDS]);
    expect(
      groups
        .find((group) => group.groupId === "api_key")
        ?.families.map((family) => family.familyId),
    ).toEqual([
      "family.anthropic_api_key",
      "family.deepseek_account",
      "family.aws_bedrock_aksk",
      "family.aws_bedrock_api_key",
      "family.openrouter",
      "family.nvidia",
      "family.deepseek_api",
      "family.openai_api_key",
      "family.gemini_api_key",
      "family.custom_http",
    ]);
    expect(familyCategoryId("family.custom_http")).toBe("api_key");
    expect(familyGroupId("family.custom_http")).toBe("api_key");
    expect(familyCategoryId("family.future_provider")).toBe("api_key");
    const futureFamily = {
      ...providerRegistry.families[0]!,
      familyId: "family.future_provider",
    };
    expect(
      groupFamilies([...providerRegistry.families, futureFamily])
        .find((group) => group.groupId === "api_key")
        ?.families.some(
          (family) => family.familyId === "family.future_provider",
        ),
    ).toBe(true);
  });

  it("classifies auth, experimental maturity, and supported Apps", () => {
    expect(familyAuthKind(familyById("family.claude_oauth")!)).toBe("oauth");
    expect(familyAuthKind(familyById("family.openrouter")!)).toBe("api_key");
    expect(familyAuthKind(familyById("family.aws_bedrock_aksk")!)).toBe("aws");
    expect(familyAuthKind(familyById("family.custom_http")!)).toBe("custom");
    expect(familyCategoryId("family.cursor_api_key")).toBe("subscription");
    expect(familyCategoryId("family.ollama_cloud")).toBe("subscription");
    expect(familyCategoryId("family.kimi_coding_api_key")).toBe("subscription");
    expect(familyCategoryId("family.bailian_coding_plan_cn")).toBe(
      "subscription",
    );
    expect(familyCategoryId("family.deepseek_account")).toBe("api_key");
    expect(familyIsExperimental(familyById("family.cursor_oauth")!)).toBe(true);
    expect(familySupportedApps(familyById("family.openai_oauth")!)).toEqual([
      "claude",
      "codex",
    ]);
  });

  it("filters Families by search text and App", () => {
    const grok = filterFamilies(providerRegistry.families, "grok");
    expect(grok.map((family) => family.familyId)).toEqual([
      "family.grok_oauth",
    ]);
    const codexOnly = filterFamilies(providerRegistry.families, "", "codex");
    expect(
      codexOnly.every((family) =>
        family.surfaces.some((surface) => surface.app === "codex"),
      ),
    ).toBe(true);
    expect(familySearchText(familyById("family.openrouter")!)).toContain(
      "openrouter",
    );
  });

  it("prefers the default managed account", () => {
    expect(
      preferredManagedAccount([
        { id: "second", is_default: false, authIdentityGeneration: 1 },
        { id: "first", is_default: true, authIdentityGeneration: 2 },
      ])?.id,
    ).toBe("first");
    expect(
      preferredManagedAccount([
        { id: "only", is_default: false, authIdentityGeneration: 1 },
      ])?.id,
    ).toBe("only");
  });
});
