import { describe, expect, it } from "vitest";

import { formatReferralDate } from "./CodexReferralPanel";

describe("formatReferralDate", () => {
  it("returns null for missing or invalid upstream timestamps", () => {
    expect(formatReferralDate(null, "en")).toBeNull();
    expect(formatReferralDate("not-a-date", "en")).toBeNull();
  });

  it("returns null when the locale cannot be formatted", () => {
    expect(
      formatReferralDate("2026-08-09T12:00:00Z", "invalid_locale"),
    ).toBeNull();
  });

  it("formats a valid upstream timestamp", () => {
    expect(formatReferralDate("2026-08-09T12:00:00Z", "en")).toBeTruthy();
  });
});
