import { describe, expect, it } from "vitest";

import type { ProviderRuntimePlan } from "@/lib/api/providers";
import { runtimeSummaryRows } from "./runtimeSummary";

function plan(
  overrides: Partial<ProviderRuntimePlan> = {},
): ProviderRuntimePlan {
  return {
    providerKey: { app: "codex", providerId: "provider" },
    providerRevision: 1,
    profileId: "codex.custom_http",
    profileSchemaRevision: 1,
    driverId: "http.openai_chat",
    driverContractRevision: 1,
    endpoint: "https://example.com/v1",
    upstreamProtocol: "open_ai_chat",
    outboundIdentityPolicy: { kind: "server_identity" },
    authRef: { kind: "static_credential" },
    modelPolicy: { mode: "single", upstreamModel: "gpt-test" },
    probePolicyFingerprint: "fingerprint",
    transportPolicy: {
      timeoutMs: 45_000,
      streamFirstByteTimeoutMs: 15_000,
      streamIdleTimeoutMs: 30_000,
      redirectPolicy: "same_origin",
      directConnection: true,
    },
    driverOptions: {},
    configurationState: "ready",
    runtimeFingerprint: "fixture",
    ...overrides,
  };
}

function valueOf(rows: ReturnType<typeof runtimeSummaryRows>, id: string) {
  return rows.find((row) => row.id === id)?.value;
}

describe("runtimeSummaryRows", () => {
  it("formats the resolved endpoint, model and timeouts", () => {
    const rows = runtimeSummaryRows(plan());
    expect(valueOf(rows, "endpoint")).toBe("https://example.com/v1");
    expect(valueOf(rows, "model")).toBe("gpt-test");
    expect(valueOf(rows, "timeout")).toBe("45s / 15s / 30s");
    expect(valueOf(rows, "state")).toBe("ready");
  });

  it("leaves passthrough and absent values null so the UI can say so once", () => {
    const rows = runtimeSummaryRows(
      plan({
        modelPolicy: { mode: "passthrough" },
        extraHeaders: [],
        transportPolicy: {
          timeoutMs: 30_000,
          redirectPolicy: "same_origin",
          directConnection: true,
        },
      }),
    );
    expect(valueOf(rows, "model")).toBeNull();
    expect(valueOf(rows, "headers")).toBeNull();
    expect(valueOf(rows, "region")).toBeNull();
    expect(valueOf(rows, "timeout")).toBe("30s");
  });

  it("lists custom header names", () => {
    const rows = runtimeSummaryRows(
      plan({
        extraHeaders: [
          { name: "x-a", credentialSlot: "/a" },
          { name: "x-b", credentialSlot: "/b" },
        ],
      }),
    );
    expect(valueOf(rows, "headers")).toBe("x-a, x-b");
  });
});
