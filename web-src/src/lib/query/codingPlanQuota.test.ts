import { describe, expect, it } from "vitest";

import type {
  CodingPlanQuotaSnapshot,
  ProviderResource,
} from "@/lib/api/providers";
import {
  codingPlanQuotaKeys,
  isCodingPlanQuotaSnapshotForResource,
} from "./codingPlanQuota";

function resource(): ProviderResource {
  return {
    app: "codex",
    provider: { id: "coding-plan-1", name: "Coding Plan", settingsConfig: {} },
    providerType: "codex",
    providerTypeId: "codex",
    revision: 7,
    profileId: "codex.kimi_coding_api_key",
    identity: { status: "bound" },
    credentialConfigured: true,
    credentialSlots: ["/settingsConfig/apiKey"],
    runtime: {
      providerKey: { app: "codex", providerId: "coding-plan-1" },
      providerRevision: 7,
      profileId: "codex.kimi_coding_api_key",
      profileSchemaRevision: 1,
      driverId: "http.openai_chat",
      driverContractRevision: 1,
      endpoint: "https://api.kimi.com",
      upstreamProtocol: "open_ai_chat",
      outboundIdentityPolicy: { kind: "server_identity" },
      authRef: { kind: "static_credential" },
      modelPolicy: { mode: "passthrough" },
      codingPlan: {
        contractRevision: 1,
        fixedOrigin: "https://api.kimi.com",
        protocol: "open_ai_chat",
        inferenceCredentialSlot: "/settingsConfig/apiKey",
        inferenceAuthScheme: "bearer",
        routes: { codex_responses: "/coding/v1/chat/completions" },
        models: [
          {
            id: "kimi-for-coding",
            displayName: "Kimi For Coding",
            contextWindow: 262_144,
            inputModalities: ["text"],
          },
        ],
        quota: {
          adapter: "kimi",
          endpoint: "https://api.kimi.com/coding/v1/usages",
          credentialSlots: [
            {
              role: "inference_credential",
              slot: "/settingsConfig/apiKey",
            },
          ],
          cacheTtlMs: 60_000,
          staleTtlMs: 900_000,
        },
        cacheTokens: "input_includes_cached",
        stream: {
          format: "open_ai_chat_sse",
          terminalEvent: "[DONE]",
          errorBeforeTerminalIsFatal: true,
        },
        error: {
          envelope: "open_ai",
          retrySameCredentialOnceOn401: false,
          retryAfterCommit: false,
        },
        pricing: {
          evidence: "flat_rate_subscription_no_usd",
          source: "fixture",
          capturedAt: "2026-08-13T00:00:00Z",
        },
      },
      transportPolicy: {
        timeoutMs: 300_000,
        redirectPolicy: "same_origin",
        directConnection: true,
      },
      driverOptions: {},
      configurationState: "ready",
      runtimeFingerprint: "runtime-fingerprint-7",
    },
  };
}

function snapshot(): CodingPlanQuotaSnapshot {
  return {
    providerKey: { app: "codex", providerId: "coding-plan-1" },
    providerRevision: 7,
    credentialGeneration: 4,
    runtimeFingerprint: "runtime-fingerprint-7",
    profileId: "codex.kimi_coding_api_key",
    source: "live",
    quota: { state: "supported", windows: [] },
  };
}

describe("coding-plan quota query scope", () => {
  it("keys cache entries by App, Provider, revision, and runtime fingerprint", () => {
    expect(codingPlanQuotaKeys.snapshot(resource())).toEqual([
      "codingPlanQuota",
      "snapshot",
      "codex",
      "coding-plan-1",
      7,
      "runtime-fingerprint-7",
    ]);
  });

  it("rejects snapshots from another revision or runtime", () => {
    const target = resource();
    expect(isCodingPlanQuotaSnapshotForResource(snapshot(), target)).toBe(true);

    expect(
      isCodingPlanQuotaSnapshotForResource(
        { ...snapshot(), providerRevision: 8 },
        target,
      ),
    ).toBe(false);
    expect(
      isCodingPlanQuotaSnapshotForResource(
        { ...snapshot(), runtimeFingerprint: "another-runtime" },
        target,
      ),
    ).toBe(false);
    expect(
      isCodingPlanQuotaSnapshotForResource(
        {
          ...snapshot(),
          providerKey: { app: "claude", providerId: "coding-plan-1" },
        },
        target,
      ),
    ).toBe(false);
  });
});
