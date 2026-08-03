import { describe, expect, it, vi } from "vitest";

import { selectCodexActiveAccountAndNotify } from "./CodexOAuthSection";

describe("selectCodexActiveAccountAndNotify", () => {
  it("updates the Provider draft only after the active-account transaction", async () => {
    const events: string[] = [];

    const result = await selectCodexActiveAccountAndNotify(
      "account-b",
      async () => {
        events.push("active-rebound");
      },
      () => events.push("draft-updated"),
    );

    expect(result).toBe(true);
    expect(events).toEqual(["active-rebound", "draft-updated"]);
  });

  it("preserves the Provider draft when the active-account transaction fails", async () => {
    const onAccountSelect = vi.fn();

    const result = await selectCodexActiveAccountAndNotify(
      "account-b",
      async () => {
        throw new Error("share conflict");
      },
      onAccountSelect,
    );

    expect(result).toBe(false);
    expect(onAccountSelect).not.toHaveBeenCalled();
  });
});
