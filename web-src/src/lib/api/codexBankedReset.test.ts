import { beforeEach, describe, expect, it, vi } from "vitest";

const runtimeMocks = vi.hoisted(() => ({
  invokeCommand: vi.fn(),
}));

vi.mock("@/lib/runtime", () => ({
  invokeCommand: runtimeMocks.invokeCommand,
}));

import {
  consumeCodexBankedReset,
  getCodexBankedResetStatus,
} from "./codexBankedReset";

beforeEach(() => {
  runtimeMocks.invokeCommand.mockReset();
  runtimeMocks.invokeCommand.mockResolvedValue({});
});

describe("Codex Banked Reset API", () => {
  it("scopes status and consume commands to the persisted Provider revision", async () => {
    const target = {
      providerId: "openai-oauth",
      expectedRevision: 7,
      accountId: "account-a",
    };

    await getCodexBankedResetStatus(target, true);
    await consumeCodexBankedReset(target, " credit-a ");

    expect(runtimeMocks.invokeCommand).toHaveBeenNthCalledWith(
      1,
      "codex_banked_reset_status",
      { ...target, force: true },
    );
    expect(runtimeMocks.invokeCommand).toHaveBeenNthCalledWith(
      2,
      "codex_banked_reset_consume",
      { ...target, creditId: "credit-a" },
    );
  });
});
