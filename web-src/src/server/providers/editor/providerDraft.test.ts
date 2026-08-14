import { describe, expect, it } from "vitest";

import {
  modelPoliciesForProfile,
  providerRegistry,
} from "@/server/providerRegistry";
import {
  createDraftForProfile,
  profileAllowsEndpointEditing,
  providerPresetForProfile,
  readEndpoint,
  readModelPolicy,
  readUpstreamModel,
  setEndpoint,
  setPassthroughModel,
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
    expect(creatable.length).toBeGreaterThan(0);
    expect(new Set(creatable.map((profile) => profile.profileId)).size).toBe(
      creatable.length,
    );
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
        ).toBe(profile.defaultUpstreamModel);
      }
      expect(
        serializedSecrets(first.settingsConfig),
        profile.profileId,
      ).toEqual([]);
    }
  });

  it("materializes the Claude Google OAuth draft for Code Assist", () => {
    const profile = providerRegistry.profiles.find(
      (item) => item.profileId === "claude.google_oauth",
    );
    expect(profile).toBeDefined();

    const draft = createDraftForProfile(profile!);
    expect(draft.name).toBe("Google Gemini OAuth");
    expect(readEndpoint(draft.settingsConfig, "claude")).toBe(
      "https://cloudcode-pa.googleapis.com",
    );
    expect(readUpstreamModel(draft.settingsConfig)).toBe(
      "gemini-3.1-pro-preview",
    );
    expect(draft.meta).toMatchObject({
      providerType: "gemini_cli",
      apiFormat: "gemini_native",
    });
    expect(serializedSecrets(draft.settingsConfig)).toEqual([]);
    expect(profileAllowsEndpointEditing(profile!)).toBe(false);
  });

  it("materializes the Server-native Codex GitHub Copilot surface", () => {
    const profile = providerRegistry.profiles.find(
      (item) => item.profileId === "codex.github_copilot",
    );
    expect(profile).toBeDefined();

    const draft = createDraftForProfile(profile!);
    expect(draft.name).toBe("GitHub Copilot");
    expect(readEndpoint(draft.settingsConfig, "codex")).toBe(
      "https://api.githubcopilot.com",
    );
    expect(readUpstreamModel(draft.settingsConfig)).toBe("gpt-5.5");
    expect(draft.meta).toMatchObject({
      providerType: "github_copilot",
    });
    expect(serializedSecrets(draft.settingsConfig)).toEqual([]);
    expect(profileAllowsEndpointEditing(profile!)).toBe(false);
  });

  it("materializes the Server-native Codex Kiro surface", () => {
    const profile = providerRegistry.profiles.find(
      (item) => item.profileId === "codex.kiro_oauth",
    );
    expect(profile).toBeDefined();

    const draft = createDraftForProfile(profile!);
    expect(draft.name).toBe("Kiro OAuth");
    expect(readEndpoint(draft.settingsConfig, "codex")).toBe(
      "https://q.us-east-1.amazonaws.com",
    );
    expect(readUpstreamModel(draft.settingsConfig)).toBe(
      "claude-sonnet-4-8",
    );
    expect(draft.meta.providerType).toBe("kiro_oauth");
    expect(serializedSecrets(draft.settingsConfig)).toEqual([]);
  });

  it("has an icon-selector preset for every non-custom creatable profile", () => {
    for (const profile of creatable.filter(
      (item) => item.formComposition !== "custom",
    )) {
      expect(
        providerPresetForProfile(profile),
        profile.profileId,
      ).toBeDefined();
    }
  });

  it("materializes all Qoder surfaces with the managed-account visual identity", () => {
    for (const profileId of [
      "claude.qoder_cosy",
      "codex.qoder_cosy",
      "gemini.qoder_cosy",
    ]) {
      const profile = providerRegistry.profiles.find(
        (item) => item.profileId === profileId,
      );
      expect(profile, profileId).toBeDefined();

      const draft = createDraftForProfile(profile!);
      expect(draft.name, profileId).toBe("Qoder COSY");
      expect(draft.icon, profileId).toBe("qoder");
      expect(draft.meta.providerType, profileId).toBe("qoder_cosy");
      expect(readUpstreamModel(draft.settingsConfig), profileId).toBe("auto");
      expect(serializedSecrets(draft.settingsConfig), profileId).toEqual([]);
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

  it("lets configurable profiles persist passthrough without losing their default", () => {
    const profile = providerRegistry.profiles.find(
      (item) => item.profileId === "codex.openrouter",
    );
    expect(profile).toBeDefined();
    expect(modelPoliciesForProfile(profile!)).toEqual([
      "single",
      "passthrough",
    ]);

    const draft = createDraftForProfile(profile!);
    const defaultModel = readUpstreamModel(draft.settingsConfig);
    setPassthroughModel(draft.settingsConfig);

    expect(readModelPolicy(draft.settingsConfig, profile!)).toBe("passthrough");
    expect(draft.settingsConfig.modelMapping).toEqual({ mode: "passthrough" });
    expect(readUpstreamModel(draft.settingsConfig)).toBe(defaultModel);
  });

  it("keeps official profiles locked to passthrough", () => {
    const profile = providerRegistry.profiles.find(
      (item) => item.profileId === "codex.openai_api_key",
    );
    expect(profile).toBeDefined();
    expect(modelPoliciesForProfile(profile!)).toEqual(["passthrough"]);

    expect(
      readModelPolicy(
        {
          modelMapping: { mode: "single", upstreamModel: "gpt-fixed" },
        },
        profile!,
      ),
    ).toBe("passthrough");
  });

  it("reads fixed Codex endpoints from the structured TOML provider section", () => {
    const profile = providerRegistry.profiles.find(
      (item) => item.profileId === "codex.openrouter",
    );
    expect(profile).toBeDefined();

    const draft = createDraftForProfile(profile!);
    expect(readEndpoint(draft.settingsConfig, "codex")).toBe(
      "https://openrouter.ai/api/v1",
    );
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

  it("reserves custom User-Agent overrides for Custom HTTP profiles", () => {
    for (const driver of providerRegistry.drivers) {
      expect(driver.outboundIdentityPolicy.kind).not.toBe("custom_override");
    }
    for (const policy of providerRegistry.customPolicies) {
      expect(policy.outboundIdentityPolicy).toEqual({
        kind: "custom_override",
      });
    }
  });
});
