import { describe, expect, it } from "vitest";

import {
  resolveManagedAccountId,
  resolveManagedAccountIdentity,
} from "./authBinding";
import type { ProviderMeta } from "@/types";

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
