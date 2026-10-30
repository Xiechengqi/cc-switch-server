import { beforeEach, describe, expect, it, vi } from "vitest";

const runtimeMocks = vi.hoisted(() => ({
  invokeCommand: vi.fn(),
}));

vi.mock("@/lib/runtime", () => ({
  invokeCommand: runtimeMocks.invokeCommand,
}));

import {
  providersApi,
  type CodingPlanQuotaSnapshot,
  type OllamaCloudSnapshot,
} from "./providers";

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

const ollamaSnapshot: OllamaCloudSnapshot = {
  providerKey: { app: "codex", providerId: "ollama" },
  providerRevision: 3,
  credentialSourceKey: { app: "codex", providerId: "ollama" },
  credentialGeneration: 2,
  source: "live",
  status: "complete",
  account: {
    state: "available",
    observedAtMs: 1,
    data: { id: "account-1", plan: "free" },
  },
  usage: {
    state: "available",
    observedAtMs: 1,
    data: { limits: [] },
  },
};

beforeEach(() => {
  runtimeMocks.invokeCommand.mockReset();
  runtimeMocks.invokeCommand.mockResolvedValue(snapshot);
});

describe("providersApi coding-plan quota", () => {
  it("validates an unsaved Bundle through the authenticated no-store tunnel command", async () => {
    const bundle = { id: "custom" } as Parameters<typeof providersApi.upsertBundle>[0];
    await providersApi.validateBundleDraft(bundle, true);
    expect(runtimeMocks.invokeCommand).toHaveBeenLastCalledWith(
      "validate_provider_bundle_draft", { bundle, shareEnabled: true }, { cache: "no-store" },
    );
  });
  it("uses preview and apply commands that work through client tunnels", async () => {
    const bundle = { id: "custom", expectedRevision: 1 } as Parameters<typeof providersApi.upsertBundle>[0];
    runtimeMocks.invokeCommand.mockResolvedValueOnce({ preview: { previewToken: "preview" } });
    await expect(providersApi.previewBundleRebind(bundle)).resolves.toEqual({ previewToken: "preview" });
    expect(runtimeMocks.invokeCommand).toHaveBeenLastCalledWith(
      "rebind_custom_provider_bundle", { mode: "preview", bundle }, { cache: "no-store" },
    );
    runtimeMocks.invokeCommand.mockResolvedValueOnce({ bundle });
    await expect(providersApi.applyBundleRebind(bundle, "preview")).resolves.toBe(bundle);
    expect(runtimeMocks.invokeCommand).toHaveBeenLastCalledWith(
      "rebind_custom_provider_bundle", { mode: "apply", bundle, previewToken: "preview" }, { cache: "no-store" },
    );
  });
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

describe("providersApi Ollama account usage", () => {
  it("reads the Provider-owned snapshot through the invoke contract", async () => {
    runtimeMocks.invokeCommand.mockResolvedValueOnce(ollamaSnapshot);
    await expect(
      providersApi.getProviderAccountUsage("codex", "ollama"),
    ).resolves.toBe(ollamaSnapshot);
    expect(runtimeMocks.invokeCommand).toHaveBeenCalledWith(
      "get_provider_account_usage",
      { app: "codex", providerId: "ollama" },
    );
  });

  it("uses a no-store command for force refresh", async () => {
    runtimeMocks.invokeCommand.mockResolvedValueOnce(ollamaSnapshot);
    await expect(
      providersApi.refreshProviderAccountUsage("codex", "ollama"),
    ).resolves.toBe(ollamaSnapshot);
    expect(runtimeMocks.invokeCommand).toHaveBeenCalledWith(
      "refresh_provider_account_usage",
      { app: "codex", providerId: "ollama" },
      { cache: "no-store" },
    );
  });
});
