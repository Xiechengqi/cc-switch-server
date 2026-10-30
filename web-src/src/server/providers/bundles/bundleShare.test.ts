import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ShareRecord } from "@/lib/api/share";
import { PERMANENT_EXPIRES_AT } from "@/utils/shareUtils";

const shareApiMock = vi.hoisted(() => ({
  delete: vi.fn(),
  disable: vi.fn(),
  saveProviderBundleShare: vi.fn(),
  suggestShareSlug: vi.fn(),
  preflightProviderBundleShare: vi.fn(),
}));

vi.mock("@/lib/api/share", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/lib/api/share")>();
  return { ...original, shareApi: shareApiMock };
});

import {
  BUNDLE_SHARE_EXPIRY_PRESETS,
  createBundleShareDraft,
  changedBundleUserGrants,
  deleteBundleShare,
  enableBundleShare,
  isValidShareSlug,
  saveBundleShare,
  shareForBundle,
  toggleBundleShare,
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
    freeAccess: false,
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
  it("does not treat dynamic quota refreshes as administrator edits", () => {
    const previous: NonNullable<ShareRecord["userGrants"]> = { "u@example.com": { email: "u@example.com", active: true,
      role: "shareto", policy: { tokenPeriod: "accountReset", tokenLimit: 100 },
      accountResetCapability: { support: "supported", syncStatus: "ready", minimumContractVersion: 8 } } };
    const refreshed = structuredClone(previous);
    refreshed["u@example.com"].accountResetCapability!.syncStatus = "stale";
    refreshed["u@example.com"].usageQuota = { period: "accountReset", windowStartsAtMs: 4123,
      effectiveTokensUsed: 90, observedTokensUsed: 90, manualOffsetTokens: 0, observedRequestsCount: 2,
      rebaseApplies: false, usageWatermark: 10 };
    expect(changedBundleUserGrants(previous, refreshed)).toBeUndefined();
    refreshed["u@example.com"].policy.tokenLimit = 200;
    expect(changedBundleUserGrants(previous, refreshed)?.["u@example.com"].policy?.tokenLimit).toBe(200);
  });
  it("freezes the draft revision and exact expiry while sending only changed users", async () => {
    const existing = share({ codex: "bundle-1" }, 7, "2090-10-26T12:34:56Z");
    existing.userGrants = {
      "a@example.com": { email: "a@example.com", role: "shareto", active: true, revision: 3, policy: { parallelLimit: 2 } },
      "b@example.com": { email: "b@example.com", role: "shareto", active: true, revision: 4, policy: { parallelLimit: 8 } },
    };
    const draft = createBundleShareDraft(existing);
    draft.userGrants["a@example.com"] = { ...draft.userGrants["a@example.com"], policy: { parallelLimit: 5 } };
    await saveBundleShare("bundle-1", draft, { ...existing, configRevision: 8 });
    const sent = shareApiMock.saveProviderBundleShare.mock.calls.at(-1)?.[0];
    expect(sent.expectedConfigRevision).toBe(7);
    expect(sent.expiresAt).toBe(existing.expiresAt);
    expect(sent.userGrants).toBeUndefined();
    expect(sent.userGrantEdits).toEqual({ "a@example.com": { expectedRevision: 3, policy: { parallelLimit: 5 } } });
    expect(sent.userUsageEdits).toEqual({});
  });
  beforeEach(() => {
    vi.clearAllMocks();
    shareApiMock.preflightProviderBundleShare.mockResolvedValue({ canShare: true });
  });

  it("checks server-owned eligibility before enabling, without writing a rejected Share", async () => {
    shareApiMock.preflightProviderBundleShare.mockResolvedValue({ canShare: false, reason: "credential_mismatch" });
    const draft = createBundleShareDraft();
    draft.enabled = true;
    draft.subdomain = "custom-share";
    await expect(saveBundleShare("custom-bundle", draft)).rejects.toThrow();
    expect(shareApiMock.preflightProviderBundleShare).toHaveBeenCalledWith("custom-bundle");
    expect(shareApiMock.saveProviderBundleShare).not.toHaveBeenCalled();
    // Disabling must not be prevented by an enable-only advisory preflight.
    draft.enabled = false;
    await saveBundleShare("custom-bundle", draft);
    expect(shareApiMock.preflightProviderBundleShare).toHaveBeenCalledTimes(1);
    expect(shareApiMock.saveProviderBundleShare).toHaveBeenCalledTimes(1);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("creates a new Share as private by default", () => {
    expect(createBundleShareDraft()).toMatchObject({
      enabled: false,
      freeAccess: false,
      tokenLimit: "",
      parallelLimit: "",
      expiry: "permanent",
      userGrants: {},
      allowPersonalCredits: false,
      autoConsumeBankedReset: false,
      previousResponseCacheEnabled: true,
    });
  });

  it("uses canonical user grants", () => {
    const existing = share({ claude: "bundle-1" });
    existing.userGrants = {
      "owner@example.com": {
        email: "owner@example.com",
        role: "owner",
        active: true,
        policy: {},
      },
      "current@example.com": {
        email: "current@example.com",
        role: "shareto",
        active: true,
        policy: {},
      },
    };

    expect(Object.keys(createBundleShareDraft(existing).userGrants).sort()).toEqual([
      "current@example.com",
      "owner@example.com",
    ]);
  });

  it("matches the Router share slug contract before saving", () => {
    expect(isValidShareSlug("share-1")).toBe(true);
    expect(isValidShareSlug("a12345")).toBe(true);
    expect(isValidShareSlug("short")).toBe(false);
    expect(isValidShareSlug("Uppercase")).toBe(false);
    expect(isValidShareSlug("share--one")).toBe(false);
    expect(isValidShareSlug("share-")).toBe(false);
    expect(isValidShareSlug("router")).toBe(false);
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
        freeAccess: false,
      }),
    );
    expect(
      shareApiMock.saveProviderBundleShare.mock.calls[0]?.[0],
    ).not.toHaveProperty("bindings");
  });

  it("quick-enables a Bundle Share with a suggested slug", async () => {
    const created = share({ claude: "bundle-1" });
    shareApiMock.suggestShareSlug.mockResolvedValue({
      subdomain: "quick-share",
    });
    shareApiMock.saveProviderBundleShare.mockResolvedValue(created);

    const result = await enableBundleShare("bundle-1");

    expect(result).toBe(created);
    expect(shareApiMock.suggestShareSlug).toHaveBeenCalledTimes(1);
    expect(shareApiMock.saveProviderBundleShare).toHaveBeenCalledWith(
      expect.objectContaining({
        bundleId: "bundle-1",
        enabled: true,
        subdomain: "quick-share",
      }),
    );
  });

  it("quick-resumes an existing Bundle Share without replacing its slug", async () => {
    const existing = {
      ...share({ claude: "bundle-1" }, 7),
      status: "paused",
    };
    const resumed = { ...existing, status: "active", configRevision: 8 };
    shareApiMock.saveProviderBundleShare.mockResolvedValue(resumed);

    await enableBundleShare("bundle-1", existing);

    expect(shareApiMock.suggestShareSlug).not.toHaveBeenCalled();
    expect(shareApiMock.saveProviderBundleShare).toHaveBeenCalledWith(
      expect.objectContaining({
        shareId: "share-1",
        expectedConfigRevision: 7,
        enabled: true,
        subdomain: "bundle-share",
      }),
    );
  });

  it("stops a running Bundle Share through the existing disable command", async () => {
    const existing = {
      ...share({ claude: "bundle-1" }, 7),
      subdomain: "bundle-share",
      tunnelUrl: "https://bundle-share.example.com",
    };
    shareApiMock.disable.mockResolvedValue(undefined);

    const result = await toggleBundleShare("bundle-1", existing);

    expect(result).toBe("disabled");
    expect(shareApiMock.disable).toHaveBeenCalledWith("share-1");
    expect(shareApiMock.saveProviderBundleShare).not.toHaveBeenCalled();
  });

  it("resumes a stopped Bundle Share through the Bundle-scoped command", async () => {
    const existing = {
      ...share({ claude: "bundle-1" }, 7),
      status: "paused",
      tunnelUrl: null,
    };
    shareApiMock.saveProviderBundleShare.mockResolvedValue({
      ...existing,
      status: "active",
    });

    const result = await toggleBundleShare("bundle-1", existing);

    expect(result).toBe("enabled");
    expect(shareApiMock.disable).not.toHaveBeenCalled();
    expect(shareApiMock.saveProviderBundleShare).toHaveBeenCalledWith(
      expect.objectContaining({
        bundleId: "bundle-1",
        shareId: "share-1",
        enabled: true,
      }),
    );
  });

  it("deletes a stopped Bundle Share through the existing delete command", async () => {
    const existing = {
      ...share({ claude: "bundle-1" }),
      status: "paused",
    };
    shareApiMock.delete.mockResolvedValue(undefined);

    await deleteBundleShare(existing);

    expect(shareApiMock.delete).toHaveBeenCalledWith("share-1");
  });

  it("does not send a Share-total usage correction from the provider share form", async () => {
    const existing = share({ claude: "bundle-1" });
    existing.tokensUsed = 1234;
    shareApiMock.saveProviderBundleShare.mockResolvedValue(existing);

    const draft = createBundleShareDraft(existing);
    expect(draft).not.toHaveProperty("tokensUsed");

    await saveBundleShare("bundle-1", draft, existing);

    expect(
      shareApiMock.saveProviderBundleShare.mock.calls[0]?.[0],
    ).not.toHaveProperty("shareUsageEdit");
  });

  it("persists per-email user limits through the Bundle command", async () => {
    shareApiMock.saveProviderBundleShare.mockResolvedValue(undefined);
    const draft = createBundleShareDraft();
    draft.enabled = true;
    draft.subdomain = "bundle-share";
    draft.userGrants = {
      "friend@example.com": {
        email: "friend@example.com",
        role: "shareto",
        active: true,
        policy: {
          parallelLimit: 2,
          tokenLimit: 10_000,
          tokenPeriod: "day",
        },
      },
    };

    await saveBundleShare("bundle-1", draft);

    expect(shareApiMock.saveProviderBundleShare).toHaveBeenCalledWith(
      expect.objectContaining({
        userGrants: draft.userGrants,
      }),
    );
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
