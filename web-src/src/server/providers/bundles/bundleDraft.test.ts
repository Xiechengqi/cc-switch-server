import { describe, expect, it } from "vitest";

import { familyById, providerRegistry } from "@/server/providerRegistry";
import { readEndpoint } from "@/server/providers/editor/providerDraft";
import {
  createProviderBundleDraft,
  familyCredentialSlots,
  modelPoliciesForFamily,
  parseSettings,
  toProviderBundleWriteDraft,
  updateBundleModel,
  updateSurfaceEndpoint,
  validateProviderBundleDraft,
} from "./bundleDraft";

describe("Provider Bundle drafts", () => {
  it("materializes every registry family with its complete Surface set", () => {
    for (const family of providerRegistry.families) {
      const draft = createProviderBundleDraft(family);
      expect(
        draft.surfaces.map((surface) => ({
          app: surface.app,
          profileId: surface.profileId,
          enabled: surface.enabled,
        })),
        family.familyId,
      ).toEqual(
        family.surfaces.map((surface) => ({
          app: surface.app,
          profileId: surface.profileId,
          enabled: surface.defaultEnabled,
        })),
      );
      expect(draft.routeKey, family.familyId).toMatch(
        /^(?=.{3,64}$)(?=.*[a-z])[a-z0-9_-]+$/,
      );
      expect(modelPoliciesForFamily(family), family.familyId).toContain(
        draft.modelPolicy,
      );
    }
  });

  it("writes one shared model policy and upstream model to every Surface", () => {
    const family = familyById("family.grok_oauth")!;
    const draft = updateBundleModel(
      createProviderBundleDraft(family),
      "single",
      "grok-shared-model",
    );
    const write = toProviderBundleWriteDraft(draft);

    for (const surface of write.surfaces) {
      expect(surface.settingsConfig.modelMapping).toEqual({
        mode: "single",
        upstreamModel: "grok-shared-model",
      });
    }

    const passthrough = toProviderBundleWriteDraft(
      updateBundleModel(draft, "passthrough", "grok-shared-model"),
    );
    for (const surface of passthrough.surfaces) {
      expect(surface.settingsConfig.modelMapping).toEqual({
        mode: "passthrough",
      });
    }
  });

  it("locks official Claude and Codex families to passthrough", () => {
    for (const familyId of ["family.claude_oauth", "family.openai_oauth"]) {
      const family = familyById(familyId)!;
      const draft = createProviderBundleDraft(family);
      expect(modelPoliciesForFamily(family), familyId).toEqual(["passthrough"]);
      expect(draft.modelPolicy, familyId).toBe("passthrough");

      draft.accountId = "official-account";
      draft.modelPolicy = "single";
      draft.upstreamModel = "forced-model";
      expect(validateProviderBundleDraft(draft), familyId).toBe(
        "Provider model policy is invalid",
      );
    }
  });

  it("binds one Grok OAuth account to Claude, Codex, and Gemini", () => {
    const family = familyById("family.grok_oauth")!;
    const draft = createProviderBundleDraft(family);
    draft.accountId = "grok-account";
    draft.accountGeneration = 7;

    expect(validateProviderBundleDraft(draft)).toBeNull();
    const write = toProviderBundleWriteDraft(draft);
    expect(write.surfaces.map((surface) => surface.app)).toEqual([
      "claude",
      "codex",
      "gemini",
    ]);
    for (const surface of write.surfaces) {
      expect(surface.meta?.authBinding).toMatchObject({
        source: "managed_account",
        authProvider: "grok_oauth",
        accountId: "grok-account",
        authIdentityGeneration: 7,
      });
    }
  });

  it("writes shared credentials at Bundle scope for fixed endpoint families", () => {
    const family = familyById("family.openrouter")!;
    const draft = createProviderBundleDraft(family);
    const [{ pointer }] = familyCredentialSlots(family);
    draft.secrets[pointer] = {
      configured: false,
      value: "shared-openrouter-key",
      clear: false,
    };

    const write = toProviderBundleWriteDraft(draft);
    expect(write.credentialPatches).toEqual({
      [pointer]: { action: "replace", value: "shared-openrouter-key" },
    });
    for (const surface of write.surfaces) {
      expect(surface.credentialPatches).toBeUndefined();
      expect(readEndpoint(surface.settingsConfig, surface.app)).toMatch(
        /^https:\/\/openrouter\.ai\/api/,
      );
    }
  });

  it("keeps Custom HTTP endpoint and credentials isolated by Surface", () => {
    const family = familyById("family.custom_http")!;
    const draft = createProviderBundleDraft(family);
    draft.surfaces = draft.surfaces.map((surface, index) => ({
      ...updateSurfaceEndpoint(
        surface,
        `https://${surface.app}.example/v${index + 1}`,
      ),
      secret: {
        configured: false,
        value: `${surface.app}-secret`,
        clear: false,
      },
    }));

    expect(validateProviderBundleDraft(draft)).toBeNull();
    const write = toProviderBundleWriteDraft(draft);
    expect(write.credentialPatches).toEqual({});
    for (const surface of write.surfaces) {
      expect(surface.credentialPatches).toEqual({
        "/settingsConfig/apiKey": {
          action: "replace",
          value: `${surface.app}-secret`,
        },
      });
      expect(
        readEndpoint(
          parseSettings(JSON.stringify(surface.settingsConfig)),
          surface.app,
        ),
      ).toContain(`${surface.app}.example`);
    }
  });

  it("does not require Surface-scoped credentials for disabled APIs", () => {
    const family = familyById("family.custom_http")!;
    const draft = createProviderBundleDraft(family);
    draft.surfaces = draft.surfaces.map((surface, index) => {
      const next =
        index === 0
          ? updateSurfaceEndpoint(surface, "https://claude.example/v1")
          : surface;
      return {
        ...next,
        enabled: index === 0,
        secret:
          index === 0
            ? { configured: false, value: "claude-secret", clear: false }
            : surface.secret,
      };
    });

    expect(validateProviderBundleDraft(draft)).toBeNull();
  });
});
