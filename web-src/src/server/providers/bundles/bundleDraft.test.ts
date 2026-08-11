import { describe, expect, it } from "vitest";

import type {
  ProviderBundleView,
  ProviderResource,
  ProviderRuntimePlan,
} from "@/lib/api/providers";
import { familyById, providerRegistry } from "@/server/providerRegistry";
import {
  applyCustomRecipeToBundleDraft,
  changeModelPolicyScope,
  createProviderBundleDraft,
  customRecipeMatchesBundleDraft,
  duplicateProviderBundleDraft,
  editProviderBundleDraft,
  familyCredentialSlots,
  modelPoliciesForFamily,
  perAppModelPoliciesDiffer,
  providerBundleIdentityEditable,
  supportsPerAppModelPolicy,
  toProviderBundleWriteDraft,
  updateBundleModel,
  updateSurfaceEndpoint,
  validateProviderBundleDraft,
} from "./bundleDraft";

function runtime(
  resource: Pick<ProviderResource, "app" | "profileId" | "revision">,
  endpoint: string,
): ProviderRuntimePlan {
  return {
    providerKey: { app: resource.app, providerId: "provider" },
    providerRevision: resource.revision,
    profileId: resource.profileId!,
    profileSchemaRevision: 1,
    driverId: "http.openai_chat",
    driverContractRevision: 1,
    endpoint,
    upstreamProtocol: "open_ai_chat",
    outboundIdentityPolicy: { kind: "server_identity" },
    authRef: { kind: "static_credential" },
    modelPolicy: { mode: "single", upstreamModel: "shared-model" },
    testModel: "health-model",
    transportPolicy: {
      timeoutMs: 45_000,
      streamFirstByteTimeoutMs: 15_000,
      streamIdleTimeoutMs: 30_000,
      redirectPolicy: "same_origin",
      directConnection: true,
    },
    extraHeaders: [],
    driverOptions: {},
    configurationState: "ready",
    warnings: [],
    runtimeFingerprint: "fixture",
  };
}

describe("Provider Bundle drafts", () => {
  it("materializes every family and every Driver option schema", () => {
    for (const family of providerRegistry.families) {
      const draft = createProviderBundleDraft(family);
      expect(
        draft.surfaces.map(({ app, profileId, enabled }) => ({
          app,
          profileId,
          enabled,
        })),
        family.familyId,
      ).toEqual(
        family.surfaces.map(({ app, profileId, defaultEnabled }) => ({
          app,
          profileId,
          enabled: defaultEnabled,
        })),
      );
      expect(modelPoliciesForFamily(family), family.familyId).toContain(
        draft.modelPolicy,
      );
      expect(
        draft.surfaces.every((surface) => !("settingsText" in surface)),
      ).toBe(true);
      for (const surface of toProviderBundleWriteDraft(draft).surfaces) {
        expect("settingsConfig" in surface, family.familyId).toBe(false);
        expect("meta" in surface, family.familyId).toBe(false);
      }
    }
    for (const driver of providerRegistry.drivers) {
      expect(
        providerRegistry.optionSchemas.some(
          (schema) => schema.optionSchemaId === driver.optionSchemaId,
        ),
        driver.driverId,
      ).toBe(true);
    }
  });

  it("writes one shared model policy without Surface settings JSON", () => {
    const family = familyById("family.grok_oauth")!;
    const draft = updateBundleModel(
      createProviderBundleDraft(family),
      "single",
      "grok-shared-model",
    );
    draft.accountId = "grok-account";
    draft.accountGeneration = 7;

    const write = toProviderBundleWriteDraft(draft);
    expect(write.modelPolicyScope).toBe("global");
    expect(write.modelPolicy).toBe("single");
    expect(write.upstreamModel).toBe("grok-shared-model");
    expect(write.managedAccount).toEqual({
      accountId: "grok-account",
      authIdentityGeneration: 7,
    });
    expect(
      write.surfaces.every((surface) => !("settingsConfig" in surface)),
    ).toBe(true);

    const passthrough = toProviderBundleWriteDraft(
      updateBundleModel(draft, "passthrough", "ignored-model"),
    );
    expect(passthrough.modelPolicy).toBe("passthrough");
    expect(passthrough.upstreamModel).toBeUndefined();
  });

  it("writes independent model policies only at configurable Surface scope", () => {
    const family = familyById("family.grok_oauth")!;
    let draft = changeModelPolicyScope(
      createProviderBundleDraft(family),
      "per_app",
    );
    draft.accountId = "grok-account";
    draft.accountGeneration = 7;
    draft.surfaces[0]!.modelPolicy = "single";
    draft.surfaces[0]!.upstreamModel = "claude-model";
    draft.surfaces[1]!.modelPolicy = "passthrough";
    draft.surfaces[2]!.modelPolicy = "single";
    draft.surfaces[2]!.upstreamModel = "gemini-model";

    expect(supportsPerAppModelPolicy(family)).toBe(true);
    expect(perAppModelPoliciesDiffer(draft)).toBe(true);
    expect(validateProviderBundleDraft(draft)).toBeNull();

    const write = toProviderBundleWriteDraft(draft);
    expect(write).toMatchObject({
      modelPolicyScope: "per_app",
      modelPolicy: undefined,
      upstreamModel: undefined,
      surfaces: [
        {
          app: "claude",
          modelPolicy: "single",
          upstreamModel: "claude-model",
        },
        {
          app: "codex",
          modelPolicy: "passthrough",
          upstreamModel: undefined,
        },
        {
          app: "gemini",
          modelPolicy: "single",
          upstreamModel: "gemini-model",
        },
      ],
    });

    draft = changeModelPolicyScope(draft, "global");
    expect(draft.modelPolicyScope).toBe("global");
    expect(toProviderBundleWriteDraft(draft).modelPolicy).toBe("single");
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

  it("materializes the Anthropic bearer relay as a Custom HTTP recipe", () => {
    expect(familyById("family.claude_bearer_relay")).toBeUndefined();
    expect(
      providerRegistry.profiles.some(
        (profile) => profile.profileId === "claude.bearer_relay",
      ),
    ).toBe(false);
    const recipe = providerRegistry.customRecipes.find(
      (candidate) => candidate.recipeId === "claude.anthropic_bearer_relay",
    )!;
    const family = familyById("family.custom_http")!;
    const draft = applyCustomRecipeToBundleDraft(
      createProviderBundleDraft(family),
      recipe,
    );

    expect(customRecipeMatchesBundleDraft(draft, recipe)).toBe(true);
    expect(draft).toMatchObject({
      name: "Anthropic Bearer Relay",
      websiteUrl: "",
      icon: "anthropic",
      iconColor: "#D4915D",
      modelPolicyScope: "global",
      modelPolicy: "passthrough",
    });
    expect(
      draft.surfaces.map((surface) => ({
        app: surface.app,
        enabled: surface.enabled,
        customBinding: surface.customBinding,
      })),
    ).toEqual([
      {
        app: "claude",
        enabled: true,
        customBinding: {
          upstreamProtocol: "anthropic_messages",
          authScheme: "bearer",
        },
      },
      {
        app: "codex",
        enabled: false,
        customBinding: {
          upstreamProtocol: "open_ai_responses",
          authScheme: "bearer",
        },
      },
      {
        app: "gemini",
        enabled: false,
        customBinding: {
          upstreamProtocol: "gemini_native",
          authScheme: "api_key",
        },
      },
    ]);

    const claude = draft.surfaces[0]!;
    claude.endpoint = "https://relay.example/v1";
    claude.secret.value = "relay-token";
    expect(validateProviderBundleDraft(draft)).toBeNull();
    expect(toProviderBundleWriteDraft(draft).surfaces[0]).toMatchObject({
      app: "claude",
      enabled: true,
      endpoint: "https://relay.example/v1",
      customBinding: {
        upstreamProtocol: "anthropic_messages",
        authScheme: "bearer",
      },
      credentialPatches: {
        "/settingsConfig/apiKey": {
          action: "replace",
          value: "relay-token",
        },
      },
    });
  });

  it("locks Claude OAuth while OpenAI OAuth uses Claude model policies", () => {
    const claudeFamily = familyById("family.claude_oauth")!;
    const claudeDraft = createProviderBundleDraft(claudeFamily);
    expect(modelPoliciesForFamily(claudeFamily)).toEqual(["passthrough"]);
    claudeDraft.accountId = "official-account";
    claudeDraft.accountGeneration = 1;
    claudeDraft.modelPolicy = "single";
    claudeDraft.upstreamModel = "forced-model";
    expect(validateProviderBundleDraft(claudeDraft)).toBe(
      "Provider model policy is invalid",
    );

    const openaiFamily = familyById("family.openai_oauth")!;
    expect(supportsPerAppModelPolicy(openaiFamily)).toBe(false);
    const openaiDraft = createProviderBundleDraft(openaiFamily);
    expect(modelPoliciesForFamily(openaiFamily)).toEqual([
      "single",
      "passthrough",
    ]);
    expect(openaiDraft.modelPolicy).toBe("single");
    expect(openaiDraft.upstreamModel).toBe("gpt-5.6-sol");
    openaiDraft.accountId = "official-account";
    openaiDraft.accountGeneration = 1;
    expect(validateProviderBundleDraft(openaiDraft)).toBeNull();

    openaiDraft.modelPolicy = "passthrough";
    openaiDraft.upstreamModel = "";
    expect(validateProviderBundleDraft(openaiDraft)).toBeNull();
  });

  it("restores the OpenAI OAuth Bundle model policy from Claude", () => {
    const family = familyById("family.openai_oauth")!;
    const resource = (
      app: "claude" | "codex",
      profileId: string,
      modelMapping: Record<string, string>,
    ): ProviderResource => ({
      app,
      provider: {
        id: "openai-bundle",
        name: "OpenAI OAuth",
        settingsConfig: { modelMapping },
      },
      providerType: "codex_oauth",
      providerTypeId: "codex_oauth",
      revision: 3,
      profileId,
      identity: { status: "bound" },
      credentialConfigured: true,
      credentialSlots: [],
    });
    const bundle: ProviderBundleView = {
      id: "openai-bundle",
      familyId: family.familyId,
      revision: 3,
      name: "OpenAI OAuth",
      modelPolicyScope: "global",
      supportedApps: ["claude", "codex"],
      enabledApps: ["claude", "codex"],
      credentialConfigured: true,
      credentialSlots: [],
      surfaces: {
        claude: resource("claude", "claude.openai_oauth", {
          mode: "single",
          upstreamModel: "persisted-claude-model",
        }),
        codex: resource("codex", "codex.openai_oauth", {
          mode: "passthrough",
        }),
      },
    };

    const edited = editProviderBundleDraft(bundle);
    expect(edited.modelPolicy).toBe("single");
    expect(edited.upstreamModel).toBe("persisted-claude-model");
  });

  it("writes shared credentials at Bundle scope and omits fixed endpoints", () => {
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
      expect(surface.endpoint).toBeUndefined();
    }
  });

  it("keeps Custom HTTP endpoint, credentials, auth field, and transport typed", () => {
    const family = familyById("family.custom_http")!;
    const draft = createProviderBundleDraft(family);
    draft.surfaces = draft.surfaces.map((surface, index) => ({
      ...updateSurfaceEndpoint(
        surface,
        `https://${surface.app}.example/v${index + 1}`,
      ),
      testModel: `${surface.app}-health-model`,
      transport: {
        timeoutMs: "60000",
        streamFirstByteTimeoutMs: "15000",
        streamIdleTimeoutMs: "45000",
      },
      secret: {
        configured: false,
        value: `${surface.app}-secret`,
        clear: false,
      },
    }));
    draft.surfaces[0]!.customBinding = {
      upstreamProtocol: "anthropic_messages",
      authScheme: "custom_header",
    };
    draft.surfaces[0]!.driverOptions.apiKeyField = "x-api-key";

    expect(validateProviderBundleDraft(draft)).toBeNull();
    const write = toProviderBundleWriteDraft(draft);
    expect(write.credentialPatches).toEqual({});
    expect(write.surfaces[0]).toMatchObject({
      endpoint: "https://claude.example/v1",
      testModel: "claude-health-model",
      transport: {
        timeoutMs: 60000,
        streamFirstByteTimeoutMs: 15000,
        streamIdleTimeoutMs: 45000,
      },
      driverOptions: { apiKeyField: "x-api-key" },
      credentialPatches: {
        "/settingsConfig/apiKey": {
          action: "replace",
          value: "claude-secret",
        },
      },
    });
  });

  it("validates Custom Header names and timeout bounds", () => {
    const family = familyById("family.custom_http")!;
    const draft = createProviderBundleDraft(family);
    draft.surfaces = draft.surfaces.map((item) =>
      updateSurfaceEndpoint(item, `https://${item.app}.example/v1`),
    );
    const surface = draft.surfaces[0]!;
    surface.customBinding = {
      upstreamProtocol: "anthropic_messages",
      authScheme: "custom_header",
    };
    surface.driverOptions.apiKeyField = "bad header";
    surface.secret.value = "secret";
    expect(validateProviderBundleDraft(draft)).toBe(
      "claude authentication header name is invalid",
    );

    surface.driverOptions.apiKeyField = "x-api-key";
    surface.transport.timeoutMs = "999";
    expect(validateProviderBundleDraft(draft)).toBe(
      "claude request timeout is invalid",
    );

    surface.transport.timeoutMs = "60000";
    surface.driverOptions.apiKeyField = "Host";
    expect(validateProviderBundleDraft(draft)).toBe(
      "claude authentication header name is invalid",
    );

    surface.driverOptions.apiKeyField = "x-api-key";
    surface.headers = [
      {
        id: "managed-header",
        name: "Authorization",
        configured: false,
        value: "shadow-secret",
        removed: false,
      },
    ];
    expect(validateProviderBundleDraft(draft)).toBe(
      "claude custom header name is invalid or repeated",
    );
  });

  it("requires a new Custom Header secret when its credential slot changes", () => {
    const family = familyById("family.custom_http")!;
    const draft = createProviderBundleDraft(family);
    draft.surfaces = draft.surfaces.map((surface) => ({
      ...updateSurfaceEndpoint(surface, `https://${surface.app}.example/v1`),
      secret: {
        configured: false,
        value: `${surface.app}-secret`,
        clear: false,
      },
    }));
    draft.surfaces[0]!.headers = [
      {
        id: "configured-header",
        name: "x-new-route~id",
        originalName: "x-old-route~id",
        configured: true,
        value: "",
        removed: false,
      },
      {
        id: "removed-destination-header",
        name: "x-new-route~id",
        originalName: "x-new-route~id",
        configured: true,
        value: "",
        removed: true,
      },
    ];

    expect(validateProviderBundleDraft(draft)).toBe(
      "claude custom header value must be re-entered after renaming",
    );
    expect(
      toProviderBundleWriteDraft(draft).surfaces[0]?.credentialPatches,
    ).toEqual({
      "/settingsConfig/apiKey": {
        action: "replace",
        value: "claude-secret",
      },
      "/settingsConfig/extraHeaders/x-old-route~0id": { action: "clear" },
      "/settingsConfig/extraHeaders/x-new-route~0id": { action: "clear" },
    });

    draft.surfaces[0]!.headers[0]!.value = "new-route-secret";
    expect(validateProviderBundleDraft(draft)).toBeNull();
    expect(
      toProviderBundleWriteDraft(draft).surfaces[0]?.credentialPatches,
    ).toMatchObject({
      "/settingsConfig/extraHeaders/x-old-route~0id": { action: "clear" },
      "/settingsConfig/extraHeaders/x-new-route~0id": {
        action: "replace",
        value: "new-route-secret",
      },
    });
  });

  it("decodes escaped Custom Header credential slots for editing", () => {
    const family = familyById("family.custom_http")!;
    const source = createProviderBundleDraft(family);
    const surfaces = Object.fromEntries(
      family.surfaces.map((surface) => {
        const resource: ProviderResource = {
          app: surface.app,
          provider: {
            id: source.id,
            name: source.name,
            settingsConfig:
              surface.app === "claude"
                ? {
                    modelMapping: {
                      mode: "single",
                      upstreamModel: "persisted-disabled-model",
                    },
                    testModel: "persisted-health-model",
                    transport: {
                      timeoutMs: 61_000,
                      streamFirstByteTimeoutMs: 16_000,
                      streamIdleTimeoutMs: 46_000,
                    },
                  }
                : {},
            meta:
              surface.app === "claude"
                ? { customUserAgent: "persisted-agent/1" }
                : undefined,
          },
          providerType: "custom",
          providerTypeId: "custom",
          revision: 2,
          profileId: surface.profileId,
          customBinding: source.surfaces.find(
            (candidate) => candidate.app === surface.app,
          )!.customBinding,
          identity: { status: "bound" },
          credentialConfigured: true,
          credentialSlots:
            surface.app === "claude"
              ? [
                  "/settingsConfig/apiKey",
                  "/settingsConfig/extraHeaders/x-route~0id",
                ]
              : ["/settingsConfig/apiKey"],
        };
        if (surface.app !== "claude") {
          resource.runtime = runtime(
            resource,
            `https://${surface.app}.example/v1`,
          );
        }
        return [surface.app, resource];
      }),
    ) as ProviderBundleView["surfaces"];
    const view: ProviderBundleView = {
      id: source.id,
      familyId: family.familyId,
      revision: 2,
      name: source.name,
      modelPolicyScope: "global",
      supportedApps: family.surfaces.map((surface) => surface.app),
      enabledApps: ["codex", "gemini"],
      credentialConfigured: true,
      credentialSlots: [
        "/settingsConfig/apiKey",
        "/settingsConfig/extraHeaders/x-route~0id",
      ],
      surfaces,
    };

    const edited = editProviderBundleDraft(view);
    expect(edited.modelPolicy).toBe("single");
    expect(edited.upstreamModel).toBe("persisted-disabled-model");
    expect(edited.surfaces[0]?.runtime).toBeUndefined();
    expect(edited.surfaces[0]?.testModel).toBe("persisted-health-model");
    expect(edited.surfaces[0]?.transport).toEqual({
      timeoutMs: "61000",
      streamFirstByteTimeoutMs: "16000",
      streamIdleTimeoutMs: "46000",
    });
    expect(edited.surfaces[0]?.driverOptions.customUserAgent).toBe(
      "persisted-agent/1",
    );
    expect(edited.surfaces[0]?.headers).toMatchObject([
      {
        name: "x-route~id",
        originalName: "x-route~id",
        configured: true,
      },
    ]);
  });

  it("duplicates effective configuration without reusing stored secrets", () => {
    const family = familyById("family.openrouter")!;
    const source = createProviderBundleDraft(family);
    const [{ pointer }] = familyCredentialSlots(family);
    const surfaces = Object.fromEntries(
      family.surfaces.map((surface) => {
        const resource: ProviderResource = {
          app: surface.app,
          provider: {
            id: source.id,
            name: source.name,
            settingsConfig: {},
          },
          providerType: "openrouter",
          providerTypeId: "openrouter",
          revision: 4,
          profileId: surface.profileId,
          identity: { status: "bound" },
          credentialConfigured: true,
          credentialSlots: [pointer],
        };
        resource.runtime = runtime(resource, "https://openrouter.ai/api");
        return [surface.app, resource];
      }),
    ) as ProviderBundleView["surfaces"];
    const view: ProviderBundleView = {
      id: source.id,
      familyId: family.familyId,
      revision: 4,
      name: source.name,
      websiteUrl: source.websiteUrl,
      modelPolicyScope: "global",
      supportedApps: family.surfaces.map((surface) => surface.app),
      enabledApps: family.surfaces.map((surface) => surface.app),
      credentialConfigured: true,
      credentialSlots: [pointer],
      surfaces,
    };

    const edited = editProviderBundleDraft(view);
    expect(edited.surfaces[0]?.runtime?.runtimeFingerprint).toBe("fixture");
    expect(edited.surfaces[0]?.testModel).toBe("health-model");
    expect(edited.surfaces[0]?.transport.timeoutMs).toBe("45000");

    const duplicate = duplicateProviderBundleDraft(view);
    expect(duplicate.id).not.toBe(source.id);
    expect(duplicate.expectedRevision).toBeUndefined();
    expect(duplicate.surfaces.every((surface) => surface.runtime == null)).toBe(
      true,
    );
    expect(toProviderBundleWriteDraft(duplicate).credentialPatches).toEqual({});
    expect(validateProviderBundleDraft(duplicate)).toBe(
      "Configure the required credential",
    );
  });
});
