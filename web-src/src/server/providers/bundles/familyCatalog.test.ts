import { describe, expect, it } from "vitest";

import { familyById, providerRegistry } from "@/server/providerRegistry";
import {
  FAMILY_GROUP_ORDER,
  familyAuthKind,
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

  it("places every visible Family into exactly one group", () => {
    const grouped = new Set(
      groupFamilies(providerRegistry.families).flatMap((group) =>
        group.families.map((family) => family.familyId),
      ),
    );
    expect([...grouped].sort()).toEqual(
      [...providerRegistry.families.map((family) => family.familyId)].sort(),
    );
    expect(FAMILY_GROUP_ORDER).toContain(
      familyGroupId("family.custom_http"),
    );
  });

  it("classifies auth, experimental maturity, and supported Apps", () => {
    expect(familyAuthKind(familyById("family.claude_oauth")!)).toBe("oauth");
    expect(familyAuthKind(familyById("family.openrouter")!)).toBe("api_key");
    expect(familyAuthKind(familyById("family.aws_bedrock_aksk")!)).toBe("aws");
    expect(familyAuthKind(familyById("family.custom_http")!)).toBe("custom");
    expect(familyIsExperimental(familyById("family.cursor_oauth")!)).toBe(true);
    expect(familySupportedApps(familyById("family.openai_oauth")!)).toEqual([
      "claude",
      "codex",
    ]);
  });

  it("filters Families by search text and App", () => {
    const grok = filterFamilies(providerRegistry.families, "grok");
    expect(grok.map((family) => family.familyId)).toEqual(["family.grok_oauth"]);
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
