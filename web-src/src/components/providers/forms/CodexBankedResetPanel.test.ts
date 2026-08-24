import { describe, expect, it } from "vitest";

import {
  resolveBankedResetConsumeCreditId,
  selectableBankedResetCredits,
} from "./CodexBankedResetPanel";

describe("CodexBankedResetPanel selection", () => {
  it("uses an exact fresh upstream credit id when details are available", () => {
    const credits = selectableBankedResetCredits({
      availableCount: 1,
      detailsAvailable: true,
      detailsStale: false,
      credits: [
        {
          id: "credit-a",
          idAvailable: true,
          status: "available",
        },
      ],
    });

    expect(resolveBankedResetConsumeCreditId(1, "credit-a", credits)).toBe(
      "credit-a",
    );
  });

  it("omits credit id so upstream can select when only the count is usable", () => {
    const credits = selectableBankedResetCredits({
      availableCount: 2,
      detailsAvailable: false,
      detailsStale: true,
      credits: [
        {
          id: "stale-credit",
          idAvailable: true,
          status: "available",
        },
      ],
    });

    expect(credits).toEqual([]);
    expect(resolveBankedResetConsumeCreditId(2, "", credits)).toBe("");
  });

  it("does not consume a synthetic id or consume when the count is zero", () => {
    const credits = selectableBankedResetCredits({
      availableCount: 1,
      detailsAvailable: true,
      detailsStale: false,
      credits: [
        {
          id: "credit-1",
          idAvailable: false,
          status: "available",
        },
      ],
    });

    expect(credits).toEqual([]);
    expect(resolveBankedResetConsumeCreditId(0, "", credits)).toBeNull();
  });
});
