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
  normalizeBundleTestApp,
  perAppModelPoliciesDiffer,
  providerBundleIdentityEditable,
  requiresPerAppModelPolicy,
  supportsPerAppModelPolicy,
  toProviderBundleWriteDraft,
  updateBundleModel,
  updateSurfaceEndpoint,
  validateProviderBundleDraft,
  validateProviderBundleDraftIssue,
} from "./bundleDraft";
import { createDraftForSelectedFamily } from "./bundleDefaults";

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

function openAiOAuthBundle(
  codexWebsocketEnabled?: boolean,
): ProviderBundleView {
  const family = familyById("family.openai_oauth")!;
  const id = "openai-oauth-bundle";
  const surfaces = Object.fromEntries(
    family.surfaces.map((surface) => [
      surface.app,
      {
        app: surface.app,
        provider: {
          id,
          name: family.label,
          settingsConfig: {},
          meta: {
            providerType: "codex_oauth",
            ...(codexWebsocketEnabled === undefined
              ? {}
              : { codexWebsocketEnabled }),
          },
        },
        providerType: "codex_oauth",
        providerTypeId: "codex_oauth",
        revision: 1,
        profileId: surface.profileId,
        identity: { status: "bound" as const },
        credentialConfigured: false,
        credentialSlots: [],
      } satisfies ProviderResource,
    ]),
  ) as ProviderBundleView["surfaces"];
  return {
    id,
    familyId: family.familyId,
    revision: 1,
    name: family.label,
    modelPolicyScope: "global",
    testApp: "codex",
    surfaceTestModels: {},
    transport: {},
    supportedApps: family.surfaces.map((surface) => surface.app),
    enabledApps: family.surfaces.map((surface) => surface.app),
    credentialConfigured: false,
    credentialSlots: [],
    surfaces,
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
      expect(draft.testApp, family.familyId).toBe(
        family.familyId === "family.openai_oauth" &&
          draft.surfaces.some(
            (surface) => surface.app === "codex" && surface.enabled,
          )
          ? "codex"
          : (["claude", "codex", "gemini"] as const).find((app) =>
              draft.surfaces.some(
                (surface) => surface.app === app && surface.enabled,
              ),
            ),
      );
      for (const surface of toProviderBundleWriteDraft(draft).surfaces) {
        expect("settingsConfig" in surface, family.familyId).toBe(false);
        expect("meta" in surface, family.familyId).toBe(false);
        expect("testModel" in surface, family.familyId).toBe(false);
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

  it("selects one enabled test App and normalizes it after Surface changes", () => {
    const grok = createProviderBundleDraft(familyById("family.grok_oauth")!);
    expect(grok.testApp).toBe("claude");

    grok.surfaces[0]!.enabled = false;
    expect(normalizeBundleTestApp(grok).testApp).toBe("codex");

    const openai = createProviderBundleDraft(
      familyById("family.openai_oauth")!,
    );
    expect(openai.testApp).toBe("codex");
  });

  it("creates all Copilot surfaces with one managed-account binding", () => {
    const family = familyById("family.github_copilot")!;
    const draft = createProviderBundleDraft(family);
    expect(
      draft.surfaces.map(({ app, profileId, enabled }) => ({
        app,
        profileId,
        enabled,
      })),
    ).toEqual([
      {
        app: "claude",
        profileId: "claude.github_copilot",
        enabled: true,
      },
      {
        app: "codex",
        profileId: "codex.github_copilot",
        enabled: true,
      },
      {
        app: "gemini",
        profileId: "gemini.github_copilot",
        enabled: true,
      },
    ]);
    expect(supportsPerAppModelPolicy(family)).toBe(true);
    expect(requiresPerAppModelPolicy(family)).toBe(false);
    expect(draft.modelPolicyScope).toBe("per_app");
    expect(
      draft.surfaces.map(({ app, upstreamModel }) => ({ app, upstreamModel })),
    ).toEqual([
      { app: "claude", upstreamModel: "claude-sonnet-5" },
      { app: "codex", upstreamModel: "gpt-5.5" },
      { app: "gemini", upstreamModel: "gemini-3.5-flash" },
    ]);

    draft.accountId = "copilot-account";
    draft.accountGeneration = 4;
    const write = toProviderBundleWriteDraft(draft);
    expect(write.managedAccount).toEqual({
      accountId: "copilot-account",
      authIdentityGeneration: 4,
    });
    expect(write.modelPolicyScope).toBe("per_app");
    expect(write.modelPolicy).toBeUndefined();
    expect(write.surfaces).toHaveLength(3);
    expect(write.surfaces.every((surface) => surface.enabled)).toBe(true);

    const global = changeModelPolicyScope(draft, "global");
    expect(global.modelPolicyScope).toBe("global");
    expect(validateProviderBundleDraft(global)).toBeNull();
  });

  it("defaults new OpenAI OAuth WebSocket options on without affecting other drivers", () => {
    const openai = createProviderBundleDraft(
      familyById("family.openai_oauth")!,
    );
    expect(
      openai.surfaces.map(
        (surface) => surface.driverOptions.codexWebsocketEnabled,
      ),
    ).toEqual([true, true]);
    expect(
      toProviderBundleWriteDraft(openai).surfaces.map(
        (surface) => surface.driverOptions?.codexWebsocketEnabled,
      ),
    ).toEqual([true, true]);

    const openrouter = createProviderBundleDraft(
      familyById("family.openrouter")!,
    );
    expect(
      openrouter.surfaces.every(
        (surface) => surface.driverOptions.codexWebsocketEnabled === undefined,
      ),
    ).toBe(true);
  });

  it.each([
    ["missing", undefined, true],
    ["enabled", true, true],
    ["disabled", false, false],
  ] as const)(
    "preserves the %s OpenAI OAuth WebSocket setting while editing",
    (_label, configured, expected) => {
      const edited = editProviderBundleDraft(openAiOAuthBundle(configured));
      expect(
        edited.surfaces.map(
          (surface) => surface.driverOptions.codexWebsocketEnabled,
        ),
      ).toEqual([expected, expected]);
      expect(
        toProviderBundleWriteDraft(edited).surfaces.map(
          (surface) => surface.driverOptions?.codexWebsocketEnabled,
        ),
      ).toEqual([expected, expected]);
    },
  );

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
    expect(validateProviderBundleDraftIssue(claudeDraft)).toMatchObject({
      code: "modelPolicyInvalid",
      field: "modelPolicy",
    });

    const openaiFamily = familyById("family.openai_oauth")!;
    expect(supportsPerAppModelPolicy(openaiFamily)).toBe(true);
    expect(requiresPerAppModelPolicy(openaiFamily)).toBe(true);
    const openaiDraft = createProviderBundleDraft(openaiFamily);
    expect(openaiDraft.modelPolicyScope).toBe("per_app");
    expect(openaiDraft.surfaces).toMatchObject([
      {
        app: "claude",
        modelPolicy: "single",
        upstreamModel: "gpt-5.6-sol",
      },
      { app: "codex", modelPolicy: "passthrough" },
    ]);
    openaiDraft.accountId = "official-account";
    openaiDraft.accountGeneration = 1;
    expect(validateProviderBundleDraft(openaiDraft)).toBeNull();
    expect(toProviderBundleWriteDraft(openaiDraft)).toMatchObject({
      modelPolicyScope: "per_app",
      modelPolicy: undefined,
      upstreamModel: undefined,
      surfaces: [
        {
          app: "claude",
          modelPolicy: "single",
          upstreamModel: "gpt-5.6-sol",
        },
        {
          app: "codex",
          modelPolicy: undefined,
          upstreamModel: undefined,
        },
      ],
    });

    openaiDraft.surfaces[0]!.modelPolicy = "passthrough";
    openaiDraft.surfaces[0]!.upstreamModel = "";
    expect(validateProviderBundleDraft(openaiDraft)).toBeNull();
    expect(changeModelPolicyScope(openaiDraft, "global").modelPolicyScope).toBe(
      "per_app",
    );
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
      modelPolicyScope: "per_app",
      testApp: "codex",
      surfaceTestModels: {},
      transport: {},
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
    expect(edited.modelPolicyScope).toBe("per_app");
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
    draft.transport = {
      timeoutSeconds: "60",
      streamFirstByteTimeoutSeconds: "15",
      streamIdleTimeoutSeconds: "45",
    };
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
    draft.testApp = "claude";
    draft.testModel = "claude-health-model";
    draft.surfaces[0]!.customBinding = {
      upstreamProtocol: "anthropic_messages",
      authScheme: "custom_header",
    };
    draft.surfaces[0]!.driverOptions.apiKeyField = "x-api-key";

    expect(validateProviderBundleDraft(draft)).toBeNull();
    const write = toProviderBundleWriteDraft(draft);
    expect(write.credentialPatches).toEqual({});
    expect(write).toMatchObject({
      testApp: "claude",
      testModel: "claude-health-model",
      transport: {
        timeoutMs: 60000,
        streamFirstByteTimeoutMs: 15000,
        streamIdleTimeoutMs: 45000,
      },
    });
    expect(write.surfaces[0]).toMatchObject({
      endpoint: "https://claude.example/v1",
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
    draft.transport.timeoutSeconds = "3601";
    expect(validateProviderBundleDraft(draft)).toBe(
      "Provider request timeout is invalid",
    );

    draft.transport.timeoutSeconds = "60";
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
      testApp: "codex",
      testModel: "persisted-health-model",
      surfaceTestModels: { gemini: "persisted-gemini-health-model" },
      transport: {
        timeoutMs: 61_000,
        streamFirstByteTimeoutMs: 16_000,
        streamIdleTimeoutMs: 46_000,
      },
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
    expect(edited.testApp).toBe("codex");
    expect(edited.testModel).toBe("persisted-health-model");
    expect(edited.surfaceTestModels).toEqual({
      claude: "",
      codex: "",
      gemini: "persisted-gemini-health-model",
    });
    expect(edited.transport).toEqual({
      timeoutSeconds: "61",
      streamFirstByteTimeoutSeconds: "16",
      streamIdleTimeoutSeconds: "46",
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
      testApp: "claude",
      testModel: "health-model",
      surfaceTestModels: {},
      transport: {},
      supportedApps: family.surfaces.map((surface) => surface.app),
      enabledApps: family.surfaces.map((surface) => surface.app),
      credentialConfigured: true,
      credentialSlots: [pointer],
      surfaces,
    };

    const edited = editProviderBundleDraft(view);
    expect(edited.secrets[pointer]).toEqual({
      configured: true,
      value: "",
      clear: false,
    });
    expect(toProviderBundleWriteDraft(edited).credentialPatches).toEqual({
      [pointer]: { action: "keep" },
    });
    expect(edited.surfaces[0]?.runtime?.runtimeFingerprint).toBe("fixture");
    expect(edited.testApp).toBe("claude");
    expect(edited.testModel).toBe("health-model");
    expect(edited.transport.timeoutSeconds).toBe("");

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
    expect(validateProviderBundleDraftIssue(duplicate)).toMatchObject({
      code: "credentialRequired",
      field: "credential",
    });
  });

  it("applies the first Custom HTTP recipe when creating a selected Family", () => {
    const draft = createDraftForSelectedFamily(
      familyById("family.custom_http")!,
    );
    expect(customRecipeMatchesBundleDraft(draft, providerRegistry.customRecipes[0]!)).toBe(
      true,
    );
    expect(draft.surfaces.find((surface) => surface.app === "claude")?.enabled).toBe(
      true,
    );
    expect(
      draft.surfaces
        .filter((surface) => surface.app !== "claude")
        .every((surface) => !surface.enabled),
    ).toBe(true);
  });
});
