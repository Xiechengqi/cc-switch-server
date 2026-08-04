import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ShareRecord } from "@/lib/api/share";
import { PERMANENT_EXPIRES_AT } from "@/utils/shareUtils";

const shareApiMock = vi.hoisted(() => ({
  saveProviderBundleShare: vi.fn(),
}));

vi.mock("@/lib/api/share", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/lib/api/share")>();
  return { ...original, shareApi: shareApiMock };
});

import {
  BUNDLE_SHARE_EXPIRY_PRESETS,
  createBundleShareDraft,
  saveBundleShare,
  shareForBundle,
} from "./bundleShare";

function share(
  bindings: ShareRecord["bindings"],
  revision = 1,
  expiresAt = PERMANENT_EXPIRES_AT,
): ShareRecord {
  return {
    id: "share-1",
    capacityPoolId: "pool-1",
    name: "Bundle share",
    ownerEmail: "owner@example.com",
    sharedWithEmails: [],
    marketAccessMode: "selected",
    forSaleOfficialPricePercentByApp: {},
    forSale: "No",
    bindings,
    apiKey: "redacted",
    tokenLimit: -1,
    parallelLimit: -1,
    tokensUsed: 0,
    requestsCount: 0,
    expiresAt,
    shareSlug: "bundle-share",
    status: "active",
    autoStart: true,
    createdAt: "2026-08-03T00:00:00Z",
    configRevision: revision,
    routerSyncedRevision: revision,
    descriptorGeneration: 1,
    routerSyncedDescriptorGeneration: 1,
    userGrants: {},
  };
}

describe("Provider Bundle sharing", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("restores the original defaults for a new Share", () => {
    expect(createBundleShareDraft()).toMatchObject({
      enabled: false,
      forSale: "Yes",
      marketAccessMode: "all",
      tokenLimit: "",
      parallelLimit: "",
      expiry: "permanent",
    });
  });

  it("maps every quick expiry preset to and from an absolute timestamp", async () => {
    vi.useFakeTimers();
    const now = new Date("2026-08-04T12:00:00Z");
    vi.setSystemTime(now);
    shareApiMock.saveProviderBundleShare.mockResolvedValue(undefined);

    for (const preset of BUNDLE_SHARE_EXPIRY_PRESETS) {
      const expiresAt = new Date(
        now.getTime() + preset.seconds * 1000,
      ).toISOString();
      expect(
        createBundleShareDraft(share({ claude: "bundle-1" }, 1, expiresAt))
          .expiry,
      ).toBe(preset.value);

      const draft = createBundleShareDraft();
      draft.expiry = preset.value;
      await saveBundleShare("bundle-1", draft);
      expect(
        shareApiMock.saveProviderBundleShare.mock.calls.at(-1)?.[0].expiresAt,
      ).toBe(expiresAt);
    }

    const permanent = createBundleShareDraft();
    await saveBundleShare("bundle-1", permanent);
    expect(
      shareApiMock.saveProviderBundleShare.mock.calls.at(-1)?.[0].expiresAt,
    ).toBe(PERMANENT_EXPIRES_AT);
  });

  it("finds one Share through any Bundle Surface binding", () => {
    const existing = share({ claude: "bundle-1", codex: "bundle-1" });
    expect(shareForBundle([existing], "bundle-1")).toBe(existing);
    expect(shareForBundle([existing], "bundle-2")).toBeUndefined();
  });

  it("delegates first Share creation to one Bundle-scoped command", async () => {
    const created = share({
      claude: "bundle-1",
      codex: "bundle-1",
      gemini: "bundle-1",
    });
    shareApiMock.saveProviderBundleShare.mockResolvedValue(created);

    const draft = createBundleShareDraft();
    draft.enabled = true;
    draft.subdomain = "bundle-share";
    const result = await saveBundleShare("bundle-1", draft);

    expect(result).toBe(created);
    expect(shareApiMock.saveProviderBundleShare).toHaveBeenCalledTimes(1);
    expect(shareApiMock.saveProviderBundleShare).toHaveBeenCalledWith(
      expect.objectContaining({
        bundleId: "bundle-1",
        enabled: true,
        subdomain: "bundle-share",
        marketAccessMode: "all",
      }),
    );
    expect(
      shareApiMock.saveProviderBundleShare.mock.calls[0]?.[0],
    ).not.toHaveProperty("bindings");
  });

  it("saves binding reconciliation and settings through one atomic command", async () => {
    const existing = share({ claude: "bundle-1" }, 4);
    const saved = share(
      {
        claude: "bundle-1",
        codex: "bundle-1",
        gemini: "bundle-1",
      },
      5,
    );
    shareApiMock.saveProviderBundleShare.mockResolvedValue(saved);

    const draft = createBundleShareDraft(existing);
    draft.description = "Shared Bundle";
    const result = await saveBundleShare("bundle-1", draft, existing);

    expect(result).toBe(saved);
    expect(shareApiMock.saveProviderBundleShare).toHaveBeenCalledWith(
      expect.objectContaining({
        bundleId: "bundle-1",
        shareId: "share-1",
        expectedConfigRevision: 4,
        description: "Shared Bundle",
        marketAccessMode: "selected",
      }),
    );
  });

  it("persists settings and paused state in the same command", async () => {
    const existing = share({ claude: "bundle-1", codex: "bundle-1" }, 9);
    const paused = { ...existing, status: "paused", configRevision: 10 };
    shareApiMock.saveProviderBundleShare.mockResolvedValue(paused);
    const draft = createBundleShareDraft(existing);
    draft.enabled = false;
    draft.description = "Keep while paused";

    const result = await saveBundleShare("bundle-1", draft, existing);

    expect(result).toBe(paused);
    expect(shareApiMock.saveProviderBundleShare).toHaveBeenCalledWith(
      expect.objectContaining({
        enabled: false,
        description: "Keep while paused",
        expectedConfigRevision: 9,
      }),
    );
  });
});
