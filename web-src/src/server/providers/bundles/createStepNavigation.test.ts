import { describe, expect, it } from "vitest";

import {
  canVisitCreateStep,
  CREATE_STEPS,
  nextCreateStep,
  previousCreateStep,
  unlockCreateStep,
} from "./createStepNavigation";

describe("create step navigation", () => {
  it("keeps the three steps in a linear order", () => {
    expect(CREATE_STEPS).toEqual(["family", "supply", "share"]);
    expect(nextCreateStep("family")).toBe("supply");
    expect(nextCreateStep("supply")).toBe("share");
    expect(nextCreateStep("share")).toBeNull();
    expect(previousCreateStep("share")).toBe("supply");
    expect(previousCreateStep("supply")).toBe("family");
    expect(previousCreateStep("family")).toBeNull();
  });

  it("only permits already reached steps", () => {
    expect(canVisitCreateStep("family", "family")).toBe(true);
    expect(canVisitCreateStep("supply", "family")).toBe(false);
    expect(canVisitCreateStep("share", "supply")).toBe(false);
    expect(canVisitCreateStep("family", "share")).toBe(true);
    expect(canVisitCreateStep("supply", "share")).toBe(true);
  });

  it("never relocks a step while moving backwards", () => {
    expect(unlockCreateStep("family", "supply")).toBe("supply");
    expect(unlockCreateStep("supply", "family")).toBe("supply");
    expect(unlockCreateStep("share", "family")).toBe("share");
  });
});
