import type {
  ProviderBundleSurfaceWriteDraft,
  ProviderBundleView,
  ProviderBundleWriteDraft,
  ProviderCredentialPatches,
  ProviderCustomBinding,
  ProviderResource,
} from "@/lib/api/providers";
import {
  customPolicyForProfile,
  driverForProfile,
  familyById,
  modelPoliciesForProfile,
  profileById,
  type CoreProviderApp,
  type ProviderFamilySpec,
  type ProviderModelPolicy,
} from "@/server/providerRegistry";
import {
  createDraftForProfile,
  profileAllowsEndpointEditing,
  readEndpoint,
  readModelPolicy,
  readUpstreamModel,
  setEndpoint,
  setPassthroughModel,
  setSingleModel,
} from "@/server/providers/editor/providerDraft";
import type { ProviderMeta } from "@/types";

export const KEEP_SECRET = "__CC_SWITCH_SECRET_KEEP__";
export const PRIMARY_SECRET_SLOT = "/settingsConfig/apiKey";
export const EXTRA_HEADER_PREFIX = "/settingsConfig/extraHeaders/";

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
  category?: string;
  meta: ProviderMeta;
  settingsText: string;
  customBinding?: ProviderCustomBinding;
  secret: BundleSecretDraft;
  headers: BundleHeaderDraft[];
}

export interface ProviderBundleEditorDraft {
  id: string;
  familyId: string;
  routeKey: string;
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
  secrets: Record<string, BundleSecretDraft>;
  surfaces: BundleSurfaceEditorDraft[];
}

const DEFAULT_CUSTOM_BINDINGS: Record<CoreProviderApp, ProviderCustomBinding> = {
  claude: { upstreamProtocol: "anthropic_messages", authScheme: "api_key" },
  codex: { upstreamProtocol: "open_ai_responses", authScheme: "bearer" },
  gemini: { upstreamProtocol: "gemini_native", authScheme: "api_key" },
};

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function prettySettings(value: Record<string, unknown>): string {
  return JSON.stringify(value, null, 2);
}

function routeSlug(value: string): string {
  const slug = value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 48);
  return slug.length >= 3 ? slug : "provider";
}

function sourceSurface(
  family: ProviderFamilySpec,
  surfaces: BundleSurfaceEditorDraft[],
): BundleSurfaceEditorDraft {
  const sourceProfile = profileById(family.credentialProfileId);
  const source = surfaces.find((surface) => surface.app === sourceProfile?.app);
  if (!source) throw new Error(`Family ${family.familyId} has no credential Surface`);
  return source;
}

function credentialSlotsForFamily(
  family: ProviderFamilySpec,
): Array<{ logical: string; pointer: string }> {
  const profile = profileById(family.credentialProfileId);
  if (!profile) return [];
  if (profile.credentialPolicy.mode === "static_secret") {
    return [{ logical: profile.credentialPolicy.slots[0] ?? "api_key", pointer: PRIMARY_SECRET_SLOT }];
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
  const provider = resource?.provider;
  const settings = clone(
    (provider?.settingsConfig ?? preset.settingsConfig) as Record<string, unknown>,
  );
  const meta = clone((provider?.meta ?? preset.meta) as ProviderMeta);
  const headersValue = settings.extraHeaders;
  const configuredHeaders = new Set(
    (resource?.credentialSlots ?? []).filter((slot) =>
      slot.startsWith(EXTRA_HEADER_PREFIX),
    ),
  );
  const headers =
    headersValue && typeof headersValue === "object" && !Array.isArray(headersValue)
      ? Object.keys(headersValue as Record<string, unknown>).map((name) => ({
          id: crypto.randomUUID(),
          name,
          originalName: name,
          configured: configuredHeaders.has(`${EXTRA_HEADER_PREFIX}${name}`),
          value: "",
          removed: false,
        }))
      : [];
  return {
    app,
    enabled,
    profileId,
    category: provider?.category ?? preset.category,
    meta,
    settingsText: prettySettings(settings),
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
  };
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
  const sourceSettings = parseSettings(source.settingsText);
  const sourcePreset = createDraftForProfile(sourceProfile);
  const secrets = Object.fromEntries(
    credentialSlotsForFamily(family).map(({ pointer }) => [
      pointer,
      { configured: false, value: "", clear: false },
    ]),
  );
  return {
    id,
    familyId: family.familyId,
    routeKey: `${routeSlug(family.label)}-${id.slice(0, 8)}`,
    name: family.label,
    websiteUrl: sourcePreset.websiteUrl,
    notes: "",
    icon: sourcePreset.icon,
    iconColor: sourcePreset.iconColor,
    clientRequestId: crypto.randomUUID(),
    accountId: "",
    endpoint: readEndpoint(sourceSettings, source.app),
    awsRegion: readAwsRegion(sourceSettings),
    secrets,
    surfaces,
  };
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
  const sourceSettings = parseSettings(source.settingsText);
  const binding = sourceResource?.provider.meta?.authBinding;
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
    routeKey: bundle.routeKey,
    name: bundle.name,
    websiteUrl: bundle.websiteUrl ?? "",
    notes: bundle.notes ?? "",
    icon: bundle.icon,
    iconColor: bundle.iconColor,
    expectedRevision: bundle.revision,
    accountId: binding?.accountId ?? "",
    accountGeneration: binding?.authIdentityGeneration,
    endpoint: readEndpoint(sourceSettings, source.app),
    awsRegion: readAwsRegion(sourceSettings),
    secrets,
    surfaces,
  };
}

export function parseSettings(value: string): Record<string, unknown> {
  const parsed = JSON.parse(value) as unknown;
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error("Surface settings must be a JSON object");
  }
  return parsed as Record<string, unknown>;
}

export function surfaceModelState(surface: BundleSurfaceEditorDraft): {
  policy: ProviderModelPolicy;
  upstreamModel: string;
} {
  const profile = profileById(surface.profileId);
  if (!profile) throw new Error(`Unknown profile ${surface.profileId}`);
  const settings = parseSettings(surface.settingsText);
  return {
    policy: readModelPolicy(settings, profile),
    upstreamModel: readUpstreamModel(settings) ?? profile.defaultUpstreamModel ?? "",
  };
}

export function updateSurfaceModel(
  surface: BundleSurfaceEditorDraft,
  policy: ProviderModelPolicy,
  upstreamModel: string,
): BundleSurfaceEditorDraft {
  const settings = parseSettings(surface.settingsText);
  if (policy === "single") setSingleModel(settings, surface.app, upstreamModel);
  else setPassthroughModel(settings);
  return { ...surface, settingsText: prettySettings(settings) };
}

export function surfaceEndpoint(surface: BundleSurfaceEditorDraft): string {
  return readEndpoint(parseSettings(surface.settingsText), surface.app);
}

export function updateSurfaceEndpoint(
  surface: BundleSurfaceEditorDraft,
  endpoint: string,
): BundleSurfaceEditorDraft {
  const settings = parseSettings(surface.settingsText);
  setEndpoint(settings, surface.app, endpoint);
  return { ...surface, settingsText: prettySettings(settings) };
}

function readAwsRegion(settings: Record<string, unknown>): string {
  const env = settings.env;
  if (!env || typeof env !== "object" || Array.isArray(env)) return "us-east-1";
  const value = (env as Record<string, unknown>).AWS_REGION;
  return typeof value === "string" && value.trim() ? value.trim() : "us-east-1";
}

function setAwsRegion(settings: Record<string, unknown>, region: string): void {
  const current = settings.env;
  const env =
    current && typeof current === "object" && !Array.isArray(current)
      ? (current as Record<string, unknown>)
      : {};
  env.AWS_REGION = region.trim();
  settings.env = env;
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
      ? `${EXTRA_HEADER_PREFIX}${header.originalName}`
      : undefined;
    if (header.removed) {
      if (originalSlot) patches[originalSlot] = { action: "clear" };
      continue;
    }
    const slot = `${EXTRA_HEADER_PREFIX}${header.name.trim()}`;
    if (originalSlot && originalSlot !== slot) {
      patches[originalSlot] = { action: "clear" };
    }
    if (header.value.trim()) {
      patches[slot] = { action: "replace", value: header.value.trim() };
    } else if (header.configured) {
      patches[slot] = { action: "keep" };
    }
  }
  return patches;
}

function protocolFormat(
  protocol: string | undefined,
): ProviderMeta["apiFormat"] | undefined {
  if (protocol === "anthropic_messages") return "anthropic";
  if (protocol === "open_ai_chat") return "openai_chat";
  if (protocol === "open_ai_responses") return "openai_responses";
  if (protocol === "gemini_native") return "gemini_native";
  return undefined;
}

function surfaceWriteDraft(
  draft: ProviderBundleEditorDraft,
  family: ProviderFamilySpec,
  surface: BundleSurfaceEditorDraft,
): ProviderBundleSurfaceWriteDraft {
  const profile = profileById(surface.profileId);
  if (!profile) throw new Error(`Unknown profile ${surface.profileId}`);
  const settings = parseSettings(surface.settingsText);
  if (family.endpointScope === "bundle" && profileAllowsEndpointEditing(profile)) {
    setEndpoint(settings, surface.app, draft.endpoint);
  }
  if (profile.formComposition === "aws") setAwsRegion(settings, draft.awsRegion);
  const meta = clone(surface.meta);
  meta.providerType = profile.compatibilityProviderType;
  if (profile.credentialPolicy.mode === "managed_account") {
    meta.authBinding = {
      source: "managed_account",
      authProvider: profile.credentialPolicy.accountProviderType,
      accountId: draft.accountId,
      ...(draft.accountGeneration == null
        ? {}
        : { authIdentityGeneration: draft.accountGeneration }),
    };
  } else {
    delete meta.authBinding;
  }
  const protocol =
    profile.formComposition === "custom"
      ? surface.customBinding?.upstreamProtocol
      : driverForProfile(profile)?.upstreamProtocol;
  meta.apiFormat = protocolFormat(protocol);
  if (profile.formComposition === "custom") {
    const headers = Object.fromEntries(
      surface.headers
        .filter((header) => !header.removed && header.name.trim())
        .map((header) => [
          header.name.trim(),
          header.configured && !header.value.trim() ? KEEP_SECRET : "",
        ]),
    );
    settings.extraHeaders = headers;
  }
  return {
    app: surface.app,
    enabled: surface.enabled,
    profileId: surface.profileId,
    settingsConfig: settings,
    category: surface.category,
    meta,
    customBinding:
      profile.formComposition === "custom" ? surface.customBinding : undefined,
    credentialPatches:
      profile.formComposition === "custom"
        ? customCredentialPatches(surface)
        : undefined,
  };
}

export function validateProviderBundleDraft(
  draft: ProviderBundleEditorDraft,
): string | null {
  const family = familyById(draft.familyId);
  if (!family) return "Provider family is unavailable";
  if (!draft.name.trim()) return "Provider name is required";
  if (!/^(?=.{3,64}$)(?=.*[a-z])[a-z0-9_-]+$/.test(draft.routeKey)) {
    return "Route key must use 3-64 lowercase letters, digits, hyphens, or underscores";
  }
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
  for (const [slot, secret] of Object.entries(draft.secrets)) {
    const optional = slot.endsWith("/AWS_SESSION_TOKEN");
    if (!optional && !secret.configured && !secret.value.trim()) {
      return "Configure the required credential";
    }
    if (!optional && secret.clear) return "A required credential cannot be cleared";
  }
  if (
    credentialProfile.formComposition === "aws" &&
    !draft.awsRegion.trim()
  ) {
    return "AWS region is required";
  }
  for (const surface of draft.surfaces) {
    if (!surface.enabled) continue;
    const profile = profileById(surface.profileId);
    if (!profile) return `Profile ${surface.profileId} is unavailable`;
    let settings: Record<string, unknown>;
    try {
      settings = parseSettings(surface.settingsText);
    } catch (error) {
      return error instanceof Error ? error.message : String(error);
    }
    const modelPolicy = readModelPolicy(settings, profile);
    if (!modelPoliciesForProfile(profile).includes(modelPolicy)) {
      return `${surface.app} model policy is invalid`;
    }
    if (modelPolicy === "single" && !readUpstreamModel(settings)?.trim()) {
      return `${surface.app} upstream model is required`;
    }
    if (profile.formComposition === "custom") {
      const endpoint = readEndpoint(settings, surface.app);
      try {
        const parsed = new URL(endpoint);
        if (!/^https?:$/.test(parsed.protocol) || parsed.username || parsed.password) {
          return `${surface.app} endpoint is invalid`;
        }
      } catch {
        return `${surface.app} endpoint is invalid`;
      }
      const policy = customPolicyForProfile(profile);
      if (
        !surface.customBinding ||
        !policy?.protocols.includes(surface.customBinding.upstreamProtocol) ||
        !policy.authSchemes.includes(surface.customBinding.authScheme)
      ) {
        return `${surface.app} custom protocol binding is invalid`;
      }
      if (!surface.secret.configured && !surface.secret.value.trim()) {
        return `${surface.app} authentication credential is required`;
      }
      const names = new Set<string>();
      for (const header of surface.headers.filter((item) => !item.removed)) {
        const name = header.name.trim().toLowerCase();
        if (!/^[!#$%&'*+.^_`|~0-9a-z-]+$/.test(name) || names.has(name)) {
          return `${surface.app} custom header name is invalid or repeated`;
        }
        names.add(name);
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
  return {
    id: draft.id,
    familyId: draft.familyId,
    routeKey: draft.routeKey.trim(),
    name: draft.name.trim(),
    websiteUrl: draft.websiteUrl.trim() || undefined,
    notes: draft.notes.trim() || undefined,
    icon: draft.icon,
    iconColor: draft.iconColor,
    surfaces: draft.surfaces.map((surface) =>
      surfaceWriteDraft(draft, family, surface),
    ),
    credentialPatches: secretPatches(draft.secrets),
    expectedRevision: draft.expectedRevision,
    clientRequestId: draft.clientRequestId,
  };
}

export function familyCredentialSlots(
  family: ProviderFamilySpec,
): Array<{ logical: string; pointer: string }> {
  return credentialSlotsForFamily(family);
}
