import { describe, expect, it } from "vitest";

import { claudeAccountDisplayLabel } from "./ClaudeOAuthSection";

describe("claudeAccountDisplayLabel", () => {
  it("includes the resolved Max multiplier in account selectors", () => {
    expect(
      claudeAccountDisplayLabel({
        login: "owner@example.com",
        subscriptionLevel: "Claude Max 5x",
      }),
    ).toBe("owner@example.com · Claude Max 5x");
    expect(
      claudeAccountDisplayLabel({
        login: "owner@example.com",
        subscriptionLevel: "Claude Max 20x",
      }),
    ).toBe("owner@example.com · Claude Max 20x");
  });

  it("keeps the existing account label when the plan is unavailable", () => {
    expect(
      claudeAccountDisplayLabel({
        login: "owner@example.com",
        subscriptionLevel: null,
      }),
    ).toBe("owner@example.com");
  });
});
