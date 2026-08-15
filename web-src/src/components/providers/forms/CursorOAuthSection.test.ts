import React from "react";
import { act } from "react-dom/test-utils";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

const oauthState = vi.hoisted(() => ({
  accounts: [
    { id: "account-a", login: "account-a", email: "a@example.com" },
    { id: "account-b", login: "account-b", email: "b@example.com" },
  ],
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: { defaultValue?: string }) =>
      options?.defaultValue ?? key,
  }),
}));

vi.mock("./hooks/useCursorOauth", () => ({
  useCursorOauth: () => ({
    authStatus: { accounts: oauthState.accounts },
    accounts: oauthState.accounts,
    hasAnyAccount: oauthState.accounts.length > 0,
    isLoadingStatus: false,
    isFetchingStatus: false,
    isStatusError: false,
    pollingState: "idle",
    deviceCode: null,
    error: null,
    isPolling: false,
    isImportingCursorLocalAuth: false,
    isRemovingAccount: false,
    addAccount: vi.fn(),
    cancelAuth: vi.fn(),
    importCursorLocalAuth: vi.fn(),
    removeAccountAsync: vi.fn(),
    refetchStatus: vi.fn(),
  }),
}));

import {
  CursorOAuthSection,
  resolveCursorAccountSelection,
} from "./CursorOAuthSection";

afterEach(() => {
  document.body.replaceChildren();
});

describe("resolveCursorAccountSelection", () => {
  const accounts = [{ id: "account-a" }, { id: "account-b" }];

  it("selects the only account when the Provider is not bound yet", () => {
    expect(resolveCursorAccountSelection([accounts[0]], null)).toBe(
      "account-a",
    );
  });

  it("leaves a multi-account Provider unbound until an account is selected", () => {
    expect(resolveCursorAccountSelection(accounts, null)).toBeUndefined();
  });

  it("preserves an existing account binding", () => {
    expect(
      resolveCursorAccountSelection(accounts, "account-b"),
    ).toBeUndefined();
  });

  it("clears a binding only after its account disappears", () => {
    expect(resolveCursorAccountSelection(accounts, "account-c")).toBeNull();
  });

  it("renders an explicit Provider account selector for multiple accounts", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    globalThis.IS_REACT_ACT_ENVIRONMENT = true;

    await act(async () => {
      root.render(
        React.createElement(CursorOAuthSection, {
          selectedAccountId: "account-b",
          onAccountSelect: vi.fn(),
        }),
      );
    });

    const selector = container.querySelector('[role="combobox"]');
    expect(selector).not.toBeNull();
    expect(selector?.textContent).toContain("b@example.com");

    await act(async () => root.unmount());
  });
});
