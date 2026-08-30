import { describe, expect, it } from "vitest";

import { familyById } from "@/server/providerRegistry";
import { createProviderBundleDraft } from "./bundleDraft";
import { bundleReadiness } from "./bundleReadiness";

function fillModels<T extends ReturnType<typeof createProviderBundleDraft>>(
  draft: T,
): T {
  return {
    ...draft,
    upstreamModel: draft.upstreamModel || "model",
    surfaces: draft.surfaces.map((surface) => ({
      ...surface,
      upstreamModel: surface.upstreamModel || "model",
    })),
  };
}

describe("bundleReadiness", () => {
  it("reports a missing OAuth account on every enabled Surface", () => {
    const draft = createProviderBundleDraft(familyById("family.openai_oauth")!);
    const readiness = bundleReadiness(draft);
    expect(readiness.connection).toBe("account");
    expect(readiness.ready).toBe(false);
    expect(
      readiness.surfaces
        .filter((surface) => surface.enabled)
        .every((surface) => surface.gap === "account"),
    ).toBe(true);
    // The account is a shared gap, not something an individual Surface can fix.
    expect(readiness.surfaceGaps).toBe(0);
    // …and `ownGap` says so, so the Surface card cannot claim a badge for it.
    expect(
      readiness.surfaces
        .filter((surface) => surface.enabled)
        .every((surface) => surface.ownGap !== "account"),
    ).toBe(true);
  });

  it("clears once the shared connection is configured", () => {
    const draft = fillModels({
      ...createProviderBundleDraft(familyById("family.openai_oauth")!),
      accountId: "account-1",
    });
    const readiness = bundleReadiness(draft);
    expect(readiness.connection).toBeNull();
    expect(readiness.ready).toBe(true);
  });

  it("reports a missing shared credential", () => {
    const base = createProviderBundleDraft(familyById("family.openrouter")!);
    expect(Object.keys(base.secrets).length).toBeGreaterThan(0);
    expect(bundleReadiness(base).connection).toBe("credential");

    const filled = fillModels({
      ...base,
      secrets: Object.fromEntries(
        Object.entries(base.secrets).map(([slot, secret]) => [
          slot,
          { ...secret, value: "sk-test" },
        ]),
      ),
    });
    expect(bundleReadiness(filled).connection).toBeNull();
    expect(bundleReadiness(filled).ready).toBe(true);
  });

  it("reports per-Surface gaps for Surface-scoped endpoints", () => {
    const draft = createProviderBundleDraft(familyById("family.custom_http")!);
    const readiness = bundleReadiness(draft);
    expect(readiness.connection).toBeNull();
    expect(
      readiness.surfaces
        .filter((surface) => surface.enabled)
        .every((surface) => surface.gap !== null),
    ).toBe(true);
    expect(readiness.surfaceGaps).toBe(
      draft.surfaces.filter((surface) => surface.enabled).length,
    );

    const filled = fillModels({
      ...draft,
      surfaces: draft.surfaces.map((surface) => ({
        ...surface,
        endpoint: "https://example.com/v1",
        secret: { ...surface.secret, value: "sk-test" },
      })),
    });
    expect(bundleReadiness(filled).ready).toBe(true);
  });

  it("does not ask a disabled Surface for anything", () => {
    const draft = createProviderBundleDraft(familyById("family.custom_http")!);
    const readiness = bundleReadiness({
      ...draft,
      surfaces: draft.surfaces.map((surface, index) =>
        index === 0 ? { ...surface, enabled: false } : surface,
      ),
    });
    expect(readiness.surfaces[0]).toMatchObject({
      enabled: false,
      gap: null,
      ownGap: null,
    });
  });

  it("is not ready when no Surface is enabled", () => {
    const draft = fillModels(
      createProviderBundleDraft(familyById("family.openrouter")!),
    );
    const readiness = bundleReadiness({
      ...draft,
      secrets: Object.fromEntries(
        Object.entries(draft.secrets).map(([slot, secret]) => [
          slot,
          { ...secret, value: "sk-test" },
        ]),
      ),
      surfaces: draft.surfaces.map((surface) => ({
        ...surface,
        enabled: false,
      })),
    });
    expect(readiness.ready).toBe(false);
  });
});
