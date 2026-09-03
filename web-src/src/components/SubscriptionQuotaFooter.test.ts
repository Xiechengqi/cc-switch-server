import { describe, expect, it } from "vitest";

import {
  formatBankedResetSummary,
  formatQuotaSummary,
} from "./SubscriptionQuotaFooter";
import type { SubscriptionQuota } from "@/types/subscription";

describe("SubscriptionQuotaFooter summaries", () => {
  it("shows a read-only Banked Reset count and distinct expiries", () => {
    const now = Date.parse("2026-08-24T00:00:00Z");
    const summary = formatBankedResetSummary(
      {
        availableCount: 2,
        detailsAvailable: true,
        detailsStale: false,
        nextExpiresAt: "2026-08-25T01:00:00Z",
        credits: [
          { status: "available", expiresAt: "2026-08-25T01:00:00Z" },
          { status: "available", expiresAt: "2026-08-26T01:00:00Z" },
        ],
      },
      undefined,
      now,
    );

    expect(summary).toBe("Banked Reset 2 · expires 1d1h / 2d1h");
  });

  it("marks count-only Banked Reset snapshots as lacking details", () => {
    expect(
      formatBankedResetSummary({
        availableCount: 3,
        detailsAvailable: false,
        detailsStale: false,
        credits: [],
      }),
    ).toBe("Banked Reset 3 · details unavailable");
  });

  it("keeps Grok weekly and monthly period resets in the shared footer summary", () => {
    const now = Date.parse("2026-08-24T00:00:00Z");
    const quota: SubscriptionQuota = {
      tool: "grok_oauth",
      credentialStatus: "valid",
      credentialMessage: "SuperGrok",
      success: true,
      tiers: [
        {
          name: "grok_weekly",
          utilization: 25,
          resetsAt: "2026-08-24T02:00:00Z",
        },
        {
          name: "grok_monthly",
          utilization: 50,
          resetsAt: "2026-08-27T00:00:00Z",
        },
      ],
      extraUsage: null,
      error: null,
      queriedAt: now,
    };

    const summary = formatQuotaSummary(quota, quota.tiers, undefined, now);

    expect(summary).toContain("Weekly 25% 2h0m");
    expect(summary).toContain("Monthly 50% 3d0h");
    expect(quota.bankedReset).toBeUndefined();
  });

  it("includes the Claude Fable model-family pool beside the shared windows", () => {
    const now = Date.parse("2026-09-03T00:00:00Z");
    const quota: SubscriptionQuota = {
      tool: "claude_oauth",
      credentialStatus: "valid",
      credentialMessage: "Claude Max 20x",
      success: true,
      tiers: [
        {
          name: "five_hour",
          utilization: 3,
          resetsAt: "2026-09-03T04:22:00Z",
        },
        {
          name: "seven_day",
          utilization: 24,
          resetsAt: "2026-09-08T09:00:00Z",
        },
        {
          name: "seven_day_fable",
          utilization: 41,
          resetsAt: "2026-09-08T09:00:00Z",
          scope: "model_family",
          capacityPool: "claude_fable_7d_oi",
          modelFamily: "claude-fable-5",
          relativeWeeklyCapacity: 0.5,
          source: "anthropic_ratelimit_7d_oi",
        },
      ],
      extraUsage: null,
      error: null,
      queriedAt: now,
    };

    const summary = formatQuotaSummary(quota, quota.tiers, undefined, now);

    expect(summary).toContain("5h 3% 4h22m");
    expect(summary).toContain("7d 24% 5d9h");
    expect(summary).toContain("Fable 7d 41% 5d9h");
  });
});
