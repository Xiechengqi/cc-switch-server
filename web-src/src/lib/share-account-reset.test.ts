import { describe, expect, it } from "vitest";
import type { AccountQuotaCycle, AccountResetCapability } from "@/lib/api/share";
import { accountResetSelectable, effectiveAccountResetCapability } from "./share-account-reset";

const capability = (syncStatus: AccountResetCapability["syncStatus"], reason?: string | null) =>
  ({ support: "supported", syncStatus, reason, minimumContractVersion: 8 }) as AccountResetCapability;
const cycle = (endsAtMs: number | null) => ({ startsAtMs: 0, endsAtMs, observedAtMs: 0 }) as AccountQuotaCycle;

describe("account reset period choice", () => {
  it("follows the preview: ready, or stale only because the observation is old", () => {
    expect(accountResetSelectable({ syncStatus: "ready" })).toBe(true);
    // The preview names an old observation ACCOUNT_RESET_NOT_READY; Apply refreshes it.
    expect(accountResetSelectable({ syncStatus: "stale", reason: "ACCOUNT_RESET_NOT_READY" })).toBe(true);
    // The Share projection names the same case with no reason.
    expect(accountResetSelectable({ syncStatus: "stale", reason: null })).toBe(true);
    expect(accountResetSelectable({ syncStatus: "stale", reason: "cycle_expired" })).toBe(false);
    expect(accountResetSelectable({ syncStatus: "stale", reason: "QUOTA_HISTORY_INCOMPLETE" })).toBe(false);
    expect(accountResetSelectable({ syncStatus: "initializing", reason: "ACCOUNT_RESET_NOT_READY" })).toBe(false);
    expect(accountResetSelectable(undefined)).toBe(false);
  });

  it("shows an ended cycle as stale and keeps the reason that really blocks it", () => {
    expect(effectiveAccountResetCapability(capability("ready"), cycle(1_000), 1_000))
      .toEqual({ syncStatus: "stale", reason: "cycle_expired" });
    expect(effectiveAccountResetCapability(capability("initializing", "history_incomplete"), cycle(1_000), 2_000))
      .toEqual({ syncStatus: "stale", reason: "history_incomplete" });
    expect(effectiveAccountResetCapability(capability("stale", "ACCOUNT_RESET_NOT_READY"), cycle(1_000), 2_000))
      .toEqual({ syncStatus: "stale", reason: "cycle_expired" });
    expect(effectiveAccountResetCapability(capability("ready"), cycle(5_000), 1_000))
      .toEqual({ syncStatus: "ready", reason: undefined });
    expect(effectiveAccountResetCapability(capability("ready"), cycle(null), 1_000))
      .toEqual({ syncStatus: "ready", reason: undefined });
  });
});
