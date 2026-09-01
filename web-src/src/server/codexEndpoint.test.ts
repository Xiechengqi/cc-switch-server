import { describe, expect, it } from "vitest";

import { extractCodexBaseUrl, getCodexBaseUrl } from "./codexEndpoint";

describe("Server Codex endpoint extraction", () => {
  it("prefers the active model provider", () => {
    const config = `model_provider = "primary"

[model_providers.primary]
base_url = "https://primary.example/v1"

[model_providers.backup]
base_url = "https://backup.example/v1"
`;
    expect(extractCodexBaseUrl(config)).toBe("https://primary.example/v1");
  });

  it("supports top-level and unambiguous provider endpoints", () => {
    expect(extractCodexBaseUrl('base_url = "https://top.example/v1"')).toBe(
      "https://top.example/v1",
    );
    expect(
      extractCodexBaseUrl(
        '[model_providers.custom]\nbase_url = "https://only.example/v1"',
      ),
    ).toBe("https://only.example/v1");
  });

  it("fails closed for invalid or ambiguous config", () => {
    expect(extractCodexBaseUrl("not = [valid")).toBeUndefined();
    expect(
      extractCodexBaseUrl(
        '[model_providers.a]\nbase_url = "https://a.example"\n' +
          '[model_providers.b]\nbase_url = "https://b.example"',
      ),
    ).toBeUndefined();
  });

  it("reads the Provider settings config", () => {
    expect(
      getCodexBaseUrl({
        settingsConfig: {
          config:
            'model_provider = "custom"\n[model_providers.custom]\nbase_url = "https://custom.example/v1"',
        },
      }),
    ).toBe("https://custom.example/v1");
  });
});
