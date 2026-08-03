import { describe, expect, it, vi } from "vitest";
import type { CachedOauthQuota } from "@/lib/api/subscription";
import {
  formatOauthQuotaRetryDelay,
  oauthQuotaSnapshotFromEnvelope,
  refreshOauthQuotaAndReload,
} from "./oauthQuotaSnapshot";

const quotaEnvelope: CachedOauthQuota = {
  authProvider: "claude_oauth",
  accountId: "account-1",
  authIdentityGeneration: 3,
  quota: {
    tool: "claude_oauth",
    credentialStatus: "valid",
    credentialMessage: null,
    subscription: null,
    success: false,
    quotaStatus: null,
    warningCodes: [],
    warnings: [],
    staleTierNames: [],
    tiers: [],
    extraUsage: null,
    error: "upstream unavailable",
    queriedAt: null,
  },
  refreshedAt: 1_000,
  nextRefreshAt: 61_000,
  source: "server",
};

describe("oauthQuotaSnapshotFromEnvelope", () => {
  it("keeps cache timestamps with the quota payload", () => {
    expect(oauthQuotaSnapshotFromEnvelope(quotaEnvelope)).toMatchObject({
      queriedAt: 1_000,
      refreshedAt: 1_000,
      nextRefreshAt: 61_000,
      authProvider: "claude_oauth",
      accountId: "account-1",
      authIdentityGeneration: 3,
      error: "upstream unavailable",
    });
  });

  it("rejects a response from another identity generation", () => {
    expect(() =>
      oauthQuotaSnapshotFromEnvelope(quotaEnvelope, {
        accountId: "account-1",
        authIdentityGeneration: 4,
      }),
    ).toThrow("identity changed");
  });
});

describe("formatOauthQuotaRetryDelay", () => {
  it("formats active cooldowns and omits expired ones", () => {
    expect(formatOauthQuotaRetryDelay(10_500, 10_000)).toBe("1s");
    expect(formatOauthQuotaRetryDelay(70_000, 10_000)).toBe("1m");
    expect(formatOauthQuotaRetryDelay(3_670_000, 10_000)).toBe("1h1m");
    expect(formatOauthQuotaRetryDelay(10_000, 10_000)).toBeNull();
  });
});

describe("refreshOauthQuotaAndReload", () => {
  it("reloads persisted cooldown state after refresh failure", async () => {
    const refreshError = new Error("refresh failed");
    const reload = vi.fn().mockResolvedValue(undefined);

    await expect(
      refreshOauthQuotaAndReload(
        vi.fn().mockRejectedValue(refreshError),
        reload,
      ),
    ).rejects.toBe(refreshError);
    expect(reload).toHaveBeenCalledOnce();
  });

  it("treats a resolved TanStack refetch error as a reload failure", async () => {
    const reloadError = new Error("reload failed");

    await expect(
      refreshOauthQuotaAndReload(
        vi.fn().mockResolvedValue(undefined),
        vi.fn().mockResolvedValue({ isError: true, error: reloadError }),
      ),
    ).rejects.toBe(reloadError);
  });
});
