import { describe, expect, it } from "vitest";

import type { ProviderBundleView, ProviderResource } from "@/lib/api/providers";
import { familyById, providerRegistry } from "@/server/providerRegistry";
import { readEndpoint } from "@/server/providers/editor/providerDraft";
import {
  createProviderBundleDraft,
  duplicateProviderBundleDraft,
  editProviderBundleDraft,
  familyCredentialSlots,
  modelPoliciesForFamily,
  parseSettings,
  providerBundleIdentityEditable,
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

  it("keeps preset identity canonical while Custom HTTP remains editable", () => {
    const presetFamily = familyById("family.nvidia")!;
    const preset = createProviderBundleDraft(presetFamily);
    preset.name = "Renamed NVIDIA";
    preset.websiteUrl = "https://example.invalid";

    expect(providerBundleIdentityEditable(presetFamily)).toBe(false);
    expect(toProviderBundleWriteDraft(preset)).toMatchObject({
      name: "NVIDIA",
      websiteUrl: "https://build.nvidia.com",
    });

    const customFamily = familyById("family.custom_http")!;
    const custom = createProviderBundleDraft(customFamily);
    custom.name = "Private gateway";
    custom.websiteUrl = "https://gateway.example";

    expect(providerBundleIdentityEditable(customFamily)).toBe(true);
    expect(toProviderBundleWriteDraft(custom)).toMatchObject({
      name: "Private gateway",
      websiteUrl: "https://gateway.example",
    });
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

  it("duplicates Bundle configuration without reusing stored secrets", () => {
    const family = familyById("family.openrouter")!;
    const source = createProviderBundleDraft(family);
    const [{ pointer }] = familyCredentialSlots(family);
    const write = toProviderBundleWriteDraft(source);
    const surfaces = Object.fromEntries(
      write.surfaces.map((surface) => [
        surface.app,
        {
          app: surface.app,
          provider: {
            id: source.id,
            name: source.name,
            settingsConfig: surface.settingsConfig,
            category: surface.category,
            meta: surface.meta,
          },
          providerType: surface.meta?.providerType ?? "openrouter",
          providerTypeId: surface.meta?.providerType ?? "openrouter",
          revision: 4,
          profileId: surface.profileId,
          customBinding: surface.customBinding,
          identity: { status: "bound" as const },
          credentialConfigured: true,
          credentialSlots: [pointer],
        } satisfies ProviderResource,
      ]),
    ) as ProviderBundleView["surfaces"];
    const view: ProviderBundleView = {
      id: source.id,
      familyId: family.familyId,
      revision: 4,
      name: source.name,
      websiteUrl: source.websiteUrl,
      notes: source.notes,
      icon: source.icon,
      iconColor: source.iconColor,
      supportedApps: family.surfaces.map((surface) => surface.app),
      enabledApps: family.surfaces.map((surface) => surface.app),
      credentialConfigured: true,
      credentialSlots: [pointer],
      surfaces,
    };

    view.name = "Legacy custom name";
    view.websiteUrl = "https://legacy.example";
    expect(editProviderBundleDraft(view)).toMatchObject({
      name: source.name,
      websiteUrl: source.websiteUrl,
    });

    const duplicate = duplicateProviderBundleDraft(view);

    expect(duplicate.id).not.toBe(source.id);
    expect(duplicate.name).toBe(source.name);
    expect(duplicate.expectedRevision).toBeUndefined();
    expect(duplicate.clientRequestId).toBeTruthy();
    expect(Object.values(duplicate.secrets)).toEqual([
      { configured: false, value: "", clear: false },
    ]);
    expect(toProviderBundleWriteDraft(duplicate).credentialPatches).toEqual({});
    expect(validateProviderBundleDraft(duplicate)).toBe(
      "Configure the required credential",
    );
  });
});
