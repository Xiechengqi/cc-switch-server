import { describe, expect, it } from "vitest";

import type {
  OllamaCloudSnapshot,
  ProviderResource,
} from "@/lib/api/providers";
import { isOllamaCloudSnapshotForResource, ollamaCloudKeys } from "./ollama";

function resource(): ProviderResource {
  return {
    app: "claude",
    provider: { id: "ollama-bundle", name: "Ollama", settingsConfig: {} },
    providerType: "ollama_cloud",
    providerTypeId: "ollama_cloud",
    revision: 7,
    profileId: "claude.ollama_cloud",
    identity: { status: "bound" },
    credentialConfigured: true,
    credentialSlots: ["/settingsConfig/apiKey"],
  };
}

function snapshot(): OllamaCloudSnapshot {
  return {
    providerKey: { app: "claude", providerId: "ollama-bundle" },
    providerRevision: 7,
    credentialSourceKey: { app: "codex", providerId: "ollama-bundle" },
    credentialGeneration: 4,
    source: "fresh_cache",
    status: "complete",
    account: {
      state: "available",
      observedAtMs: 10,
      data: { id: "account-1", plan: "free" },
    },
    usage: {
      state: "available",
      observedAtMs: 10,
      data: { limits: [] },
    },
  };
}

describe("Ollama account usage query scope", () => {
  it("keys snapshots by Provider surface and revision", () => {
    expect(ollamaCloudKeys.snapshot(resource())).toEqual([
      "ollamaCloud",
      "accountUsage",
      "claude",
      "ollama-bundle",
      7,
    ]);
  });

  it("accepts a shared Codex credential source but rejects another surface revision", () => {
    expect(isOllamaCloudSnapshotForResource(snapshot(), resource())).toBe(true);
    expect(
      isOllamaCloudSnapshotForResource(
        { ...snapshot(), providerRevision: 8 },
        resource(),
      ),
    ).toBe(false);
  });
});
