import { describe, expect, it } from "vitest";

import {
  resolveManagedAccountBindingState,
  resolveManagedAccountSelection,
} from "./ServerProviderForm";

const baseInput = {
  managed: true,
  queryStatus: "success" as const,
  isEditMode: true,
  accountId: "account-1",
  storedAccountId: "account-1",
  storedGeneration: 1,
  selectedGeneration: 1,
};

describe("resolveManagedAccountBindingState", () => {
  it("fails closed while the authoritative account index is unavailable", () => {
    expect(
      resolveManagedAccountBindingState({
        ...baseInput,
        queryStatus: "pending",
      }),
    ).toBe("loading");
    expect(
      resolveManagedAccountBindingState({
        ...baseInput,
        queryStatus: "error",
      }),
    ).toBe("load_error");
  });

  it("distinguishes removed accounts from changed account identities", () => {
    expect(
      resolveManagedAccountBindingState({
        ...baseInput,
        selectedGeneration: undefined,
      }),
    ).toBe("missing");
    expect(
      resolveManagedAccountBindingState({
        ...baseInput,
        selectedGeneration: 2,
      }),
    ).toBe("stale");
  });

  it("unblocks the form after an explicit generation rebind", () => {
    expect(
      resolveManagedAccountBindingState({
        ...baseInput,
        storedGeneration: 2,
        selectedGeneration: 2,
      }),
    ).toBe("current");
  });
});

describe("resolveManagedAccountSelection", () => {
  it("writes the matching identity generation when the account changes", () => {
    expect(
      resolveManagedAccountSelection({
        currentAccountId: "account-1",
        currentBinding: {
          source: "managed_account",
          authProvider: "codex_oauth",
          accountId: "account-1",
          authIdentityGeneration: 1,
        },
        nextAccountId: "account-2",
        authProvider: "codex_oauth",
        accounts: [
          {
            id: "account-2",
            authIdentityGeneration: 7,
          },
        ],
      }),
    ).toEqual({
      accountId: "account-2",
      authBinding: {
        source: "managed_account",
        authProvider: "codex_oauth",
        accountId: "account-2",
        authIdentityGeneration: 7,
      },
    });
  });

  it("does not silently rebind a stale identity on a repeated selection", () => {
    expect(
      resolveManagedAccountSelection({
        currentAccountId: "account-1",
        currentBinding: {
          source: "managed_account",
          authProvider: "codex_oauth",
          accountId: "account-1",
          authIdentityGeneration: 1,
        },
        nextAccountId: "account-1",
        authProvider: "codex_oauth",
        accounts: [
          {
            id: "account-1",
            authIdentityGeneration: 2,
          },
        ],
      }).authBinding.authIdentityGeneration,
    ).toBe(1);
  });

  it("leaves generation unset when the selected account is unavailable", () => {
    expect(
      resolveManagedAccountSelection({
        currentAccountId: "account-1",
        nextAccountId: "missing-account",
        authProvider: "codex_oauth",
        accounts: [],
      }).authBinding,
    ).toEqual({
      source: "managed_account",
      authProvider: "codex_oauth",
      accountId: "missing-account",
    });
  });
});
