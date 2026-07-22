import { describe, expect, it } from "vitest";

import type { ProviderResource } from "@/lib/api/providers";

import { providerResourceSupportsOperation } from "./providerOperations";

function resource(
  profileId?: string,
  customBinding?: ProviderResource["customBinding"],
): ProviderResource {
  return {
    app: "claude",
    provider: { id: "provider-1", name: "Provider", settingsConfig: {} },
    providerType: "claude",
    providerTypeId: "claude",
    revision: 1,
    profileId,
    customBinding,
    identity: { status: "bound" },
    credentialConfigured: true,
    credentialSlots: ["/settingsConfig/apiKey"],
  };
}

describe("providerResourceSupportsOperation", () => {
  it("enables both Anthropic API Key tests from its fixed driver", () => {
    const anthropic = resource("claude.anthropic_api_key");

    expect(providerResourceSupportsOperation(anthropic, "test")).toBe(true);
    expect(providerResourceSupportsOperation(anthropic, "connectivity")).toBe(
      true,
    );
  });

  it("keeps driver-specific unsupported operations disabled", () => {
    const kiro = resource("claude.kiro_oauth");

    expect(providerResourceSupportsOperation(kiro, "test")).toBe(false);
    expect(providerResourceSupportsOperation(kiro, "connectivity")).toBe(true);
  });

  it("resolves Custom HTTP operations from its protocol and auth binding", () => {
    const custom = resource("claude.custom_http", {
      upstreamProtocol: "anthropic_messages",
      authScheme: "api_key",
    });

    expect(providerResourceSupportsOperation(custom, "test")).toBe(true);
    expect(providerResourceSupportsOperation(custom, "discovery")).toBe(true);
  });

  it("allows legacy callers to use their compatibility fallback", () => {
    expect(providerResourceSupportsOperation(resource(), "test")).toBe(
      undefined,
    );
  });
});
