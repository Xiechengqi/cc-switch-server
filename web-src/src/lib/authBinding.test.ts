import { describe, expect, it } from "vitest";

import {
  managedAuthAccountLabel,
  resolveManagedAccountId,
  resolveManagedAccountIdentity,
} from "./authBinding";
import type { ProviderMeta } from "@/types";

describe("managed account display labels", () => {
  it("uses the Server label and supports older responses without displayName", () => {
    const account = { id: "workbuddy-local-id", login: "workbuddy-local-id", email: null };
    for (const displayName of ["昵称", "138****0000"]) {
      expect(managedAuthAccountLabel({ ...account, displayName })).toBe(displayName);
    }
    expect(managedAuthAccountLabel({ ...account, email: " owner@example.com " })).toBe("owner@example.com");
    expect(managedAuthAccountLabel({ ...account, displayName: " " })).toBe(account.id);
    expect(managedAuthAccountLabel({ ...account, login: " " })).toBe(account.id);
  });
});

function metaWithBinding(
  authProvider: string,
  accountId: string,
  authIdentityGeneration?: number,
): ProviderMeta {
  return {
    authBinding: {
      source: "managed_account",
      authProvider,
      accountId,
      authIdentityGeneration,
    },
  };
}

describe("managed auth binding identity", () => {
  it("returns the immutable account id and identity generation", () => {
    const meta = metaWithBinding("codex_oauth", "account-1", 7);

    expect(resolveManagedAccountIdentity(meta, "codex_oauth")).toEqual({
      accountId: "account-1",
      authIdentityGeneration: 7,
    });
  });

  it("does not treat a generation-less legacy id as a quota identity", () => {
    const meta = metaWithBinding("claude_oauth", "account-1");

    expect(resolveManagedAccountId(meta, "claude_oauth")).toBe("account-1");
    expect(resolveManagedAccountIdentity(meta, "claude_oauth")).toBeNull();
  });

  it("rejects a binding owned by another auth provider", () => {
    const meta = metaWithBinding("claude_oauth", "account-1", 3);

    expect(resolveManagedAccountIdentity(meta, "codex_oauth")).toBeNull();
  });
});
