import { describe, expect, it, vi } from "vitest";

import {
  logoutAccountsAndClearSelection,
  removeAccountAndUpdateSelection,
} from "./accountSelectionActions";

describe("removeAccountAndUpdateSelection", () => {
  it("clears the selected account only after removal succeeds", async () => {
    const events: string[] = [];
    const onAccountSelect = vi.fn(() => events.push("selection-cleared"));

    await removeAccountAndUpdateSelection({
      accountId: "account-1",
      selectedAccountId: "account-1",
      removeAccount: async () => {
        events.push("removed");
      },
      onAccountSelect,
    });

    expect(events).toEqual(["removed", "selection-cleared"]);
    expect(onAccountSelect).toHaveBeenCalledWith(null);
  });

  it("preserves the selected account when removal fails", async () => {
    const onAccountSelect = vi.fn();

    await expect(
      removeAccountAndUpdateSelection({
        accountId: "account-1",
        selectedAccountId: "account-1",
        removeAccount: async () => {
          throw new Error("remove failed");
        },
        onAccountSelect,
      }),
    ).rejects.toThrow("remove failed");

    expect(onAccountSelect).not.toHaveBeenCalled();
  });
});

describe("logoutAccountsAndClearSelection", () => {
  it("clears the selection only after logout succeeds", async () => {
    const events: string[] = [];

    await logoutAccountsAndClearSelection({
      logout: async () => {
        events.push("logged-out");
      },
      onAccountSelect: () => events.push("selection-cleared"),
    });

    expect(events).toEqual(["logged-out", "selection-cleared"]);
  });

  it("preserves the selection when logout fails", async () => {
    const onAccountSelect = vi.fn();

    await expect(
      logoutAccountsAndClearSelection({
        logout: async () => {
          throw new Error("logout failed");
        },
        onAccountSelect,
      }),
    ).rejects.toThrow("logout failed");

    expect(onAccountSelect).not.toHaveBeenCalled();
  });
});
