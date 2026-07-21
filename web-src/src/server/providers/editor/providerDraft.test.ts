import { describe, expect, it } from "vitest";

import { providerRegistry } from "@/server/providerRegistry";
import {
  createDraftForProfile,
  readEndpoint,
  readUpstreamModel,
  setEndpoint,
  setSingleModel,
} from "./providerDraft";

function serializedSecrets(value: unknown): string[] {
  const found: string[] = [];
  const visit = (item: unknown, path: string) => {
    if (!item || typeof item !== "object") return;
    for (const [key, child] of Object.entries(
      item as Record<string, unknown>,
    )) {
      const next = `${path}/${key}`;
      if (
        /(api[_-]?key|auth[_-]?token|access[_-]?key|secret|password)/i.test(
          key,
        ) &&
        typeof child === "string" &&
        child.trim()
      ) {
        found.push(next);
      }
      visit(child, next);
    }
  };
  visit(value, "");
  return found;
}

describe("Server Provider profile drafts", () => {
  const creatable = providerRegistry.profiles.filter(
    (profile) => profile.creationPolicy === "create_allowed",
  );

  it("covers every creatable profile with a deterministic typed draft", () => {
    expect(creatable).toHaveLength(34);
    for (const profile of creatable) {
      const first = createDraftForProfile(profile);
      const second = createDraftForProfile(profile);
      expect(second).toEqual(first);
      expect(first.name.trim(), profile.profileId).not.toBe("");
      expect(first.settingsConfig, profile.profileId).toBeTypeOf("object");
      expect(first.meta.providerType, profile.profileId).toBe(
        profile.compatibilityProviderType,
      );
      const mapping = first.settingsConfig.modelMapping as
        Record<string, unknown> | undefined;
      expect(mapping?.mode, profile.profileId).toBe(profile.modelPolicy);
      if (profile.modelPolicy === "single") {
        expect(
          readUpstreamModel(first.settingsConfig),
          profile.profileId,
        ).toBeTruthy();
      }
      expect(
        serializedSecrets(first.settingsConfig),
        profile.profileId,
      ).toEqual([]);
    }
  });

  it("updates only canonical endpoint and model fields", () => {
    const settings: Record<string, unknown> = {
      env: {},
      other: { keep: true },
    };
    setEndpoint(settings, "codex", "https://gateway.example/v1/");
    setSingleModel(settings, "codex", "model-x");

    expect(readEndpoint(settings, "codex")).toBe("https://gateway.example/v1");
    expect(readUpstreamModel(settings)).toBe("model-x");
    expect(settings.other).toEqual({ keep: true });
    expect(settings.modelMapping).toEqual({
      mode: "single",
      upstreamModel: "model-x",
    });
  });

  it("materializes non-secret AWS defaults without credential placeholders", () => {
    const profile = providerRegistry.profiles.find(
      (item) => item.profileId === "claude.aws_bedrock_aksk",
    );
    expect(profile).toBeDefined();

    const draft = createDraftForProfile(profile!);
    const env = draft.settingsConfig.env as Record<string, unknown>;
    expect(env.AWS_REGION).toBe("us-east-1");
    expect(env.ANTHROPIC_BASE_URL).toBe(
      "https://bedrock-runtime.us-east-1.amazonaws.com",
    );
    expect(env).not.toHaveProperty("AWS_ACCESS_KEY_ID");
    expect(env).not.toHaveProperty("AWS_SECRET_ACCESS_KEY");
  });
});
