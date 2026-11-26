import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ShareRecord } from "@/lib/api/share";
import { PERMANENT_EXPIRES_AT } from "@/utils/shareUtils";
import { normalizeShareRecord } from "@/utils/shareRecordNormalize";
import i18n from "@/i18n";

const shareApiMock = vi.hoisted(() => ({
  delete: vi.fn(),
  disable: vi.fn(),
  saveProviderBundleShare: vi.fn(),
  suggestShareSlug: vi.fn(),
  preflightProviderBundleShare: vi.fn(),
  previewGrantCapabilities: vi.fn(),
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
  MalformedValueQuotaGrantError,
  saveBundleShare,
  shareForBundle,
  shareLimitInput,
  shareLimitState,
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
  describe("USD-equivalent edits", () => {
    const email = "u@example.com";
    const usd = (micros: string) => ({ limitUsdMicros: micros, tariffPolicyId: "api-equivalent-v1" as const });
    const grants = (policy: NonNullable<ShareRecord["userGrants"]>[string]["policy"],
      extra: Partial<NonNullable<ShareRecord["userGrants"]>[string]> = {}): NonNullable<ShareRecord["userGrants"]> =>
      ({ [email]: { email, role: "shareto", active: true, revision: 5, policy, ...extra } });

    it("sends Tokens unlimited → US$10", () => {
      const edits = changedBundleUserGrants(grants({ tokenPeriod: "day" }), grants({ tokenPeriod: "day", valueQuota: usd("10000000") }));
      expect(edits).toEqual({ [email]: { expectedRevision: 5, policy: { tokenPeriod: "day", valueQuota: usd("10000000") } } });
    });
    it("sends US$10 → US$20", () => {
      const edits = changedBundleUserGrants(grants({ tokenPeriod: "day", valueQuota: usd("10000000") }),
        grants({ tokenPeriod: "day", valueQuota: usd("20000000") }));
      expect(edits?.[email].policy?.valueQuota).toEqual(usd("20000000"));
    });
    it("sends US$10 → Tokens unlimited", () => {
      const edits = changedBundleUserGrants(grants({ tokenPeriod: "day", valueQuota: usd("10000000") }), grants({ tokenPeriod: "day" }));
      expect(edits).toEqual({ [email]: { expectedRevision: 5, policy: { tokenPeriod: "day" } } });
    });
    it("treats a dropped schedule as a change and an untouched schedule as none", () => {
      const pending = { policy: { tokenPeriod: "day" as const, valueQuota: usd("10000000") }, effectiveFromMs: 9, retainAccess: true };
      const baseline = grants({ tokenPeriod: "day", tokenLimit: 100 }, { pendingValueQuota: pending });
      expect(changedBundleUserGrants(baseline, structuredClone(baseline))).toBeUndefined();
      const cancelled = grants({ tokenPeriod: "day", tokenLimit: 100 });
      expect(changedBundleUserGrants(baseline, cancelled)).toEqual({ [email]: { expectedRevision: 5, policy: { tokenPeriod: "day", tokenLimit: 100 } } });
    });
    it("refuses to re-send a grant whose USD fields could not be read, but allows revoking it", () => {
      const baseline = grants({ tokenPeriod: "day", valueQuota: { limitUsdMicros: "01" } as never }, { valueQuotaMalformed: true });
      expect(changedBundleUserGrants(baseline, structuredClone(baseline))).toBeUndefined();
      const edited = grants({ tokenPeriod: "day", parallelLimit: 2, valueQuota: { limitUsdMicros: "01" } as never }, { valueQuotaMalformed: true });
      expect(() => changedBundleUserGrants(baseline, edited)).toThrow(MalformedValueQuotaGrantError);
      expect(changedBundleUserGrants(baseline, {})).toEqual({ [email]: { expectedRevision: 5, policy: null } });
    });
    it("does not save a usage edit for a grant whose USD fields could not be read", async () => {
      const existing = share({ codex: "bundle-1" });
      existing.userGrants = grants({ tokenPeriod: "day" }, { pendingValueQuota: "broken" as never, valueQuotaMalformed: true });
      const draft = createBundleShareDraft(existing);
      draft.userUsageEdits = { [email]: { action: "clear" } };
      await expect(saveBundleShare("bundle-1", draft, existing)).rejects.toThrow(MalformedValueQuotaGrantError);
      expect(shareApiMock.saveProviderBundleShare).not.toHaveBeenCalled();
    });
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
  it("saves opening USD usage with the new grant and reuses its operation ID on retry", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-11-03T00:00:00Z"));
    const existing = share({codex: "bundle-1"}, 7);
    const draft = createBundleShareDraft(existing);
    const opening = {targetPicos: "123456789123", windowId: "Lifetime:0:0", reason: "Opening usage"};
    draft.userGrants["new@example.test"] = {email: "new@example.test", role: "shareto", active: true,
      policy: {tokenPeriod: "lifetime", valueQuota: {limitUsdMicros: "1000000", tariffPolicyId: "api-equivalent-v1"}}};
    draft.userValueUsageInitializations = {"new@example.test": opening, "removed@example.test": opening};
    draft.expiry = "1h";
    await saveBundleShare("bundle-1", draft, existing);
    vi.setSystemTime(new Date("2026-11-03T00:00:10Z"));
    await saveBundleShare("bundle-1", draft, existing);
    const sent = shareApiMock.saveProviderBundleShare.mock.calls.at(-1)?.[0];
    expect(sent.userValueUsageInitializations).toEqual({"new@example.test": opening});
    expect(sent.userGrantEdits["new@example.test"].expectedRevision).toBe(0);
    expect(sent.userUsageEdits).toEqual({});
    expect(sent.valueUsageOperationId).toBe(draft.valueUsageOperationId);
    expect(shareApiMock.saveProviderBundleShare.mock.calls.at(-2)?.[0].valueUsageOperationId).toBe(sent.valueUsageOperationId);
    expect(shareApiMock.saveProviderBundleShare.mock.calls.at(-2)?.[0]).toEqual(sent);
    expect(createBundleShareDraft(existing).userValueUsageInitializations).toEqual({});
  });
  beforeEach(() => {
    vi.clearAllMocks();
    shareApiMock.preflightProviderBundleShare.mockResolvedValue({ canShare: true });
  });

  it("persists a staged permission revocation when the Share is paused in the same save", async () => {
    const existing = share({ codex: "bundle-1" }, 7);
    const email = "u@example.com";
    existing.grantCapabilityPolicy = true;
    existing.userGrants = {
      [email]: { email, role: "shareto", active: true, revision: 3,
        policy: { tokenPeriod: "lifetime", allowedOperations: ["tool.web_search"] } },
    };
    const draft = createBundleShareDraft(existing);
    draft.enabled = false;
    draft.userGrants[email] = { ...draft.userGrants[email], policy: { tokenPeriod: "lifetime", allowedOperations: [] } };
    shareApiMock.previewGrantCapabilities.mockResolvedValue({
      ready: true, expiresAt: Date.now() + 60_000, previewCredential: "revocation-review",
    });
    shareApiMock.saveProviderBundleShare.mockResolvedValue({ ...existing, userGrants: draft.userGrants });
    await saveBundleShare("bundle-1", draft, existing);
    expect(shareApiMock.preflightProviderBundleShare).not.toHaveBeenCalled();
    expect(shareApiMock.saveProviderBundleShare).toHaveBeenCalledWith(expect.objectContaining({
      enabled: false,
      grantCapabilityEdits: { [email]: { operations: [], expectedRevision: 3 } },
      capabilityConsent: { credential: "revocation-review", acceptUncovered: [] },
    }));
  });

  it("saves and reopens only changed legacy Grok media flags", async () => {
    const raw = share({ codex: "bundle-1" });
    const original = { imageGenerationEnabled: true, imageEditEnabled: false, videoGenerationEnabled: true };
    const existing = normalizeShareRecord({ ...raw, grok_media_policy: {
      image_generation_enabled: true, image_edit_enabled: false, video_generation_enabled: true,
    } })!;
    const draft = createBundleShareDraft(existing);
    expect(draft.grokMediaPolicy).toEqual(original);
    expect(normalizeShareRecord({ ...raw, grokMediaPolicy: { imageEditEnabled: true } })?.grokMediaPolicy)
      .toEqual({ imageGenerationEnabled: false, imageEditEnabled: true, videoGenerationEnabled: false });
    await saveBundleShare("bundle-1", { ...draft, description: "renamed" }, existing);
    expect(shareApiMock.saveProviderBundleShare.mock.lastCall?.[0]).not.toHaveProperty("grokMediaPolicy");
    const changed = { imageGenerationEnabled: false, imageEditEnabled: true, videoGenerationEnabled: false };
    draft.grokMediaPolicy = changed;
    for (const response of [existing, undefined, normalizeShareRecord({
      ...raw, grokMediaPolicy: { ...changed, imageGenerationEnabled: "false" },
    })]) {
      shareApiMock.saveProviderBundleShare.mockResolvedValueOnce(response);
      await expect(saveBundleShare("bundle-1", draft, existing)).rejects.toThrow(i18n.t("share.grantMedia.saveMismatch"));
    }
    const response = normalizeShareRecord({ ...raw, grokMediaPolicy: changed });
    shareApiMock.saveProviderBundleShare.mockResolvedValueOnce(response);
    expect(createBundleShareDraft(await saveBundleShare("bundle-1", draft, existing)).grokMediaPolicy).toEqual(changed);
    expect(shareApiMock.saveProviderBundleShare.mock.lastCall?.[0].grokMediaPolicy).toEqual(changed);
    expect(original).toEqual(existing.grokMediaPolicy);
  });

  it("confirms v14 media operations from the intended user's saved grant", async () => {
    const email = "friend@example.com";
    const existing = share({ codex: "bundle-1" });
    existing.grantCapabilityPolicy = true;
    existing.userGrants = {
      [email]: { email, active: true, role: "shareto", revision: 2, policy: { tokenPeriod: "day", allowedOperations: [] } },
    };
    const draft = createBundleShareDraft(existing);
    const grant = { ...draft.userGrants[email], policy: { ...draft.userGrants[email].policy, allowedOperations: ["image.edit" as const] } };
    draft.userGrants[email] = grant;
    shareApiMock.previewGrantCapabilities.mockResolvedValue({
      ready: true, expiresAt: Date.now() + 60_000, previewCredential: "media-review",
    });
    const saved = { ...existing, userGrants: { [email]: grant } };
    for (const response of [
      existing,
      { ...saved, id: "other-share" },
      { ...saved, userGrants: { "other@example.com": { ...grant, email: "other@example.com" } } },
      { ...saved, userGrants: { [email]: { ...grant, manager: "routerShareMarket" } } },
    ]) {
      shareApiMock.saveProviderBundleShare.mockResolvedValueOnce(response);
      await expect(saveBundleShare("bundle-1", draft, existing)).rejects.toThrow(i18n.t("share.grantMedia.saveMismatch"));
    }
    shareApiMock.saveProviderBundleShare.mockResolvedValueOnce(normalizeShareRecord(saved));
    const reopened = createBundleShareDraft(await saveBundleShare("bundle-1", draft, existing));
    expect(reopened.userGrants[email].policy.allowedOperations).toEqual(["image.edit"]);
    const sent = shareApiMock.saveProviderBundleShare.mock.lastCall?.[0];
    expect(sent.grantCapabilityEdits).toEqual({ [email]: { expectedRevision: 2, operations: ["image.edit"] } });
    expect(sent).not.toHaveProperty("grokMediaPolicy");
  });

  it("omits the global policy on create and for both generations of capability policies", async () => {
    const raw = share({ codex: "bundle-1" });
    const grokMediaPolicy = { imageGenerationEnabled: true, imageEditEnabled: true, videoGenerationEnabled: true };
    for (const existing of [undefined,
      normalizeShareRecord({ ...raw, grokMediaPolicy, grantCapabilityPolicy: true })!,
      normalizeShareRecord({ ...raw, grokMediaPolicy, capabilityPolicy: { version: 1, allowedOperations: [], operationBindings: {} } })!]) {
      const draft = createBundleShareDraft(existing);
      draft.enabled = true;
      expect(draft.baseline?.legacyGrokMediaPolicy).toBeUndefined();
      await saveBundleShare("bundle-1", draft, existing);
      expect(shareApiMock.saveProviderBundleShare.mock.lastCall?.[0]).not.toHaveProperty("grokMediaPolicy");
    }
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

  it("keeps a saved expiry exactly and counts every preset from the save", async () => {
    vi.useFakeTimers();
    const now = new Date("2026-08-04T12:00:00Z");
    vi.setSystemTime(now);
    shareApiMock.saveProviderBundleShare.mockResolvedValue(undefined);

    for (const preset of BUNDLE_SHARE_EXPIRY_PRESETS) {
      const expiresAt = new Date(
        now.getTime() + preset.seconds * 1000,
      ).toISOString();
      // A saved date is shown as itself, not rounded to the nearest preset.
      const saved = share({ claude: "bundle-1" }, 1, "2026-08-05T03:21:00Z");
      const kept = createBundleShareDraft(saved);
      expect(kept.expiry).toBe("current");
      await saveBundleShare("bundle-1", kept, saved);
      expect(shareApiMock.saveProviderBundleShare.mock.calls.at(-1)?.[0].expiresAt).toBe(saved.expiresAt);
      // Choosing the same duration again renews from now.
      const renewed = createBundleShareDraft(saved);
      renewed.expiry = preset.value;
      await saveBundleShare("bundle-1", renewed, saved);
      expect(shareApiMock.saveProviderBundleShare.mock.calls.at(-1)?.[0].expiresAt).toBe(expiresAt);

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

  it("refuses a zero Share-wide limit instead of saving a Share that stops at once", async () => {
    for (const field of ["tokenLimit", "parallelLimit"] as const) {
      const draft = createBundleShareDraft();
      draft[field] = "0";
      await expect(saveBundleShare("bundle-1", draft)).rejects.toThrow();
    }
    expect(shareLimitInput("")).toBe("unlimited");
    expect(shareLimitInput("0")).toBe("invalid");
    expect(shareLimitInput("1.5")).toBe("invalid");
    expect(shareLimitInput(" 12 ")).toBe(12);
  });

  it("saves a stored zero limit untouched but refuses one typed anew", async () => {
    shareApiMock.saveProviderBundleShare.mockResolvedValue(undefined);
    const stored = { ...share({ claude: "bundle-1" }, 3), tokenLimit: 0, parallelLimit: 0 };
    const draft = createBundleShareDraft(stored);
    expect(shareLimitState(draft.tokenLimit, draft.baseline?.tokenLimit)).toBe("legacy");
    // Untouched: saves the stored 0s as they are.
    await saveBundleShare("bundle-1", draft, stored);
    expect(shareApiMock.saveProviderBundleShare.mock.calls.at(-1)?.[0]).toEqual(
      expect.objectContaining({ tokenLimit: 0, parallelLimit: 0 }));
    // The list's quick enable goes through the same untouched draft.
    await enableBundleShare("bundle-1", { ...stored, status: "paused" });
    expect(shareApiMock.saveProviderBundleShare.mock.calls.at(-1)?.[0]).toEqual(
      expect.objectContaining({ enabled: true, tokenLimit: 0 }));
    // Edited away and back to 0, or 0 typed into a field that held another value, is refused.
    const edited = createBundleShareDraft({ ...stored, tokenLimit: 5 });
    edited.tokenLimit = "0";
    expect(shareLimitState(edited.tokenLimit, edited.baseline?.tokenLimit)).toBe("invalid");
    await expect(saveBundleShare("bundle-1", edited, stored)).rejects.toThrow();
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
