import registrySnapshot from "../../../assets/contract/provider-registry.json";

export type CoreProviderApp = "claude" | "codex" | "gemini";
export type ProviderModelPolicy = "passthrough" | "single";
export type ProviderFieldScope = "bundle" | "surface";

export type ProviderFormComposition =
  | "managed_account"
  | "static_secret"
  | "aws"
  | "custom"
  | "legacy";

export type ProviderUpstreamProtocol =
  | "anthropic_messages"
  | "open_ai_chat"
  | "open_ai_responses"
  | "gemini_native"
  | "bedrock"
  | "special"
  | "custom"
  | "legacy";

export type ProviderAuthScheme =
  | "none"
  | "api_key"
  | "bearer"
  | "oauth"
  | "aws_sig_v4"
  | "custom_header"
  | "query";

export type ProviderOutboundIdentityPolicy =
  | {
      kind: "managed_identity";
      family:
        | "claude_code"
        | "codex_cli"
        | "gemini_cli"
        | "grok_cli"
        | "kiro"
        | "cursor"
        | "copilot"
        | "deepseek";
    }
  | { kind: "managed_version"; family: "antigravity" }
  | {
      kind:
        | "server_identity"
        | "omit"
        | "custom_override"
        | "legacy_frozen";
    };

export type ProviderCredentialPolicy =
  | { mode: "managed_account"; accountProviderType: string }
  | { mode: "static_secret"; slots: string[]; authScheme: ProviderAuthScheme }
  | { mode: "aws"; slots: string[] }
  | { mode: "custom" }
  | { mode: "legacy" };

export type ProviderDriverBinding =
  | { kind: "fixed"; driverId: string }
  | { kind: "custom"; customPolicyId: string };

export interface ProviderRegistryProfile {
  profileId: string;
  profileSchemaRevision: number;
  app: CoreProviderApp;
  label: string;
  driverBinding: ProviderDriverBinding;
  compatibilityProviderType?: string;
  formComposition: ProviderFormComposition;
  endpointPolicy:
    | "fixed"
    | "override_allowed"
    | "template"
    | "custom"
    | "frozen_legacy";
  credentialPolicy: ProviderCredentialPolicy;
  modelPolicy: ProviderModelPolicy;
  allowedModelPolicies?: ProviderModelPolicy[];
  defaultUpstreamModel?: string;
  visibility: "visible" | "hidden";
  creationPolicy: "create_allowed" | "existing_only";
  maturity: "stable" | "experimental";
}

export interface ProviderFamilySurfaceSpec {
  app: CoreProviderApp;
  profileId: string;
  defaultEnabled: boolean;
}

export interface ProviderFamilySpec {
  familyId: string;
  label: string;
  credentialProfileId: string;
  endpointScope: ProviderFieldScope;
  headersScope: ProviderFieldScope;
  driverOptionsScope: ProviderFieldScope;
  surfaces: ProviderFamilySurfaceSpec[];
}

export interface ProviderRegistryDriver {
  driverId: string;
  driverContractRevision: number;
  upstreamProtocol: ProviderUpstreamProtocol;
  acceptedAuthSchemes: ProviderAuthScheme[];
  operations: Record<
    "forward" | "test" | "discovery" | "connectivity",
    "supported" | "unsupported"
  >;
  capabilities: { stream: boolean; tools: boolean; images: boolean };
  outboundIdentityPolicy: ProviderOutboundIdentityPolicy;
  optionSchemaId: string;
}

export type ProviderDriverOptionField =
  | "apiKeyField"
  | "customUserAgent"
  | "codexFastMode"
  | "codexImageGenerationEnabled"
  | "codexWebsocketEnabled";

export interface ProviderDriverOptionSchema {
  optionSchemaId: string;
  fields: ProviderDriverOptionField[];
}

export interface ProviderCustomPolicy {
  customPolicyId: string;
  app: CoreProviderApp;
  protocols: ProviderUpstreamProtocol[];
  authSchemes: ProviderAuthScheme[];
  allowedDriverIds: string[];
  outboundIdentityPolicy: ProviderOutboundIdentityPolicy;
}

export interface ProviderRegistrySnapshot {
  format: "cc-switch-provider-registry";
  schemaVersion: number;
  families: ProviderFamilySpec[];
  profiles: ProviderRegistryProfile[];
  drivers: ProviderRegistryDriver[];
  optionSchemas: ProviderDriverOptionSchema[];
  customPolicies: ProviderCustomPolicy[];
  legacyPresetMappings: Array<{
    app: CoreProviderApp;
    legacyName: string;
    profileId: string;
  }>;
}

export const providerRegistry = registrySnapshot as ProviderRegistrySnapshot;

export function isCoreProviderApp(app: string): app is CoreProviderApp {
  return app === "claude" || app === "codex" || app === "gemini";
}

export function profileIdForLegacyPreset(
  app: CoreProviderApp,
  legacyName: string,
): string {
  const mapping = providerRegistry.legacyPresetMappings.find(
    (item) => item.app === app && item.legacyName === legacyName,
  );
  if (!mapping) {
    throw new Error(`Provider preset ${app}:${legacyName} has no registry profile`);
  }
  return mapping.profileId;
}

export function legacyPresetNameForProfile(
  app: CoreProviderApp,
  profileId: string,
): string | undefined {
  return providerRegistry.legacyPresetMappings.find(
    (item) => item.app === app && item.profileId === profileId,
  )?.legacyName;
}

export function customProfileId(app: CoreProviderApp): string {
  return `${app}.custom_http`;
}

export function profileById(
  profileId: string,
): ProviderRegistryProfile | undefined {
  return providerRegistry.profiles.find(
    (profile) => profile.profileId === profileId,
  );
}

export function familyById(familyId: string): ProviderFamilySpec | undefined {
  return providerRegistry.families.find((family) => family.familyId === familyId);
}

export function familyForProfile(
  profileId: string,
): ProviderFamilySpec | undefined {
  return providerRegistry.families.find((family) =>
    family.surfaces.some((surface) => surface.profileId === profileId),
  );
}

export function modelPoliciesForProfile(
  profile: ProviderRegistryProfile,
): ProviderModelPolicy[] {
  return profile.allowedModelPolicies?.length
    ? profile.allowedModelPolicies
    : [profile.modelPolicy];
}

export function driverForProfile(
  profile: ProviderRegistryProfile,
): ProviderRegistryDriver | undefined {
  const binding = profile.driverBinding;
  if (binding.kind !== "fixed") return undefined;
  return providerRegistry.drivers.find(
    (driver) => driver.driverId === binding.driverId,
  );
}

export function optionSchemaForDriver(
  driver: ProviderRegistryDriver,
): ProviderDriverOptionSchema | undefined {
  return providerRegistry.optionSchemas.find(
    (schema) => schema.optionSchemaId === driver.optionSchemaId,
  );
}

export function customPolicyForProfile(
  profile: ProviderRegistryProfile,
): ProviderCustomPolicy | undefined {
  const binding = profile.driverBinding;
  if (binding.kind !== "custom") return undefined;
  return providerRegistry.customPolicies.find(
    (policy) => policy.customPolicyId === binding.customPolicyId,
  );
}
