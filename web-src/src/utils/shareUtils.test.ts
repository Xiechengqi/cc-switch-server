import { describe, expect, it } from "vitest";
import {
  formatShareLimitInput,
  getShareDisplayStatus,
  isUnlimitedParallelLimit,
  isUnlimitedTokenLimit,
  normalizeShareLimitValue,
  UNLIMITED_LIMIT_SENTINEL,
  UNLIMITED_TOKEN_LIMIT,
} from "./shareUtils";
import { normalizeShareRecord } from "./shareRecordNormalize";

describe("shareUtils limits", () => {
  it("treats MAX_SAFE_INTEGER as unlimited", () => {
    expect(normalizeShareLimitValue(UNLIMITED_LIMIT_SENTINEL)).toBe(
      UNLIMITED_TOKEN_LIMIT,
    );
    expect(isUnlimitedTokenLimit(UNLIMITED_LIMIT_SENTINEL)).toBe(true);
    expect(isUnlimitedParallelLimit(UNLIMITED_LIMIT_SENTINEL)).toBe(true);
    expect(formatShareLimitInput(UNLIMITED_LIMIT_SENTINEL)).toBe("");
  });

  it("keeps finite limits unchanged", () => {
    expect(normalizeShareLimitValue(100_000)).toBe(100_000);
    expect(formatShareLimitInput(100_000)).toBe("100000");
  });

  it("normalizes share records with legacy unlimited sentinel", () => {
    const share = normalizeShareRecord({
      id: "share-1",
      tokenLimit: UNLIMITED_LIMIT_SENTINEL,
      parallelLimit: UNLIMITED_LIMIT_SENTINEL,
      bindings: { claude: "provider-1" },
    });

    expect(share?.tokenLimit).toBe(UNLIMITED_TOKEN_LIMIT);
    expect(share?.parallelLimit).toBe(UNLIMITED_TOKEN_LIMIT);
  });

  it("reads canonical freeAccess without legacy sale projection", () => {
    const share = normalizeShareRecord({
      id: "share-private",
      bindings: { codex: "provider-1" },
      freeAccess: false,
    });

    expect(share?.freeAccess).toBe(false);
  });
});

describe("getShareDisplayStatus", () => {
  it("falls back to sharing when tunnel probe is missing but share is running", () => {
    expect(
      getShareDisplayStatus(
        {
          status: "active",
          tunnelUrl: "https://share-abc.example.com",
        },
        true,
        null,
      ),
    ).toBe("sharing");
  });
});

describe("account quota roundtrip", () => {
  const cycle = { streamIncarnation: "stream", sourceRef: "opaque", sourceEpoch: 1, bucketId: "weekly",
    cycleId: "cycle", cycleSequence: 2, cycleRevision: 3, startsAtMs: 4123, endsAtMs: null,
    basis: "publicAnnouncement", observedAtMs: 5000, eventId: "public:event" };
  function record(accountQuotaCycle: unknown = cycle) {
    return { id: "share-account", bindings: { codex: "provider" }, userGrants: { "u@example.com": {
      email: "u@example.com", active: true, role: "shareto", policy: { tokenPeriod: "accountReset", tokenLimit: 100 },
      accountQuotaCycle, accountResetCapability: { support: "supported", syncStatus: "stale", minimumContractVersion: 8 },
      accountUsageBaseline: { sourceRef: "opaque", cycleId: "cycle", targetTokens: 10, usageWatermark: 5, appliedAtMs: 4500 },
      usageQuota: { period: "accountReset", windowStartsAtMs: 4123, effectiveTokensUsed: 20, observedTokensUsed: 30,
        manualOffsetTokens: -10, observedRequestsCount: 2, rebaseApplies: true, usageWatermark: 7 },
    } } };
  }
  it("keeps millisecond/open boundaries, baseline and exact watermark across export/import", () => {
    const normalized = normalizeShareRecord(JSON.parse(JSON.stringify(record())));
    const grant = normalized?.userGrants?.["u@example.com"];
    expect(grant?.policy.tokenPeriod).toBe("accountReset");
    expect(grant?.accountQuotaCycle).toEqual(cycle);
    expect(grant?.accountUsageBaseline?.usageWatermark).toBe(5);
    expect(grant?.usageQuota?.usageWatermark).toBe(7);
    expect(grant?.accountResetCapability?.syncStatus).toBe("stale");
  });
  it.each([{ ...cycle, sourceEpoch: 0 }, { ...cycle, cycleRevision: 0 }, { ...cycle, startsAtMs: 4123.5 },
    { ...cycle, endsAtMs: 4123 }, { ...cycle, observedAtMs: 4000 }, { ...cycle, basis: "expiry" }])(
    "does not normalize malformed boundaries into usable quota: %j", (invalid) => {
      expect(normalizeShareRecord(record(invalid))?.userGrants?.["u@example.com"].accountQuotaCycle).toBeUndefined();
    });
});
