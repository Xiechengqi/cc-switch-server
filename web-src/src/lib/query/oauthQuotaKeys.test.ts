import { describe, expect, it } from "vitest";

import {
  isStaleOauthQuotaAccountKey,
  oauthQuotaAccountKey,
  oauthQuotaInvalidationKeys,
  oauthQuotaProviderKey,
} from "./oauthQuotaKeys";

describe("OAuth quota query keys", () => {
  it("keeps Agy and Antigravity account caches independent", () => {
    expect(oauthQuotaAccountKey("agy_oauth", "agy-account")).toEqual([
      "agy_oauth",
      "quota",
      "agy-account",
    ]);
    expect(
      oauthQuotaAccountKey("antigravity_oauth", "antigravity-account"),
    ).toEqual(["antigravity_oauth", "quota", "antigravity-account"]);
  });

  it("maps GitHub Copilot account caches to the footer query root", () => {
    expect(
      oauthQuotaAccountKey("github_copilot", "copilot-account", 7),
    ).toEqual(["copilot", "quota", "copilot-account", 7]);
  });

  it("isolates reauthorized identities that reuse one account id", () => {
    expect(oauthQuotaAccountKey("codex_oauth", "account-1", 4)).not.toEqual(
      oauthQuotaAccountKey("codex_oauth", "account-1", 5),
    );
    expect(
      isStaleOauthQuotaAccountKey(
        oauthQuotaAccountKey("codex_oauth", "account-1", 4),
        "codex_oauth",
        "account-1",
        5,
      ),
    ).toBe(true);
    expect(
      isStaleOauthQuotaAccountKey(
        oauthQuotaAccountKey("codex_oauth", "account-1", 5),
        "codex_oauth",
        "account-1",
        5,
      ),
    ).toBe(false);
  });

  it("maps Ollama provider caches and account refresh events to one root", () => {
    expect(
      oauthQuotaProviderKey("ollama_cloud", "provider-1", "gemini"),
    ).toEqual(["ollama", "quota", "provider-1", "gemini"]);
    expect(
      oauthQuotaInvalidationKeys({
        authProvider: "ollama_cloud",
        accountId: "ollama-account-1",
      }),
    ).toEqual([["ollama", "quota"]]);
    expect(
      oauthQuotaInvalidationKeys({
        authProvider: "ollama",
        accountId: "legacy-account-1",
      }),
    ).toEqual([["ollama", "quota"]]);
  });

  it("preserves account, default, and provider-scoped Cursor invalidation", () => {
    expect(
      oauthQuotaInvalidationKeys({
        authProvider: "cursor_apikey",
        accountId: "cursor-account",
        authIdentityGeneration: 3,
        providerId: "provider-1",
        appType: "codex",
      }),
    ).toEqual([
      ["cursor_apikey", "quota", "cursor-account", 3],
      ["cursor_apikey", "quota", "provider-1", "codex"],
      ["cursor_apikey", "quota", "default"],
    ]);
  });
});
