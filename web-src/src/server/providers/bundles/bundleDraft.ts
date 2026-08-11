import type {
  ProviderBundleSurfaceWriteDraft,
  ProviderBundleView,
  ProviderBundleWriteDraft,
  ProviderCredentialPatches,
  ProviderCustomBinding,
  ProviderModelPolicyScope,
  ProviderResource,
  ProviderRuntimePlan,
} from "@/lib/api/providers";
import {
  customPolicyForProfile,
  driverForProfile,
  familyById,
  modelPoliciesForProfile,
  optionSchemaForDriver,
  profileById,
  providerRegistry,
  type CoreProviderApp,
  type ProviderCustomRecipe,
  type ProviderFamilySpec,
  type ProviderModelPolicy,
  type ProviderRegistryProfile,
} from "@/server/providerRegistry";
import {
  createDraftForProfile,
  profileAllowsEndpointEditing,
  readEndpoint,
  readModelPolicy,
  readUpstreamModel,
} from "@/server/providers/editor/providerDraft";

export const PRIMARY_SECRET_SLOT = "/settingsConfig/apiKey";
export const EXTRA_HEADER_PREFIX = "/settingsConfig/extraHeaders/";

const CUSTOM_AUTH_HEADER_DENYLIST = new Set([
  "proxy-authorization",
  "proxy-authenticate",
  "host",
  "content-length",
  "content-type",
  "connection",
  "keep-alive",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
  "user-agent",
]);

const EXTRA_HEADER_DENYLIST = new Set([
  ...CUSTOM_AUTH_HEADER_DENYLIST,
  "authorization",
  "x-api-key",
  "api-key",
  "x-goog-api-key",
]);

function escapePointerSegment(value: string): string {
  return value.replace(/~/g, "~0").replace(/\//g, "~1");
}

function unescapePointerSegment(value: string): string {
  return value.replace(/~1/g, "/").replace(/~0/g, "~");
}

function extraHeaderSlot(name: string): string {
  return `${EXTRA_HEADER_PREFIX}${escapePointerSegment(name)}`;
}

export const AWS_CREDENTIAL_SLOTS = {
  access_key_id: "/settingsConfig/env/AWS_ACCESS_KEY_ID",
  secret_access_key: "/settingsConfig/env/AWS_SECRET_ACCESS_KEY",
  session_token: "/settingsConfig/env/AWS_SESSION_TOKEN",
} as const;

export interface BundleSecretDraft {
  configured: boolean;
  value: string;
  clear: boolean;
}

export interface BundleHeaderDraft {
  id: string;
  name: string;
  originalName?: string;
  configured: boolean;
  value: string;
  removed: boolean;
}

export interface BundleSurfaceEditorDraft {
  app: CoreProviderApp;
  enabled: boolean;
  profileId: string;
  modelPolicy: ProviderModelPolicy;
  upstreamModel: string;
  endpoint: string;
  testModel: string;
  transport: {
    timeoutMs: string;
    streamFirstByteTimeoutMs: string;
    streamIdleTimeoutMs: string;
  };
  driverOptions: {
    apiKeyField?: string;
    customUserAgent?: string;
    codexFastMode?: boolean;
    codexImageGenerationEnabled?: boolean;
    codexWebsocketEnabled?: boolean;
  };
  customBinding?: ProviderCustomBinding;
  secret: BundleSecretDraft;
  headers: BundleHeaderDraft[];
  runtime?: ProviderRuntimePlan;
}

export interface ProviderBundleEditorDraft {
  id: string;
  familyId: string;
  name: string;
  websiteUrl: string;
  notes: string;
  icon?: string;
  iconColor?: string;
  expectedRevision?: number;
  clientRequestId?: string;
  accountId: string;
  accountGeneration?: number;
  endpoint: string;
  awsRegion: string;
  modelPolicyScope: ProviderModelPolicyScope;
  modelPolicy: ProviderModelPolicy;
  upstreamModel: string;
  secrets: Record<string, BundleSecretDraft>;
  surfaces: BundleSurfaceEditorDraft[];
}

const DEFAULT_CUSTOM_BINDINGS: Record<CoreProviderApp, ProviderCustomBinding> =
  {
    claude: { upstreamProtocol: "anthropic_messages", authScheme: "api_key" },
    codex: { upstreamProtocol: "open_ai_responses", authScheme: "bearer" },
    gemini: { upstreamProtocol: "gemini_native", authScheme: "api_key" },
  };

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function sourceSurface(
  family: ProviderFamilySpec,
  surfaces: BundleSurfaceEditorDraft[],
): BundleSurfaceEditorDraft {
  const sourceProfile = profileById(family.credentialProfileId);
  const source = surfaces.find((surface) => surface.app === sourceProfile?.app);
  if (!source)
    throw new Error(`Family ${family.familyId} has no credential Surface`);
  return source;
}

export function providerBundleIdentityEditable(
  family: ProviderFamilySpec,
): boolean {
  return profileById(family.credentialProfileId)?.formComposition === "custom";
}

function canonicalBundleIdentity(family: ProviderFamilySpec): {
  name: string;
  websiteUrl: string;
} {
  const profile = profileById(family.credentialProfileId);
  if (!profile)
    throw new Error(`Unknown profile ${family.credentialProfileId}`);
  const preset = createDraftForProfile(profile);
  return { name: family.label, websiteUrl: preset.websiteUrl };
}

function modelControlProfilesForFamily(
  family: ProviderFamilySpec,
): ProviderRegistryProfile[] {
  const profiles = family.surfaces.map((surface) =>
    profileById(surface.profileId),
  );
  if (profiles.some((profile) => !profile)) return [];
  const resolved = profiles as ProviderRegistryProfile[];
  const configurable = resolved.filter(
    (profile) => modelPoliciesForProfile(profile).length > 1,
  );
  return configurable.length > 0 ? configurable : resolved;
}

function profileHasConfigurableModelPolicy(
  profile: ProviderRegistryProfile,
): boolean {
  return modelPoliciesForProfile(profile).length > 1;
}

export function configurableModelSurfaceCount(
  family: ProviderFamilySpec,
): number {
  return family.surfaces.filter((surface) => {
    const profile = profileById(surface.profileId);
    return profile ? profileHasConfigurableModelPolicy(profile) : false;
  }).length;
}

export function supportsPerAppModelPolicy(family: ProviderFamilySpec): boolean {
  return configurableModelSurfaceCount(family) >= 2;
}

function modelControlSurface(
  family: ProviderFamilySpec,
  surfaces: BundleSurfaceEditorDraft[],
): BundleSurfaceEditorDraft {
  const profile = modelControlProfilesForFamily(family)[0];
  const surface = surfaces.find(
    (candidate) => candidate.profileId === profile?.profileId,
  );
  if (!surface)
    throw new Error(`Family ${family.familyId} has no model control Surface`);
  return surface;
}

export function modelPoliciesForFamily(
  family: ProviderFamilySpec,
): ProviderModelPolicy[] {
  const profiles = modelControlProfilesForFamily(family);
  const first = profiles[0];
  if (!first) return [];
  return modelPoliciesForProfile(first).filter((policy) =>
    profiles.every((profile) =>
      modelPoliciesForProfile(profile).includes(policy),
    ),
  );
}

export function defaultUpstreamModelForFamily(
  family: ProviderFamilySpec,
): string {
  return (
    modelControlProfilesForFamily(family)
      .map((profile) => profile.defaultUpstreamModel)
      .find(Boolean) ?? ""
  );
}

function initialBundleModel(
  family: ProviderFamilySpec,
  surfaces: BundleSurfaceEditorDraft[],
): { policy: ProviderModelPolicy; upstreamModel: string } {
  const source = modelControlSurface(family, surfaces);
  const allowedPolicies = modelPoliciesForFamily(family);
  return {
    policy: allowedPolicies.includes(source.modelPolicy)
      ? source.modelPolicy
      : (allowedPolicies[0] ?? source.modelPolicy),
    upstreamModel: source.upstreamModel,
  };
}

function credentialSlotsForFamily(
  family: ProviderFamilySpec,
): Array<{ logical: string; pointer: string }> {
  const profile = profileById(family.credentialProfileId);
  if (!profile) return [];
  if (profile.credentialPolicy.mode === "static_secret") {
    return [
      {
        logical: profile.credentialPolicy.slots[0] ?? "api_key",
        pointer: PRIMARY_SECRET_SLOT,
      },
    ];
  }
  if (profile.credentialPolicy.mode === "aws") {
    return profile.credentialPolicy.slots.map((logical) => ({
      logical,
      pointer:
        AWS_CREDENTIAL_SLOTS[logical as keyof typeof AWS_CREDENTIAL_SLOTS] ??
        PRIMARY_SECRET_SLOT,
    }));
  }
  return [];
}

function configuredSlot(
  slots: string[],
  canonical: string,
): string | undefined {
  const suffix = canonical.slice(canonical.lastIndexOf("/"));
  return slots.find((slot) => slot === canonical || slot.endsWith(suffix));
}

function surfaceFromResource(
  family: ProviderFamilySpec,
  app: CoreProviderApp,
  profileId: string,
  enabled: boolean,
  resource?: ProviderResource,
): BundleSurfaceEditorDraft {
  const profile = profileById(profileId);
  if (!profile) throw new Error(`Unknown profile ${profileId}`);
  const preset = createDraftForProfile(profile);
  const runtime = resource?.runtime;
  const settings = clone(
    (resource?.provider.settingsConfig ?? preset.settingsConfig) as Record<
      string,
      unknown
    >,
  );
  const configuredHeaderSlots = (resource?.credentialSlots ?? []).filter(
    (slot) => slot.startsWith(EXTRA_HEADER_PREFIX),
  );
  const runtimeHeaders = new Map(
    (runtime?.extraHeaders ?? []).map((header) => [
      header.credentialSlot,
      header.name,
    ]),
  );
  const headers = configuredHeaderSlots.map((slot) => {
    const originalName = unescapePointerSegment(
      slot.slice(EXTRA_HEADER_PREFIX.length),
    );
    return {
      id: crypto.randomUUID(),
      name: originalName || runtimeHeaders.get(slot) || "",
      originalName,
      configured: true,
      value: "",
      removed: false,
    };
  });
  const runtimeOptions = runtime?.driverOptions ?? {};
  const configuredMeta = resource?.provider.meta ?? preset.meta ?? {};
  const configuredTransport = objectOption(settings.transport);
  const endpoint = runtime?.endpoint ?? readEndpoint(settings, app);
  const modelPolicy =
    runtime?.modelPolicy.mode ?? readModelPolicy(settings, profile);
  const upstreamModel =
    runtime?.modelPolicy.mode === "single"
      ? runtime.modelPolicy.upstreamModel
      : (readUpstreamModel(settings) ?? profile.defaultUpstreamModel ?? "");
  return {
    app,
    enabled,
    profileId,
    modelPolicy,
    upstreamModel,
    endpoint,
    testModel: runtime?.testModel ?? stringOption(settings.testModel) ?? "",
    transport: {
      timeoutMs: runtime
        ? String(runtime.transportPolicy.timeoutMs)
        : numberOption(configuredTransport?.timeoutMs),
      streamFirstByteTimeoutMs:
        runtime?.transportPolicy.streamFirstByteTimeoutMs == null
          ? numberOption(configuredTransport?.streamFirstByteTimeoutMs)
          : String(runtime.transportPolicy.streamFirstByteTimeoutMs),
      streamIdleTimeoutMs:
        runtime?.transportPolicy.streamIdleTimeoutMs == null
          ? numberOption(configuredTransport?.streamIdleTimeoutMs)
          : String(runtime.transportPolicy.streamIdleTimeoutMs),
    },
    driverOptions: {
      apiKeyField:
        stringOption(runtimeOptions.apiKeyField) ?? configuredMeta.apiKeyField,
      customUserAgent:
        stringOption(runtimeOptions.customUserAgent) ??
        configuredMeta.customUserAgent,
      codexFastMode:
        booleanOption(runtimeOptions.codexFastMode) ??
        configuredMeta.codexFastMode,
      codexImageGenerationEnabled:
        booleanOption(runtimeOptions.codexImageGenerationEnabled) ??
        configuredMeta.codexImageGenerationEnabled,
      codexWebsocketEnabled:
        booleanOption(runtimeOptions.codexWebsocketEnabled) ??
        configuredMeta.codexWebsocketEnabled,
    },
    customBinding:
      resource?.customBinding ??
      (profile.formComposition === "custom"
        ? clone(DEFAULT_CUSTOM_BINDINGS[app])
        : undefined),
    secret: {
      configured: Boolean(
        resource?.credentialSlots.some(
          (slot) => !slot.startsWith(EXTRA_HEADER_PREFIX),
        ),
      ),
      value: "",
      clear: false,
    },
    headers,
    runtime,
  };
}

function stringOption(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function booleanOption(value: unknown): boolean | undefined {
  return typeof value === "boolean" ? value : undefined;
}

function numberOption(value: unknown): string {
  return typeof value === "number" && Number.isSafeInteger(value)
    ? String(value)
    : "";
}

function objectOption(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

export function createProviderBundleDraft(
  family: ProviderFamilySpec,
): ProviderBundleEditorDraft {
  const id = crypto.randomUUID();
  const surfaces = family.surfaces.map((surface) =>
    surfaceFromResource(
      family,
      surface.app,
      surface.profileId,
      surface.defaultEnabled,
    ),
  );
  const source = sourceSurface(family, surfaces);
  const sourceProfile = profileById(source.profileId)!;
  const sourcePreset = createDraftForProfile(sourceProfile);
  const model = initialBundleModel(family, surfaces);
  const secrets = Object.fromEntries(
    credentialSlotsForFamily(family).map(({ pointer }) => [
      pointer,
      { configured: false, value: "", clear: false },
    ]),
  );
  return {
    id,
    familyId: family.familyId,
    name: family.label,
    websiteUrl: sourcePreset.websiteUrl,
    notes: "",
    icon: sourcePreset.icon,
    iconColor: sourcePreset.iconColor,
    clientRequestId: crypto.randomUUID(),
    accountId: "",
    endpoint: source.endpoint,
    awsRegion: readAwsRegion(
      sourcePreset.settingsConfig as Record<string, unknown>,
    ),
    modelPolicyScope: "global",
    modelPolicy: model.policy,
    upstreamModel: model.upstreamModel,
    secrets,
    surfaces,
  };
}

export function applyCustomRecipeToBundleDraft(
  draft: ProviderBundleEditorDraft,
  recipe: ProviderCustomRecipe,
): ProviderBundleEditorDraft {
  const targetProfile = profileById(recipe.profileId);
  if (!targetProfile || targetProfile.formComposition !== "custom") {
    throw new Error(`Unknown Custom HTTP recipe profile ${recipe.profileId}`);
  }
  if (
    !draft.surfaces.some((surface) => surface.profileId === recipe.profileId)
  ) {
    throw new Error(
      `Recipe ${recipe.recipeId} does not belong to ${draft.familyId}`,
    );
  }
  const defaultModel = targetProfile.defaultUpstreamModel ?? "";
  return {
    ...draft,
    name: recipe.label,
    websiteUrl: "",
    icon: recipe.icon,
    iconColor: recipe.iconColor,
    modelPolicyScope: "global",
    modelPolicy: recipe.modelPolicy,
    upstreamModel:
      recipe.modelPolicy === "single" && !draft.upstreamModel.trim()
        ? defaultModel
        : draft.upstreamModel,
    surfaces: draft.surfaces.map((surface) => {
      const selected = surface.profileId === recipe.profileId;
      if (!selected) return { ...surface, enabled: false };
      return {
        ...surface,
        enabled: true,
        modelPolicy: recipe.modelPolicy,
        upstreamModel:
          recipe.modelPolicy === "single" && !surface.upstreamModel.trim()
            ? defaultModel
            : surface.upstreamModel,
        customBinding: { ...recipe.binding },
        driverOptions: {
          ...surface.driverOptions,
          apiKeyField: undefined,
        },
      };
    }),
  };
}

export function customRecipeMatchesBundleDraft(
  draft: ProviderBundleEditorDraft,
  recipe: ProviderCustomRecipe,
): boolean {
  const target = draft.surfaces.find(
    (surface) => surface.profileId === recipe.profileId,
  );
  return Boolean(
    target?.enabled &&
    draft.surfaces.every((surface) => surface === target || !surface.enabled) &&
    draft.modelPolicyScope === "global" &&
    draft.modelPolicy === recipe.modelPolicy &&
    target.customBinding?.upstreamProtocol ===
      recipe.binding.upstreamProtocol &&
    target.customBinding.authScheme === recipe.binding.authScheme,
  );
}

export function editProviderBundleDraft(
  bundle: ProviderBundleView,
): ProviderBundleEditorDraft {
  const family = familyById(bundle.familyId);
  if (!family) throw new Error(`Unknown family ${bundle.familyId}`);
  const surfaces = family.surfaces.map((surface) =>
    surfaceFromResource(
      family,
      surface.app,
      surface.profileId,
      bundle.enabledApps.includes(surface.app),
      bundle.surfaces[surface.app],
    ),
  );
  const source = sourceSurface(family, surfaces);
  const sourceResource = bundle.surfaces[source.app];
  const binding = sourceResource?.provider.meta?.authBinding;
  const model = initialBundleModel(family, surfaces);
  const identity = providerBundleIdentityEditable(family)
    ? { name: bundle.name, websiteUrl: bundle.websiteUrl ?? "" }
    : canonicalBundleIdentity(family);
  const secrets = Object.fromEntries(
    credentialSlotsForFamily(family).map(({ pointer }) => {
      const actual = configuredSlot(bundle.credentialSlots, pointer) ?? pointer;
      return [
        actual,
        {
          configured: bundle.credentialSlots.includes(actual),
          value: "",
          clear: false,
        },
      ];
    }),
  );
  return {
    id: bundle.id,
    familyId: bundle.familyId,
    name: identity.name,
    websiteUrl: identity.websiteUrl,
    notes: bundle.notes ?? "",
    icon: bundle.icon,
    iconColor: bundle.iconColor,
    expectedRevision: bundle.revision,
    accountId: binding?.accountId ?? "",
    accountGeneration: binding?.authIdentityGeneration,
    endpoint: source.endpoint,
    awsRegion:
      sourceResource?.runtime?.awsRegion ??
      readAwsRegion(
        clone(
          (sourceResource?.provider.settingsConfig ?? {}) as Record<
            string,
            unknown
          >,
        ),
      ),
    modelPolicyScope: bundle.modelPolicyScope,
    modelPolicy: model.policy,
    upstreamModel: model.upstreamModel,
    secrets,
    surfaces,
  };
}

export function duplicateProviderBundleDraft(
  bundle: ProviderBundleView,
): ProviderBundleEditorDraft {
  const source = editProviderBundleDraft(bundle);
  const family = familyById(bundle.familyId);
  if (!family) throw new Error(`Unknown family ${bundle.familyId}`);
  const id = crypto.randomUUID();
  return {
    ...source,
    id,
    name: providerBundleIdentityEditable(family)
      ? `${source.name} copy`
      : source.name,
    expectedRevision: undefined,
    clientRequestId: crypto.randomUUID(),
    secrets: Object.fromEntries(
      Object.entries(source.secrets).map(([slot, secret]) => [
        slot,
        { ...secret, configured: false, value: "", clear: false },
      ]),
    ),
    surfaces: source.surfaces.map((surface) => ({
      ...surface,
      runtime: undefined,
      secret: {
        ...surface.secret,
        configured: false,
        value: "",
        clear: false,
      },
      headers: surface.headers.map((header) => ({
        ...header,
        configured: false,
        value: "",
      })),
    })),
  };
}

export function surfaceModelState(surface: BundleSurfaceEditorDraft): {
  policy: ProviderModelPolicy;
  upstreamModel: string;
} {
  return {
    policy: surface.modelPolicy,
    upstreamModel: surface.upstreamModel,
  };
}

export function modelPoliciesForSurface(
  surface: BundleSurfaceEditorDraft,
): ProviderModelPolicy[] {
  const profile = profileById(surface.profileId);
  return profile ? modelPoliciesForProfile(profile) : [];
}

export function updateSurfaceModel(
  surface: BundleSurfaceEditorDraft,
  policy: ProviderModelPolicy,
  upstreamModel: string,
): BundleSurfaceEditorDraft {
  return { ...surface, modelPolicy: policy, upstreamModel };
}

export function changeModelPolicyScope(
  draft: ProviderBundleEditorDraft,
  scope: ProviderModelPolicyScope,
): ProviderBundleEditorDraft {
  if (scope === draft.modelPolicyScope) return draft;
  if (scope === "global") return { ...draft, modelPolicyScope: scope };
  return {
    ...draft,
    modelPolicyScope: scope,
    surfaces: draft.surfaces.map((surface) => {
      const profile = profileById(surface.profileId);
      if (!profile || !profileHasConfigurableModelPolicy(profile))
        return surface;
      const allowed = modelPoliciesForProfile(profile);
      const policy = allowed.includes(draft.modelPolicy)
        ? draft.modelPolicy
        : (allowed[0] ?? surface.modelPolicy);
      return updateSurfaceModel(
        surface,
        policy,
        policy === "single"
          ? draft.upstreamModel.trim() ||
              surface.upstreamModel ||
              profile.defaultUpstreamModel ||
              ""
          : surface.upstreamModel,
      );
    }),
  };
}

export function perAppModelPoliciesDiffer(
  draft: ProviderBundleEditorDraft,
): boolean {
  const signatures = draft.surfaces.flatMap((surface) => {
    const profile = profileById(surface.profileId);
    if (!profile || !profileHasConfigurableModelPolicy(profile)) return [];
    return [
      surface.modelPolicy === "single"
        ? `single:${surface.upstreamModel.trim()}`
        : "passthrough",
    ];
  });
  return new Set(signatures).size > 1;
}

export function updateBundleModel(
  draft: ProviderBundleEditorDraft,
  policy: ProviderModelPolicy,
  upstreamModel: string,
): ProviderBundleEditorDraft {
  return {
    ...draft,
    modelPolicy: policy,
    upstreamModel,
  };
}

export function surfaceEndpoint(surface: BundleSurfaceEditorDraft): string {
  return surface.endpoint;
}

export function updateSurfaceEndpoint(
  surface: BundleSurfaceEditorDraft,
  endpoint: string,
): BundleSurfaceEditorDraft {
  return { ...surface, endpoint };
}

function readAwsRegion(settings: Record<string, unknown>): string {
  const env = settings.env;
  if (!env || typeof env !== "object" || Array.isArray(env)) return "us-east-1";
  const value = (env as Record<string, unknown>).AWS_REGION;
  return typeof value === "string" && value.trim() ? value.trim() : "us-east-1";
}

function secretPatches(
  secrets: Record<string, BundleSecretDraft>,
): ProviderCredentialPatches {
  const patches: ProviderCredentialPatches = {};
  for (const [slot, secret] of Object.entries(secrets)) {
    if (secret.clear) patches[slot] = { action: "clear" };
    else if (secret.value.trim()) {
      patches[slot] = { action: "replace", value: secret.value.trim() };
    } else if (secret.configured) patches[slot] = { action: "keep" };
  }
  return patches;
}

function customCredentialPatches(
  surface: BundleSurfaceEditorDraft,
): ProviderCredentialPatches {
  const patches = secretPatches({ [PRIMARY_SECRET_SLOT]: surface.secret });
  for (const header of surface.headers) {
    const originalSlot = header.originalName
      ? extraHeaderSlot(header.originalName)
      : undefined;
    const slot = extraHeaderSlot(header.name.trim());
    if (originalSlot && (header.removed || originalSlot !== slot)) {
      patches[originalSlot] = { action: "clear" };
    }
  }
  for (const header of surface.headers) {
    if (header.removed) continue;
    const originalSlot = header.originalName
      ? extraHeaderSlot(header.originalName)
      : undefined;
    const slot = extraHeaderSlot(header.name.trim());
    const renamed = Boolean(originalSlot && originalSlot !== slot);
    if (header.value.trim()) {
      patches[slot] = { action: "replace", value: header.value.trim() };
    } else if (header.configured && !renamed) {
      patches[slot] = { action: "keep" };
    }
  }
  return patches;
}

function driverForSurface(profileId: string, binding?: ProviderCustomBinding) {
  const profile = profileById(profileId);
  if (!profile) return undefined;
  const fixed = driverForProfile(profile);
  if (fixed) return fixed;
  if (!binding) return undefined;
  const policy = customPolicyForProfile(profile);
  return providerRegistry.drivers.find(
    (driver) =>
      policy?.allowedDriverIds.includes(driver.driverId) &&
      driver.upstreamProtocol === binding.upstreamProtocol &&
      driver.acceptedAuthSchemes.includes(binding.authScheme),
  );
}

function optionalDuration(value: string): number | undefined {
  const trimmed = value.trim();
  return trimmed ? Number(trimmed) : undefined;
}

function typedDriverOptions(surface: BundleSurfaceEditorDraft) {
  const profile = profileById(surface.profileId);
  const driver = driverForSurface(surface.profileId, surface.customBinding);
  const schema = driver ? optionSchemaForDriver(driver) : undefined;
  const fields = new Set(schema?.fields ?? []);
  const apiKeyField = surface.driverOptions.apiKeyField?.trim();
  const customUserAgent = surface.driverOptions.customUserAgent?.trim();
  return {
    apiKeyField:
      profile?.formComposition === "custom" &&
      fields.has("apiKeyField") &&
      apiKeyField
        ? apiKeyField
        : undefined,
    customUserAgent:
      profile?.formComposition === "custom" &&
      fields.has("customUserAgent") &&
      customUserAgent
        ? customUserAgent
        : undefined,
    codexFastMode: fields.has("codexFastMode")
      ? surface.driverOptions.codexFastMode
      : undefined,
    codexImageGenerationEnabled: fields.has("codexImageGenerationEnabled")
      ? surface.driverOptions.codexImageGenerationEnabled
      : undefined,
    codexWebsocketEnabled: fields.has("codexWebsocketEnabled")
      ? surface.driverOptions.codexWebsocketEnabled
      : undefined,
  } satisfies NonNullable<ProviderBundleSurfaceWriteDraft["driverOptions"]>;
}

function surfaceWriteDraft(
  draft: ProviderBundleEditorDraft,
  family: ProviderFamilySpec,
  surface: BundleSurfaceEditorDraft,
): ProviderBundleSurfaceWriteDraft {
  const profile = profileById(surface.profileId);
  if (!profile) throw new Error(`Unknown profile ${surface.profileId}`);
  const endpoint = profileAllowsEndpointEditing(profile)
    ? family.endpointScope === "bundle"
      ? draft.endpoint.trim()
      : surface.endpoint.trim()
    : "";
  return {
    app: surface.app,
    enabled: surface.enabled,
    profileId: surface.profileId,
    modelPolicy:
      draft.modelPolicyScope === "per_app" &&
      profileHasConfigurableModelPolicy(profile)
        ? surface.modelPolicy
        : undefined,
    upstreamModel:
      draft.modelPolicyScope === "per_app" &&
      profileHasConfigurableModelPolicy(profile) &&
      surface.modelPolicy === "single"
        ? surface.upstreamModel.trim()
        : undefined,
    endpoint: endpoint || undefined,
    testModel: surface.testModel.trim() || undefined,
    transport: {
      timeoutMs: optionalDuration(surface.transport.timeoutMs),
      streamFirstByteTimeoutMs: optionalDuration(
        surface.transport.streamFirstByteTimeoutMs,
      ),
      streamIdleTimeoutMs: optionalDuration(
        surface.transport.streamIdleTimeoutMs,
      ),
    },
    driverOptions: typedDriverOptions(surface),
    extraHeaders:
      profile.formComposition === "custom"
        ? surface.headers
            .filter((header) => !header.removed && header.name.trim())
            .map((header) => header.name.trim())
        : undefined,
    customBinding:
      profile.formComposition === "custom" ? surface.customBinding : undefined,
    credentialPatches:
      profile.formComposition === "custom"
        ? customCredentialPatches(surface)
        : undefined,
  };
}

function validateEndpoint(endpoint: string, required: boolean): boolean {
  if (!endpoint.trim()) return !required;
  try {
    const parsed = new URL(endpoint);
    return (
      /^https?:$/.test(parsed.protocol) &&
      !parsed.username &&
      !parsed.password &&
      Boolean(parsed.hostname)
    );
  } catch {
    return false;
  }
}

function validateDuration(value: string, min: number, max: number): boolean {
  if (!value.trim()) return true;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed >= min && parsed <= max;
}

export function validateProviderBundleDraft(
  draft: ProviderBundleEditorDraft,
): string | null {
  const family = familyById(draft.familyId);
  if (!family) return "Provider family is unavailable";
  if (!draft.name.trim()) return "Provider name is required";
  if (!draft.surfaces.some((surface) => surface.enabled)) {
    return "Enable at least one API Surface";
  }
  const credentialProfile = profileById(family.credentialProfileId);
  if (!credentialProfile) return "Credential profile is unavailable";
  if (
    credentialProfile.credentialPolicy.mode === "managed_account" &&
    !draft.accountId
  ) {
    return "Select an OAuth account";
  }
  if (
    credentialProfile.credentialPolicy.mode === "managed_account" &&
    draft.accountGeneration == null
  ) {
    return "OAuth account identity is unavailable";
  }
  for (const [slot, secret] of Object.entries(draft.secrets)) {
    const optional = slot.endsWith("/AWS_SESSION_TOKEN");
    if (!optional && !secret.configured && !secret.value.trim()) {
      return "Configure the required credential";
    }
    if (!optional && secret.clear)
      return "A required credential cannot be cleared";
  }
  if (credentialProfile.formComposition === "aws" && !draft.awsRegion.trim()) {
    return "AWS region is required";
  }
  if (
    credentialProfile.formComposition === "aws" &&
    !/^[A-Za-z0-9-]{1,64}$/.test(draft.awsRegion.trim())
  ) {
    return "AWS region is invalid";
  }
  if (draft.modelPolicyScope === "global") {
    const allowedModelPolicies = modelPoliciesForFamily(family);
    if (!allowedModelPolicies.includes(draft.modelPolicy)) {
      return "Provider model policy is invalid";
    }
    if (draft.modelPolicy === "single" && !draft.upstreamModel.trim()) {
      return "Upstream model is required";
    }
  } else if (!supportsPerAppModelPolicy(family)) {
    return "This Provider does not support independent App model policies";
  }
  for (const surface of draft.surfaces) {
    const profile = profileById(surface.profileId);
    if (!profile) return `Profile ${surface.profileId} is unavailable`;
    if (
      draft.modelPolicyScope === "per_app" &&
      profileHasConfigurableModelPolicy(profile)
    ) {
      if (!modelPoliciesForProfile(profile).includes(surface.modelPolicy)) {
        return `${surface.app} model policy is invalid`;
      }
      if (surface.modelPolicy === "single" && !surface.upstreamModel.trim()) {
        return `${surface.app} upstream model is required`;
      }
    }
    if (surface.testModel.trim().length > 256) {
      return `${surface.app} test model is too long`;
    }
    if (!validateDuration(surface.transport.timeoutMs, 1_000, 3_600_000)) {
      return `${surface.app} request timeout is invalid`;
    }
    if (
      !validateDuration(
        surface.transport.streamFirstByteTimeoutMs,
        1_000,
        600_000,
      )
    ) {
      return `${surface.app} first-byte timeout is invalid`;
    }
    if (
      !validateDuration(surface.transport.streamIdleTimeoutMs, 1_000, 3_600_000)
    ) {
      return `${surface.app} stream idle timeout is invalid`;
    }
    if (profileAllowsEndpointEditing(profile)) {
      const endpoint =
        family.endpointScope === "bundle" ? draft.endpoint : surface.endpoint;
      if (
        !validateEndpoint(
          endpoint,
          profile.endpointPolicy === "custom" && surface.enabled,
        )
      ) {
        return `${surface.app} endpoint is invalid`;
      }
    }
    if (profile.formComposition === "custom") {
      const policy = customPolicyForProfile(profile);
      if (
        !surface.customBinding ||
        !policy?.protocols.includes(surface.customBinding.upstreamProtocol) ||
        !policy.authSchemes.includes(surface.customBinding.authScheme)
      ) {
        return `${surface.app} custom protocol binding is invalid`;
      }
      const authScheme = surface.customBinding.authScheme;
      const apiKeyField = surface.driverOptions.apiKeyField?.trim();
      if (
        (authScheme === "custom_header" || authScheme === "query") &&
        !apiKeyField
      ) {
        return `${surface.app} authentication field is required`;
      }
      if (
        authScheme === "custom_header" &&
        (!/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(apiKeyField ?? "") ||
          CUSTOM_AUTH_HEADER_DENYLIST.has(apiKeyField?.toLowerCase() ?? ""))
      ) {
        return `${surface.app} authentication header name is invalid`;
      }
      if (
        surface.enabled &&
        !surface.secret.configured &&
        !surface.secret.value.trim()
      ) {
        return `${surface.app} authentication credential is required`;
      }
      const names = new Set<string>();
      for (const header of surface.headers.filter((item) => !item.removed)) {
        const trimmedName = header.name.trim();
        const name = trimmedName.toLowerCase();
        if (
          !/^[!#$%&'*+.^_`|~0-9a-z-]+$/.test(name) ||
          EXTRA_HEADER_DENYLIST.has(name) ||
          names.has(name)
        ) {
          return `${surface.app} custom header name is invalid or repeated`;
        }
        names.add(name);
        if (
          header.originalName != null &&
          header.originalName !== trimmedName &&
          !header.value.trim()
        ) {
          return `${surface.app} custom header value must be re-entered after renaming`;
        }
        if (!header.configured && !header.value.trim()) {
          return `${surface.app} custom header value is required`;
        }
      }
    }
  }
  return null;
}

export function toProviderBundleWriteDraft(
  draft: ProviderBundleEditorDraft,
): ProviderBundleWriteDraft {
  const family = familyById(draft.familyId);
  if (!family) throw new Error(`Unknown family ${draft.familyId}`);
  const identity = providerBundleIdentityEditable(family)
    ? { name: draft.name.trim(), websiteUrl: draft.websiteUrl.trim() }
    : canonicalBundleIdentity(family);
  return {
    id: draft.id,
    familyId: draft.familyId,
    name: identity.name,
    websiteUrl: identity.websiteUrl || undefined,
    notes: draft.notes.trim() || undefined,
    icon: draft.icon,
    iconColor: draft.iconColor,
    modelPolicyScope: draft.modelPolicyScope,
    modelPolicy:
      draft.modelPolicyScope === "global" ? draft.modelPolicy : undefined,
    upstreamModel:
      draft.modelPolicyScope === "global" && draft.modelPolicy === "single"
        ? draft.upstreamModel.trim()
        : undefined,
    managedAccount:
      credentialProfileForFamily(family)?.credentialPolicy.mode ===
        "managed_account" && draft.accountGeneration != null
        ? {
            accountId: draft.accountId,
            authIdentityGeneration: draft.accountGeneration,
          }
        : undefined,
    awsRegion:
      credentialProfileForFamily(family)?.formComposition === "aws"
        ? draft.awsRegion.trim()
        : undefined,
    surfaces: draft.surfaces.map((surface) =>
      surfaceWriteDraft(draft, family, surface),
    ),
    credentialPatches: secretPatches(draft.secrets),
    expectedRevision: draft.expectedRevision,
    clientRequestId: draft.clientRequestId,
  };
}

function credentialProfileForFamily(family: ProviderFamilySpec) {
  return profileById(family.credentialProfileId);
}

export function familyCredentialSlots(
  family: ProviderFamilySpec,
): Array<{ logical: string; pointer: string }> {
  return credentialSlotsForFamily(family);
}
