import type { TFunction } from "i18next";
import { describe, expect, it } from "vitest";

import type { CodingPlanQuotaSnapshot } from "@/lib/api/providers";
import {
  formatCodingPlanQuotaSummary,
  formatCodingPlanQuotaWindow,
} from "./CodingPlanQuotaFooter";

const translations: Record<string, string> = {
  "provider.codingPlanQuota.fiveHour": "5h",
  "provider.codingPlanQuota.weekly": "Weekly",
  "provider.codingPlanQuota.monthly": "Monthly",
  "provider.codingPlanQuota.resetsIn": "resets in {{time}}",
  "provider.codingPlanQuota.unknown": "Unknown",
  "provider.codingPlanQuota.unavailable": "Unavailable",
  "provider.codingPlanQuota.quotaAvailable": "Quota available",
};

const t = ((key: string, options?: Record<string, unknown>) => {
  const template = translations[key] ?? key;
  return template.replace("{{time}}", String(options?.time ?? ""));
}) as TFunction;

function snapshot(
  state: CodingPlanQuotaSnapshot["quota"]["state"],
): CodingPlanQuotaSnapshot {
  return {
    providerKey: { app: "claude", providerId: "coding-plan" },
    providerRevision: 2,
    credentialGeneration: 3,
    runtimeFingerprint: "runtime-2",
    profileId: "claude.kimi_coding_api_key",
    source: state === "stale" ? "stale_cache" : "live",
    quota: { state, windows: [] },
  };
}

describe("CodingPlanQuotaFooter formatting", () => {
  it("formats amount, utilization, and reset without dropping zero usage", () => {
    expect(
      formatCodingPlanQuotaWindow(
        {
          kind: "five_hour",
          utilization: 25,
          used: 0,
          limit: 100,
          unit: "requests",
          resetsAtMs: Date.UTC(2026, 7, 14, 15, 30),
        },
        t,
        Date.UTC(2026, 7, 14, 13, 0),
      ),
    ).toBe("5h 0/100 requests 25% resets in 2h30m");
  });

  it("keeps stale quota windows visible and surfaces unavailable reasons", () => {
    const stale = snapshot("stale");
    stale.quota.plan = "Coding Pro";
    stale.quota.windows = [{ kind: "weekly", utilization: 68 }];
    expect(formatCodingPlanQuotaSummary(stale, t, 0)).toBe(
      "Coding Pro · Weekly 68%",
    );

    const unavailable = snapshot("unavailable");
    unavailable.quota.reason = "Quota endpoint is not published";
    expect(formatCodingPlanQuotaSummary(unavailable, t, 0)).toBe(
      "Quota endpoint is not published",
    );
  });

  it("labels a model-scoped quota window without hiding its base window kind", () => {
    expect(
      formatCodingPlanQuotaWindow(
        { kind: "weekly", scope: "kimi_k3", utilization: 65 },
        t,
        0,
      ),
    ).toBe("Weekly (kimi k3) 65%");
  });
});
