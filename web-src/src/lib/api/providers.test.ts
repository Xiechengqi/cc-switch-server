import { beforeEach, describe, expect, it, vi } from "vitest";

const runtimeMocks = vi.hoisted(() => ({
  invokeCommand: vi.fn(),
}));

vi.mock("@/lib/runtime", () => ({
  invokeCommand: runtimeMocks.invokeCommand,
}));

import { providersApi, type CodingPlanQuotaSnapshot } from "./providers";

const snapshot: CodingPlanQuotaSnapshot = {
  providerKey: { app: "codex", providerId: "coding-plan" },
  providerRevision: 1,
  credentialGeneration: 2,
  runtimeFingerprint: "runtime-1",
  profileId: "codex.xiaomi_mimo_token_plan",
  source: "contract",
  quota: {
    state: "unavailable",
    windows: [],
    reason: "Quota endpoint is not published",
  },
};

beforeEach(() => {
  runtimeMocks.invokeCommand.mockReset();
  runtimeMocks.invokeCommand.mockResolvedValue(snapshot);
});

describe("providersApi coding-plan quota", () => {
  it("reads quota through the client-tunnel-compatible invoke command", async () => {
    await expect(
      providersApi.getCodingPlanQuota("codex", "coding-plan"),
    ).resolves.toBe(snapshot);
    expect(runtimeMocks.invokeCommand).toHaveBeenCalledWith(
      "get_coding_plan_quota",
      { app: "codex", providerId: "coding-plan" },
    );
  });

  it("uses the dedicated no-store force-refresh command", async () => {
    await expect(
      providersApi.refreshCodingPlanQuota("codex", "coding-plan"),
    ).resolves.toBe(snapshot);
    expect(runtimeMocks.invokeCommand).toHaveBeenCalledWith(
      "refresh_coding_plan_quota",
      { app: "codex", providerId: "coding-plan" },
      { cache: "no-store" },
    );
  });
});
