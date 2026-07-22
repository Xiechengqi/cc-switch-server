import { useEffect, useMemo, useRef, useState } from "react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  ArrowLeft,
  Copy,
  Gauge,
  KeyRound,
  LoaderCircle,
  Plus,
  RotateCcw,
  Save,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";

import {
  ProviderSharePlaceholder,
  ProviderShareSection,
} from "@/components/providers/ProviderShareSection";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { IconPicker } from "@/components/IconPicker";
import { ProviderIcon } from "@/components/ProviderIcon";
import { AntigravityOAuthSection } from "@/components/providers/forms/AntigravityOAuthSection";
import { ClaudeOAuthSection } from "@/components/providers/forms/ClaudeOAuthSection";
import { CodexOAuthSection } from "@/components/providers/forms/CodexOAuthSection";
import { CopilotAuthSection } from "@/components/providers/forms/CopilotAuthSection";
import { CursorOAuthSection } from "@/components/providers/forms/CursorOAuthSection";
import { DeepSeekAccountSection } from "@/components/providers/forms/DeepSeekAccountSection";
import { GeminiOAuthSection } from "@/components/providers/forms/GeminiOAuthSection";
import { GrokOAuthSection } from "@/components/providers/forms/GrokOAuthSection";
import { KiroOAuthSection } from "@/components/providers/forms/KiroOAuthSection";
import {
  ProviderPresetSelector,
  type PresetEntry,
} from "@/components/providers/forms/ProviderPresetSelector";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import type {
  ProviderCredentialPatch,
  ProviderCredentialPatches,
  ProviderCustomBinding,
  ProviderResource,
} from "@/lib/api/providers";
import { providersApi } from "@/lib/api/providers";
import { vscodeApi } from "@/lib/api/vscode";
import { copyText } from "@/lib/clipboard";
import { getIconMetadata } from "@/icons/extracted/metadata";
import { stableStringify } from "@/lib/stableStringify";
import {
  customPolicyForProfile,
  driverForProfile,
  providerRegistry,
  profileById,
  type CoreProviderApp,
  type ProviderAuthScheme,
  type ProviderRegistryProfile,
  type ProviderUpstreamProtocol,
} from "@/server/providerRegistry";
import { SecretInput } from "@/server/ui/SecretInput";
import type { ProviderMeta } from "@/types";
import { setCodexBaseUrl } from "@/utils/providerConfigUtils";
import {
  createDraftForProfile,
  defaultSingleModel,
  ensureObject,
  providerPresetForProfile,
  readEndpoint,
  readUpstreamModel,
  setEndpoint,
  setPassthroughModel,
  setSingleModel,
  type CoreProviderDraft,
} from "./providerDraft";
import {
  credentialInputValue,
  updateCredentialInput,
  type CredentialEdit,
  type CredentialRevealStatus,
} from "./credentialEditing";

const KEEP_SENTINEL = "__CC_SWITCH_SECRET_KEEP__";
const PRIMARY_CREDENTIAL_SLOT = "/settingsConfig/apiKey";
const EXTRA_HEADER_PREFIX = "/settingsConfig/extraHeaders/";

interface ExtraHeaderEdit {
  id: string;
  name: string;
  originalName?: string;
  originalSlot?: string;
  configured: boolean;
  action: "keep" | "replace";
  value: string;
  removed: boolean;
}

interface EditorState {
  profileId: string;
  draft: CoreProviderDraft;
  endpoint: string;
  upstreamModel: string;
  accountId: string;
  awsRegion: string;
  customBinding?: ProviderCustomBinding;
  credentials: Record<string, CredentialEdit>;
  extraHeaders: ExtraHeaderEdit[];
  costMultiplier: string;
  pricingModelSource: "inherit" | "request" | "response";
  quotaDispatchLimitPercent: string;
  customUserAgent: string;
  codexFastMode: boolean;
  codexImageGenerationEnabled: boolean;
  codexWebsocketEnabled: boolean;
}

interface PendingIdentityAction {
  kind: "adopt" | "rebind" | "clone";
  previewToken: string;
  title: string;
  message: string;
  cloneDraft?: CloneAsCustomDraft;
}

interface EndpointProbeState {
  url: string;
  pending: boolean;
  latency?: number;
  status?: number;
  error?: string;
}

interface CloneAsCustomDraft {
  targetProviderId: string;
  targetName: string;
  customBinding: ProviderCustomBinding;
  clientRequestId: string;
}

interface InitialProviderData {
  name?: string;
  websiteUrl?: string;
  notes?: string;
  settingsConfig?: Record<string, unknown>;
  category?: CoreProviderDraft["category"];
  meta?: ProviderMeta;
  icon?: string;
  iconColor?: string;
}

export interface ServerProviderFormValues {
  name: string;
  websiteUrl?: string;
  notes?: string;
  settingsConfig: string;
  icon?: string;
  iconColor?: string;
  profileId?: string;
  customBinding?: ProviderCustomBinding;
  credentialPatches?: ProviderCredentialPatches;
  presetCategory?: CoreProviderDraft["category"];
  meta?: ProviderMeta;
}

interface ServerProviderFormProps {
  appId: CoreProviderApp;
  providerId?: string;
  resource?: ProviderResource;
  submitLabel: string;
  onSubmit: (values: ServerProviderFormValues) => Promise<void> | void;
  onCancel: () => void;
  onSubmittingChange?: (isSubmitting: boolean) => void;
  onDirtyChange?: (dirty: boolean) => void;
  onUnsavedChange?: (dirty: boolean) => void;
  onSubmitBlockedChange?: (blocked: boolean) => void;
  initialData?: InitialProviderData;
  showButtons?: boolean;
  onOpenShareSettings?: () => void;
}

const CUSTOM_DEFAULT_BINDINGS: Record<CoreProviderApp, ProviderCustomBinding> =
  {
    claude: { upstreamProtocol: "anthropic_messages", authScheme: "api_key" },
    codex: { upstreamProtocol: "open_ai_responses", authScheme: "bearer" },
    gemini: { upstreamProtocol: "gemini_native", authScheme: "api_key" },
  };

const AWS_SLOTS = {
  accessKeyId: "/settingsConfig/env/AWS_ACCESS_KEY_ID",
  secretAccessKey: "/settingsConfig/env/AWS_SECRET_ACCESS_KEY",
  sessionToken: "/settingsConfig/env/AWS_SESSION_TOKEN",
} as const;

const HEADER_DENYLIST = new Set([
  "authorization",
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
  "x-api-key",
  "api-key",
  "x-goog-api-key",
]);

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function escapePointerSegment(value: string): string {
  return value.replace(/~/g, "~0").replace(/\//g, "~1");
}

function unescapePointerSegment(value: string): string {
  return value.replace(/~1/g, "/").replace(/~0/g, "~");
}

function extraHeaderSlot(name: string): string {
  return `${EXTRA_HEADER_PREFIX}${escapePointerSegment(name)}`;
}

function profileList(app: CoreProviderApp): ProviderRegistryProfile[] {
  return providerRegistry.profiles.filter(
    (profile) =>
      profile.app === app &&
      profile.visibility === "visible" &&
      profile.creationPolicy === "create_allowed",
  );
}

function isLockedPresetProfile(profile: ProviderRegistryProfile): boolean {
  return (
    profile.formComposition !== "custom" && profile.formComposition !== "legacy"
  );
}

function canonicalPresetEndpoint(
  profile: ProviderRegistryProfile,
  app: CoreProviderApp,
  awsRegion = "us-east-1",
): string {
  const endpoint = readEndpoint(
    createDraftForProfile(profile).settingsConfig,
    app,
  );
  if (profile.formComposition !== "aws") return endpoint;
  return endpoint.split("us-east-1").join(awsRegion.trim() || "us-east-1");
}

function presetEntriesForApp(app: CoreProviderApp): PresetEntry[] {
  return profileList(app)
    .filter((profile) => profile.formComposition !== "custom")
    .map((profile) => {
      const preset = providerPresetForProfile(profile);
      if (!preset) {
        throw new Error(`Provider preset ${profile.profileId} is unavailable`);
      }
      return { id: profile.profileId, preset };
    });
}

const PRESET_CATEGORY_KEYS: Record<string, string> = {
  official: "serverProviderForm.categories.official",
  cn_official: "serverProviderForm.categories.cnOfficial",
  aggregator: "serverProviderForm.categories.aggregator",
  third_party: "serverProviderForm.categories.thirdParty",
  custom: "serverProviderForm.categories.custom",
};

function actualSlot(
  resource: ProviderResource | undefined,
  canonical: string,
  suffix?: string,
): string {
  const match = resource?.credentialSlots.find(
    (slot) => slot === canonical || (suffix ? slot.endsWith(suffix) : false),
  );
  return match ?? canonical;
}

function credentialEdit(
  slot: string,
  resource: ProviderResource | undefined,
  isEditMode: boolean,
): CredentialEdit {
  const configured = resource?.credentialSlots.includes(slot) ?? false;
  return {
    slot,
    configured,
    action: isEditMode && configured ? "keep" : "replace",
    value: "",
  };
}

function buildCredentialEdits(
  profile: ProviderRegistryProfile,
  resource: ProviderResource | undefined,
  isEditMode: boolean,
): Record<string, CredentialEdit> {
  if (profile.formComposition === "aws") {
    const access = actualSlot(
      resource,
      AWS_SLOTS.accessKeyId,
      "/AWS_ACCESS_KEY_ID",
    );
    const secret = actualSlot(
      resource,
      AWS_SLOTS.secretAccessKey,
      "/AWS_SECRET_ACCESS_KEY",
    );
    const session = actualSlot(
      resource,
      AWS_SLOTS.sessionToken,
      "/AWS_SESSION_TOKEN",
    );
    const sessionToken = credentialEdit(session, resource, isEditMode);
    return {
      accessKeyId: credentialEdit(access, resource, isEditMode),
      secretAccessKey: credentialEdit(secret, resource, isEditMode),
      sessionToken,
    };
  }
  if (
    profile.formComposition === "static_secret" ||
    profile.formComposition === "custom"
  ) {
    const existing = resource?.credentialSlots.find(
      (slot) => !slot.startsWith(EXTRA_HEADER_PREFIX),
    );
    const slot = existing ?? PRIMARY_CREDENTIAL_SLOT;
    return { primary: credentialEdit(slot, resource, isEditMode) };
  }
  return {};
}

function buildExtraHeaderEdits(
  settings: Record<string, unknown>,
  resource: ProviderResource | undefined,
): ExtraHeaderEdit[] {
  const configuredSlots = new Set(resource?.credentialSlots ?? []);
  const headers = settings.extraHeaders;
  if (!headers || typeof headers !== "object" || Array.isArray(headers))
    return [];
  return Object.entries(headers as Record<string, unknown>).map(
    ([name, value]) => {
      const slot = extraHeaderSlot(name);
      const configured = configuredSlots.has(slot) || value === KEEP_SENTINEL;
      return {
        id: crypto.randomUUID(),
        name,
        originalName: name,
        originalSlot: slot,
        configured,
        action: configured ? "keep" : "replace",
        value: "",
        removed: false,
      };
    },
  );
}

function initialProfile(
  app: CoreProviderApp,
  resource: ProviderResource | undefined,
  isEditMode: boolean,
): ProviderRegistryProfile {
  const profileId =
    resource?.profileId ??
    (isEditMode ? `${app}.legacy_compat` : profileList(app)[0]?.profileId);
  const profile = profileId ? profileById(profileId) : undefined;
  if (!profile || profile.app !== app) {
    throw new Error(
      `Provider profile ${profileId ?? "unknown"} is unavailable`,
    );
  }
  return profile;
}

function buildEditorState(
  app: CoreProviderApp,
  resource: ProviderResource | undefined,
  initialData: InitialProviderData | undefined,
  profileOverride?: ProviderRegistryProfile,
): EditorState {
  const profile =
    profileOverride ?? initialProfile(app, resource, Boolean(initialData));
  const isEditMode = Boolean(resource && initialData);
  const draft = initialData
    ? {
        name: initialData.name ?? "",
        websiteUrl: initialData.websiteUrl ?? "",
        notes: initialData.notes ?? "",
        settingsConfig: clone(initialData.settingsConfig ?? {}),
        category: initialData.category,
        meta: clone(initialData.meta ?? {}),
        icon: initialData.icon,
        iconColor: initialData.iconColor,
      }
    : createDraftForProfile(profile);
  ensureObject(draft.settingsConfig, "env");
  const awsRegion =
    String(
      (draft.settingsConfig.env as Record<string, unknown> | undefined)
        ?.AWS_REGION ?? "us-east-1",
    ).trim() || "us-east-1";
  if (isLockedPresetProfile(profile)) {
    const presetDraft = createDraftForProfile(profile);
    const presetEndpoint = canonicalPresetEndpoint(profile, app, awsRegion);
    draft.name = presetDraft.name;
    draft.websiteUrl = presetDraft.websiteUrl;
    setEndpoint(draft.settingsConfig, app, presetEndpoint);
    if (app === "codex" && typeof draft.settingsConfig.config === "string") {
      draft.settingsConfig.config = setCodexBaseUrl(
        draft.settingsConfig.config,
        presetEndpoint,
      );
    }
  }
  const customBinding =
    profile.formComposition === "custom"
      ? (resource?.customBinding ?? clone(CUSTOM_DEFAULT_BINDINGS[app]))
      : undefined;

  return {
    profileId: profile.profileId,
    draft,
    endpoint: readEndpoint(draft.settingsConfig, app),
    upstreamModel:
      readUpstreamModel(draft.settingsConfig) ??
      defaultSingleModel(profile.profileId),
    accountId: draft.meta.authBinding?.accountId ?? "",
    awsRegion,
    customBinding,
    credentials: buildCredentialEdits(profile, resource, isEditMode),
    extraHeaders: buildExtraHeaderEdits(draft.settingsConfig, resource),
    costMultiplier: draft.meta.costMultiplier ?? "",
    pricingModelSource:
      draft.meta.pricingModelSource === "request" ||
      draft.meta.pricingModelSource === "response"
        ? draft.meta.pricingModelSource
        : "inherit",
    quotaDispatchLimitPercent:
      draft.meta.quotaDispatchLimitPercent == null
        ? ""
        : String(draft.meta.quotaDispatchLimitPercent),
    customUserAgent: draft.meta.customUserAgent ?? "",
    codexFastMode: draft.meta.codexFastMode ?? false,
    codexImageGenerationEnabled:
      draft.meta.codexImageGenerationEnabled ?? false,
    codexWebsocketEnabled: draft.meta.codexWebsocketEnabled ?? true,
  };
}

function isValidEndpoint(value: string): boolean {
  try {
    const url = new URL(value);
    return (
      (url.protocol === "http:" || url.protocol === "https:") &&
      !url.username &&
      !url.password
    );
  } catch {
    return false;
  }
}

function isValidHeaderName(value: string): boolean {
  return /^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(value);
}

function providerMetaForSubmit(
  state: EditorState,
  profile: ProviderRegistryProfile,
): ProviderMeta {
  const meta = clone(state.draft.meta);
  if (profile.formComposition === "legacy") return meta;
  meta.providerType = profile.compatibilityProviderType;
  meta.costMultiplier = state.costMultiplier.trim() || undefined;
  meta.pricingModelSource =
    state.pricingModelSource === "inherit"
      ? undefined
      : state.pricingModelSource;
  const quota = Number.parseInt(state.quotaDispatchLimitPercent, 10);
  meta.quotaDispatchLimitPercent =
    Number.isInteger(quota) && quota >= 1 && quota <= 100 ? quota : undefined;
  meta.customUserAgent = state.customUserAgent.trim() || undefined;

  if (profile.credentialPolicy.mode === "managed_account") {
    meta.authBinding = {
      source: "managed_account",
      authProvider: profile.credentialPolicy.accountProviderType,
      accountId: state.accountId,
    };
  } else {
    delete meta.authBinding;
  }

  const driver = driverForProfile(profile);
  const protocol =
    profile.formComposition === "custom"
      ? state.customBinding?.upstreamProtocol
      : driver?.upstreamProtocol;
  meta.apiFormat =
    protocol === "anthropic_messages"
      ? "anthropic"
      : protocol === "open_ai_chat"
        ? "openai_chat"
        : protocol === "open_ai_responses"
          ? "openai_responses"
          : protocol === "gemini_native"
            ? "gemini_native"
            : undefined;

  if (profile.formComposition === "custom") {
    const authScheme = state.customBinding?.authScheme;
    if (authScheme !== "custom_header" && authScheme !== "query") {
      delete meta.apiKeyField;
    }
  }
  if (driver?.driverId === "oauth.openai_codex") {
    meta.codexFastMode = state.codexFastMode;
    meta.codexImageGenerationEnabled = state.codexImageGenerationEnabled;
    meta.codexWebsocketEnabled = state.codexWebsocketEnabled;
  } else {
    delete meta.codexFastMode;
    delete meta.codexImageGenerationEnabled;
    delete meta.codexWebsocketEnabled;
  }
  return meta;
}

function collectPrimaryCredentialPatches(
  state: EditorState,
): ProviderCredentialPatches {
  const patches: ProviderCredentialPatches = {};
  for (const edit of Object.values(state.credentials)) {
    if (edit.action === "replace") {
      if (!edit.configured && !edit.value.trim()) continue;
      patches[edit.slot] = { action: "replace", value: edit.value.trim() };
    } else if (edit.action === "clear") {
      patches[edit.slot] = { action: "clear" };
    } else if (edit.configured) {
      patches[edit.slot] = { action: "keep" };
    }
  }
  return patches;
}

function collectCredentialPatches(
  state: EditorState,
): ProviderCredentialPatches {
  const patches = collectPrimaryCredentialPatches(state);
  for (const header of state.extraHeaders) {
    if (header.removed) {
      if (header.originalSlot)
        patches[header.originalSlot] = { action: "clear" };
      continue;
    }
    const slot = extraHeaderSlot(header.name.trim());
    if (header.originalSlot && header.originalSlot !== slot) {
      patches[header.originalSlot] = { action: "clear" };
    }
    if (header.action === "replace") {
      patches[slot] = { action: "replace", value: header.value.trim() };
    } else if (header.configured) {
      patches[slot] = { action: "keep" };
    }
  }
  return patches;
}

function prepareSettingsForSubmit(
  state: EditorState,
  profile: ProviderRegistryProfile,
  app: CoreProviderApp,
): Record<string, unknown> {
  const settings = clone(state.draft.settingsConfig);
  if (profile.formComposition === "legacy") return settings;
  ensureObject(settings, "env");
  if (
    profile.endpointPolicy === "custom" ||
    profile.endpointPolicy === "override_allowed"
  ) {
    setEndpoint(settings, app, state.endpoint);
  }
  if (isLockedPresetProfile(profile)) {
    const endpoint = canonicalPresetEndpoint(profile, app, state.awsRegion);
    setEndpoint(settings, app, endpoint);
    if (app === "codex" && typeof settings.config === "string") {
      settings.config = setCodexBaseUrl(settings.config, endpoint);
    }
  }
  if (profile.formComposition === "aws") {
    ensureObject(settings, "env").AWS_REGION = state.awsRegion.trim();
  }
  if (profile.modelPolicy === "single") {
    setSingleModel(settings, app, state.upstreamModel);
  } else {
    setPassthroughModel(settings);
  }
  if (profile.formComposition === "custom") {
    const headers: Record<string, string> = {};
    for (const header of state.extraHeaders.filter((item) => !item.removed)) {
      headers[header.name.trim()] =
        header.action === "keep" && header.configured ? KEEP_SENTINEL : "";
    }
    settings.extraHeaders = headers;
  }
  return settings;
}

function validateState(
  state: EditorState,
  profile: ProviderRegistryProfile,
  t: TFunction,
): string | null {
  if (!state.draft.name.trim()) {
    return t("serverProviderForm.validation.nameRequired");
  }
  if (profile.formComposition === "legacy") return null;
  if (profile.credentialPolicy.mode === "managed_account" && !state.accountId) {
    return t("serverProviderForm.validation.accountRequired");
  }
  if (
    (profile.endpointPolicy === "custom" ||
      profile.endpointPolicy === "override_allowed") &&
    !isValidEndpoint(state.endpoint)
  ) {
    return t("serverProviderForm.validation.endpointInvalid");
  }
  if (profile.modelPolicy === "single" && !state.upstreamModel.trim()) {
    return t("serverProviderForm.validation.modelRequired");
  }
  if (profile.formComposition === "aws" && !state.awsRegion.trim()) {
    return t("serverProviderForm.validation.awsRegionRequired");
  }
  for (const [name, edit] of Object.entries(state.credentials)) {
    const required = name !== "sessionToken";
    if (edit.action === "replace" && required && !edit.value.trim()) {
      return t("serverProviderForm.validation.replacementRequired");
    }
    if (edit.action === "clear" && required) {
      return t("serverProviderForm.validation.requiredCredentialCannotClear");
    }
  }
  if (profile.formComposition === "custom") {
    if (!state.customBinding) {
      return t("serverProviderForm.validation.customBindingMissing");
    }
    const customPolicy = customPolicyForProfile(profile);
    if (
      !customPolicy?.protocols.includes(
        state.customBinding.upstreamProtocol as ProviderUpstreamProtocol,
      ) ||
      !customPolicy.authSchemes.includes(
        state.customBinding.authScheme as ProviderAuthScheme,
      )
    ) {
      return t("serverProviderForm.validation.unsupportedBinding");
    }
    const authRequired = true;
    const primary = state.credentials.primary;
    if (
      authRequired &&
      primary?.action === "replace" &&
      !primary.value.trim()
    ) {
      return t("serverProviderForm.validation.authCredentialRequired");
    }
    if (authRequired && primary?.action === "clear") {
      return t("serverProviderForm.validation.authRequired");
    }
    const names = new Set<string>();
    for (const header of state.extraHeaders.filter((item) => !item.removed)) {
      const name = header.name.trim().toLowerCase();
      if (!isValidHeaderName(name)) {
        return t("serverProviderForm.validation.invalidHeader", {
          name: header.name,
        });
      }
      if (HEADER_DENYLIST.has(name)) {
        return t("serverProviderForm.validation.managedHeader", {
          name: header.name,
        });
      }
      if (names.has(name)) {
        return t("serverProviderForm.validation.duplicateHeader", {
          name: header.name,
        });
      }
      names.add(name);
      if (header.action === "replace" && !header.value.trim()) {
        return t("serverProviderForm.validation.headerValueRequired", {
          name: header.name,
        });
      }
    }
  }
  if (state.costMultiplier.trim()) {
    const value = Number(state.costMultiplier);
    if (!Number.isFinite(value) || value < 0) {
      return t("serverProviderForm.validation.costMultiplierInvalid");
    }
  }
  if (state.quotaDispatchLimitPercent.trim()) {
    const value = Number(state.quotaDispatchLimitPercent);
    if (!Number.isInteger(value) || value < 1 || value > 100) {
      return t("serverProviderForm.validation.quotaLimitInvalid");
    }
  }
  return null;
}

function buildCloneDraft(
  app: CoreProviderApp,
  resource: ProviderResource | undefined,
): CloneAsCustomDraft {
  const sourceId = resource?.provider.id ?? "provider";
  const sourceName = resource?.provider.name ?? "Provider";
  return {
    targetProviderId: `${sourceId}-custom`,
    targetName: `${sourceName} Custom`,
    customBinding: clone(CUSTOM_DEFAULT_BINDINGS[app]),
    clientRequestId: crypto.randomUUID(),
  };
}

function cloneDraftFingerprint(draft: CloneAsCustomDraft): string {
  return stableStringify({
    targetProviderId: draft.targetProviderId,
    targetName: draft.targetName,
    customBinding: draft.customBinding,
  });
}

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="space-y-4 border-b border-border/50 pb-6 last:border-b-0 last:pb-0">
      <h3 className="text-sm font-semibold text-foreground">{title}</h3>
      {children}
    </section>
  );
}

function ProviderIconControl({
  icon,
  iconColor,
  providerName,
  onChange,
}: {
  icon?: string;
  iconColor?: string;
  providerName: string;
  onChange: (icon: string, iconColor?: string) => void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);

  const selectIcon = (nextIcon: string) => {
    onChange(nextIcon, getIconMetadata(nextIcon)?.defaultColor);
  };

  return (
    <div className="flex justify-center">
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogTrigger asChild>
          <button
            type="button"
            className="flex h-20 w-20 items-center justify-center rounded-lg border-2 border-muted bg-muted/30 p-3 transition-colors hover:border-primary hover:bg-muted/50"
            title={
              icon
                ? t("providerIcon.clickToChange")
                : t("providerIcon.clickToSelect")
            }
            aria-label={
              icon
                ? t("providerIcon.clickToChange")
                : t("providerIcon.clickToSelect")
            }
          >
            <ProviderIcon
              icon={icon}
              name={providerName || "Provider"}
              color={iconColor}
              size={48}
            />
          </button>
        </DialogTrigger>
        <DialogContent
          variant="fullscreen"
          zIndex="top"
          overlayClassName="bg-[hsl(var(--background))] backdrop-blur-0"
          className="p-0 sm:rounded-none"
        >
          <div className="flex h-full flex-col">
            <div className="flex shrink-0 items-center gap-4 border-b border-border-default bg-muted/40 px-6 py-4">
              <DialogClose asChild>
                <Button
                  type="button"
                  variant="outline"
                  size="icon"
                  title={t("common.back")}
                  aria-label={t("common.back")}
                >
                  <ArrowLeft className="h-4 w-4" />
                </Button>
              </DialogClose>
              <DialogTitle>{t("providerIcon.selectIcon")}</DialogTitle>
            </div>
            <div className="flex-1 overflow-y-auto px-6 py-6">
              <div className="space-y-4">
                <IconPicker
                  value={icon}
                  onValueChange={selectIcon}
                  color={iconColor}
                />
                <div className="flex justify-end">
                  <DialogClose asChild>
                    <Button type="button" variant="outline">
                      {t("common.done")}
                    </Button>
                  </DialogClose>
                </div>
              </div>
            </div>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function CredentialControl({
  label,
  edit,
  optional = false,
  revealedValue,
  revealStatus = "idle",
  onRetryReveal,
  onChange,
}: {
  label: string;
  edit: CredentialEdit;
  optional?: boolean;
  revealedValue?: string;
  revealStatus?: CredentialRevealStatus;
  onRetryReveal?: () => void;
  onChange: (next: CredentialEdit) => void;
}) {
  const { t } = useTranslation();
  const value = credentialInputValue(edit, revealedValue);
  const loadingCurrent =
    edit.configured && edit.action === "keep" && revealStatus === "loading";
  const currentRevealFailed =
    edit.configured && edit.action === "keep" && revealStatus === "error";

  const updateValue = (nextValue: string) => {
    onChange(
      updateCredentialInput(edit, nextValue, {
        optional,
        revealedValue,
        revealStatus,
      }),
    );
  };

  const handleCopy = async () => {
    if (!value) return;
    try {
      await copyText(value);
      toast.success(t("common.copied"));
    } catch (error) {
      toast.error(String(error));
    }
  };

  return (
    <div className="space-y-2">
      <Label>{label}</Label>
      <div className="flex items-center gap-2">
        <div className="min-w-0 flex-1">
          <SecretInput
            value={value}
            disabled={loadingCurrent || edit.action === "clear"}
            onChange={(event) => updateValue(event.target.value)}
            autoComplete="new-password"
            placeholder={
              loadingCurrent
                ? t("serverProviderForm.credentials.loading")
                : currentRevealFailed
                  ? t("serverProviderForm.credentials.loadFailedPlaceholder")
                  : edit.action === "clear"
                    ? t("serverProviderForm.credentials.willClear")
                    : t("serverProviderForm.credentials.placeholder")
            }
          />
        </div>
        <Button
          type="button"
          size="icon"
          variant="outline"
          disabled={!value || loadingCurrent || edit.action === "clear"}
          title={t("common.copy")}
          aria-label={t("common.copy")}
          onClick={() => void handleCopy()}
        >
          {loadingCurrent ? (
            <LoaderCircle className="h-4 w-4 animate-spin" />
          ) : (
            <Copy className="h-4 w-4" />
          )}
        </Button>
        {optional && edit.configured ? (
          <Button
            type="button"
            size="icon"
            variant="outline"
            title={
              edit.action === "clear"
                ? t("common.undo")
                : t("serverProviderForm.credentials.clear")
            }
            aria-label={
              edit.action === "clear"
                ? t("common.undo")
                : t("serverProviderForm.credentials.clear")
            }
            onClick={() =>
              onChange({
                ...edit,
                action: edit.action === "clear" ? "keep" : "clear",
                value: "",
              })
            }
          >
            {edit.action === "clear" ? (
              <RotateCcw className="h-4 w-4" />
            ) : (
              <Trash2 className="h-4 w-4" />
            )}
          </Button>
        ) : null}
        {currentRevealFailed && onRetryReveal ? (
          <Button
            type="button"
            size="sm"
            variant="outline"
            onClick={onRetryReveal}
          >
            {t("common.retry")}
          </Button>
        ) : null}
      </div>
      <div className="text-xs text-muted-foreground">
        {loadingCurrent
          ? t("serverProviderForm.credentials.loading")
          : currentRevealFailed
            ? t("serverProviderForm.credentials.loadFailed")
            : edit.action === "clear"
              ? t("serverProviderForm.credentials.willClear")
              : edit.action === "replace" && edit.configured
                ? t("serverProviderForm.credentials.willReplace")
                : edit.configured
                  ? t("serverProviderForm.credentials.configured")
                  : optional
                    ? t("serverProviderForm.credentials.optionalMissing")
                    : t("serverProviderForm.credentials.missing")}
      </div>
    </div>
  );
}

function ManagedAccountSection({
  providerType,
  accountId,
  onAccountSelect,
}: {
  providerType: string;
  accountId: string;
  onAccountSelect: (accountId: string | null) => void;
}) {
  const { t } = useTranslation();
  const common = {
    selectedAccountId: accountId || null,
    onAccountSelect,
    showLoggedInAccounts: false,
  };
  switch (providerType) {
    case "claude_oauth":
      return <ClaudeOAuthSection {...common} />;
    case "codex_oauth":
      return (
        <CodexOAuthSection {...common} allowDefaultAccountOption={false} />
      );
    case "grok_oauth":
      return <GrokOAuthSection {...common} allowDefaultAccountOption={false} />;
    case "github_copilot":
      return <CopilotAuthSection {...common} />;
    case "gemini_cli":
      return (
        <GeminiOAuthSection {...common} allowDefaultAccountOption={false} />
      );
    case "antigravity_oauth":
      return (
        <AntigravityOAuthSection {...common} authProvider="antigravity_oauth" />
      );
    case "agy_oauth":
      return <AntigravityOAuthSection {...common} authProvider="agy_oauth" />;
    case "cursor_oauth":
      return <CursorOAuthSection {...common} />;
    case "kiro_oauth":
      return <KiroOAuthSection {...common} />;
    case "deepseek_account":
      return <DeepSeekAccountSection {...common} />;
    default:
      return (
        <div className="flex items-center gap-2 text-sm text-destructive">
          <AlertTriangle className="h-4 w-4" />
          {t("serverProviderForm.unsupportedAccountType", {
            type: providerType,
          })}
        </div>
      );
  }
}

export function ServerProviderForm({
  appId,
  providerId,
  resource,
  submitLabel,
  onSubmit,
  onCancel,
  onSubmittingChange,
  onDirtyChange,
  onUnsavedChange,
  onSubmitBlockedChange,
  initialData,
  showButtons = true,
  onOpenShareSettings,
}: ServerProviderFormProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const isEditMode = Boolean(initialData && providerId);
  const initializationKey = `${appId}:${providerId ?? "new"}:${resource?.revision ?? 0}`;
  const initial = useMemo(
    () => buildEditorState(appId, resource, initialData),
    // The explicit key prevents query refreshes from replacing an active draft.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [initializationKey],
  );
  const presetEntries = useMemo(() => presetEntriesForApp(appId), [appId]);
  const presetCategoryLabels = Object.fromEntries(
    Object.entries(PRESET_CATEGORY_KEYS).map(([category, key]) => [
      category,
      t(key),
    ]),
  );
  const [state, setState] = useState<EditorState>(initial);
  const [baseline, setBaseline] = useState(() => stableStringify(initial));
  const [shareDirty, setShareDirty] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [identityActionPending, setIdentityActionPending] = useState(false);
  const [rebindEditing, setRebindEditing] = useState(false);
  const [adoptAccountId, setAdoptAccountId] = useState("");
  const [cloneDraft, setCloneDraft] = useState(() =>
    buildCloneDraft(appId, resource),
  );
  const [cloneBaseline, setCloneBaseline] = useState(() =>
    cloneDraftFingerprint(buildCloneDraft(appId, resource)),
  );
  const [pendingIdentityAction, setPendingIdentityAction] =
    useState<PendingIdentityAction | null>(null);
  const [revealedCredentialValues, setRevealedCredentialValues] = useState<
    Record<string, string>
  >({});
  const [credentialRevealStatuses, setCredentialRevealStatuses] = useState<
    Record<string, CredentialRevealStatus>
  >({});
  const [endpointProbe, setEndpointProbe] = useState<EndpointProbeState | null>(
    null,
  );
  const endpointProbeGeneration = useRef(0);
  const credentialRevealGeneration = useRef(0);
  const configuredCredentialSlots = Object.values(initial.credentials)
    .filter((edit) => edit.configured)
    .map((edit) => edit.slot)
    .sort();
  const configuredCredentialSlotsKey = configuredCredentialSlots.join("\n");

  useEffect(() => {
    setState(initial);
    setBaseline(stableStringify(initial));
    setShareDirty(false);
    endpointProbeGeneration.current += 1;
    setEndpointProbe(null);
    setRebindEditing(false);
    setAdoptAccountId("");
    const nextCloneDraft = buildCloneDraft(appId, resource);
    setCloneDraft(nextCloneDraft);
    setCloneBaseline(cloneDraftFingerprint(nextCloneDraft));
    // `initial` already includes the app/provider/revision initialization key.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initial]);

  useEffect(() => {
    const generation = credentialRevealGeneration.current + 1;
    credentialRevealGeneration.current = generation;
    setRevealedCredentialValues({});
    setCredentialRevealStatuses(
      Object.fromEntries(
        configuredCredentialSlots.map((slot) => [slot, "loading" as const]),
      ),
    );

    if (!providerId) return;
    for (const slot of configuredCredentialSlots) {
      void providersApi
        .getCredential(appId, providerId, slot)
        .then((value) => {
          if (credentialRevealGeneration.current !== generation) return;
          setRevealedCredentialValues((current) => ({
            ...current,
            [slot]: value,
          }));
          setCredentialRevealStatuses((current) => ({
            ...current,
            [slot]: "ready",
          }));
        })
        .catch(() => {
          if (credentialRevealGeneration.current !== generation) return;
          setCredentialRevealStatuses((current) => ({
            ...current,
            [slot]: "error",
          }));
        });
    }

    return () => {
      if (credentialRevealGeneration.current === generation) {
        credentialRevealGeneration.current += 1;
      }
    };
    // The serialized slot list changes only when the Provider revision changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [appId, configuredCredentialSlotsKey, initializationKey, providerId]);

  const retryCredentialReveal = async (slot: string) => {
    if (!providerId) return;
    const generation = credentialRevealGeneration.current;
    setCredentialRevealStatuses((current) => ({
      ...current,
      [slot]: "loading",
    }));
    try {
      const value = await providersApi.getCredential(appId, providerId, slot);
      if (credentialRevealGeneration.current !== generation) return;
      setRevealedCredentialValues((current) => ({
        ...current,
        [slot]: value,
      }));
      setCredentialRevealStatuses((current) => ({
        ...current,
        [slot]: "ready",
      }));
    } catch {
      if (credentialRevealGeneration.current !== generation) return;
      setCredentialRevealStatuses((current) => ({
        ...current,
        [slot]: "error",
      }));
    }
  };

  const profile = profileById(state.profileId);
  if (!profile || profile.app !== appId) {
    throw new Error(`Provider profile ${state.profileId} is unavailable`);
  }
  const customPolicy = customPolicyForProfile(profile);
  const cloneCustomProfile = profileById(`${appId}.custom_http`);
  const cloneCustomPolicy = cloneCustomProfile
    ? customPolicyForProfile(cloneCustomProfile)
    : undefined;
  const suggestedProfile = resource?.identity.suggestedProfileId
    ? profileById(resource.identity.suggestedProfileId)
    : undefined;
  const providerDirty = stableStringify(state) !== baseline;
  const dirty = providerDirty;
  const customBindingDirty =
    isEditMode &&
    profile.formComposition === "custom" &&
    stableStringify(state.customBinding) !==
      stableStringify(resource?.customBinding);
  const cloneDirty =
    profile.formComposition === "legacy" &&
    cloneDraftFingerprint(cloneDraft) !== cloneBaseline;

  useEffect(() => {
    onDirtyChange?.(dirty);
  }, [dirty, onDirtyChange]);
  useEffect(() => {
    onUnsavedChange?.(dirty || shareDirty || cloneDirty);
  }, [cloneDirty, dirty, onUnsavedChange, shareDirty]);
  useEffect(() => {
    onSubmitBlockedChange?.(customBindingDirty);
  }, [customBindingDirty, onSubmitBlockedChange]);
  useEffect(
    () => () => {
      onDirtyChange?.(false);
      onUnsavedChange?.(false);
      onSubmitBlockedChange?.(false);
    },
    [onDirtyChange, onSubmitBlockedChange, onUnsavedChange],
  );

  const updateDraft = (patch: Partial<CoreProviderDraft>) => {
    setState((current) => ({
      ...current,
      draft: { ...current.draft, ...patch },
    }));
  };
  const updateMeta = (patch: Partial<ProviderMeta>) => {
    setState((current) => ({
      ...current,
      draft: {
        ...current.draft,
        meta: { ...current.draft.meta, ...patch },
      },
    }));
  };
  const updateCredential = (name: string, next: CredentialEdit) => {
    setState((current) => ({
      ...current,
      credentials: { ...current.credentials, [name]: next },
    }));
  };

  const testEndpoint = async () => {
    const url = state.endpoint.trim();
    if (!isValidEndpoint(url)) return;
    const generation = endpointProbeGeneration.current + 1;
    endpointProbeGeneration.current = generation;
    setEndpointProbe({ url, pending: true });
    try {
      const [result] = await vscodeApi.testApiEndpoints([url], {
        timeoutSecs: 5,
      });
      if (endpointProbeGeneration.current !== generation) return;
      if (!result) {
        setEndpointProbe({
          url,
          pending: false,
          error: t("endpointTest.noResult"),
        });
        return;
      }
      setEndpointProbe({
        url,
        pending: false,
        latency:
          typeof result.latency === "number"
            ? Math.round(result.latency)
            : undefined,
        status: result.status,
        error: result.error,
      });
    } catch (error) {
      if (endpointProbeGeneration.current !== generation) return;
      setEndpointProbe({
        url,
        pending: false,
        error: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const selectProfile = (profileId: string) => {
    const nextProfile = profileById(profileId);
    if (!nextProfile || nextProfile.app !== appId) return;
    endpointProbeGeneration.current += 1;
    setEndpointProbe(null);
    setState(buildEditorState(appId, undefined, undefined, nextProfile));
  };

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (customBindingDirty) {
      toast.error(t("serverProviderForm.toasts.applyBindingFirst"));
      return;
    }
    const validationError = validateState(state, profile, t);
    if (validationError) {
      toast.error(validationError);
      return;
    }
    const settings = prepareSettingsForSubmit(state, profile, appId);
    const credentialPatches = collectCredentialPatches(state);
    const presetDraft = isLockedPresetProfile(profile)
      ? createDraftForProfile(profile)
      : null;
    setSubmitting(true);
    onSubmittingChange?.(true);
    try {
      await onSubmit({
        name: (presetDraft?.name ?? state.draft.name).trim(),
        websiteUrl:
          (presetDraft?.websiteUrl ?? state.draft.websiteUrl).trim() ||
          undefined,
        notes: state.draft.notes.trim() || undefined,
        settingsConfig: JSON.stringify(settings),
        icon: state.draft.icon,
        iconColor: state.draft.iconColor,
        profileId:
          isEditMode && !resource?.profileId ? undefined : profile.profileId,
        customBinding:
          profile.formComposition === "custom"
            ? state.customBinding
            : undefined,
        credentialPatches,
        presetCategory: state.draft.category,
        meta: providerMetaForSubmit(state, profile),
      });
    } finally {
      setSubmitting(false);
      onSubmittingChange?.(false);
    }
  };

  const beginAdoptProfile = async () => {
    if (!providerId || !resource || !suggestedProfile) return;
    if (
      suggestedProfile.credentialPolicy.mode === "managed_account" &&
      !adoptAccountId
    ) {
      toast.error(t("serverProviderForm.validation.accountRequired"));
      return;
    }
    setIdentityActionPending(true);
    try {
      const result = await providersApi.adoptProfile({
        app: appId,
        providerId,
        expectedRevision: resource.revision,
        profileId: suggestedProfile.profileId,
        accountId:
          suggestedProfile.credentialPolicy.mode === "managed_account"
            ? adoptAccountId
            : undefined,
        mode: "preview",
      });
      setPendingIdentityAction({
        kind: "adopt",
        previewToken: result.preview.previewToken,
        title: t("serverProviderForm.identity.adoptTitle"),
        message: [
          t("serverProviderForm.identity.adoptMessage", {
            provider: resource.provider.name,
            profile: suggestedProfile.label,
          }),
          ...result.preview.warnings,
        ].join("\n"),
      });
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setIdentityActionPending(false);
    }
  };

  const beginCloneAsCustom = async () => {
    if (!providerId || !resource) return;
    const customProfile = profileById(`${appId}.custom_http`);
    const policy = customProfile
      ? customPolicyForProfile(customProfile)
      : undefined;
    if (!customProfile || !policy) {
      toast.error(t("serverProviderForm.toasts.customUnavailable"));
      return;
    }
    const targetProviderId = cloneDraft.targetProviderId.trim();
    const targetName = cloneDraft.targetName.trim();
    if (!targetProviderId || targetProviderId === providerId) {
      toast.error(t("serverProviderForm.toasts.cloneIdInvalid"));
      return;
    }
    if (!targetName) {
      toast.error(t("serverProviderForm.toasts.cloneNameRequired"));
      return;
    }
    if (
      !policy.protocols.includes(
        cloneDraft.customBinding.upstreamProtocol as ProviderUpstreamProtocol,
      ) ||
      !policy.authSchemes.includes(
        cloneDraft.customBinding.authScheme as ProviderAuthScheme,
      )
    ) {
      toast.error(t("serverProviderForm.validation.unsupportedBinding"));
      return;
    }
    const request: CloneAsCustomDraft = {
      ...cloneDraft,
      targetProviderId,
      targetName,
      customBinding: clone(cloneDraft.customBinding),
    };
    setIdentityActionPending(true);
    try {
      const result = await providersApi.cloneAsCustom({
        app: appId,
        providerId,
        expectedRevision: resource.revision,
        ...request,
        mode: "preview",
      });
      setPendingIdentityAction({
        kind: "clone",
        previewToken: result.preview.previewToken,
        title: t("serverProviderForm.identity.cloneTitle"),
        message: [
          t("serverProviderForm.identity.cloneMessage", {
            name: request.targetName,
            id: request.targetProviderId,
          }),
          t("serverProviderForm.identity.upstreamProtocolLine", {
            protocol: request.customBinding.upstreamProtocol,
          }),
          t("serverProviderForm.identity.authSchemeLine", {
            scheme: request.customBinding.authScheme,
          }),
          ...result.preview.warnings,
        ].join("\n"),
        cloneDraft: request,
      });
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setIdentityActionPending(false);
    }
  };

  const beginCustomRebind = async () => {
    if (!providerId || !resource || !state.customBinding) return;
    const validationError = validateState(state, profile, t);
    if (validationError) {
      toast.error(validationError);
      return;
    }
    setIdentityActionPending(true);
    try {
      const result = await providersApi.rebindCustom({
        app: appId,
        providerId,
        expectedRevision: resource.revision,
        customBinding: state.customBinding,
        credentialPatches: collectPrimaryCredentialPatches(state),
        mode: "preview",
      });
      setPendingIdentityAction({
        kind: "rebind",
        previewToken: result.preview.previewToken,
        title: t("serverProviderForm.identity.rebindTitle"),
        message: [
          t("serverProviderForm.identity.upstreamProtocolLine", {
            protocol: state.customBinding.upstreamProtocol,
          }),
          t("serverProviderForm.identity.authSchemeLine", {
            scheme: state.customBinding.authScheme,
          }),
          ...result.preview.warnings,
        ].join("\n"),
      });
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setIdentityActionPending(false);
    }
  };

  const applyIdentityAction = async () => {
    if (!providerId || !resource || !pendingIdentityAction) return;
    setIdentityActionPending(true);
    try {
      if (pendingIdentityAction.kind === "adopt" && suggestedProfile) {
        await providersApi.adoptProfile({
          app: appId,
          providerId,
          expectedRevision: resource.revision,
          profileId: suggestedProfile.profileId,
          accountId:
            suggestedProfile.credentialPolicy.mode === "managed_account"
              ? adoptAccountId
              : undefined,
          mode: "apply",
          previewToken: pendingIdentityAction.previewToken,
        });
      } else if (
        pendingIdentityAction.kind === "rebind" &&
        state.customBinding
      ) {
        await providersApi.rebindCustom({
          app: appId,
          providerId,
          expectedRevision: resource.revision,
          customBinding: state.customBinding,
          credentialPatches: collectPrimaryCredentialPatches(state),
          mode: "apply",
          previewToken: pendingIdentityAction.previewToken,
        });
      } else if (
        pendingIdentityAction.kind === "clone" &&
        pendingIdentityAction.cloneDraft
      ) {
        await providersApi.cloneAsCustom({
          app: appId,
          providerId,
          expectedRevision: resource.revision,
          ...pendingIdentityAction.cloneDraft,
          mode: "apply",
          previewToken: pendingIdentityAction.previewToken,
        });
      }
      await queryClient.invalidateQueries({ queryKey: ["providers", appId] });
      toast.success(
        pendingIdentityAction.kind === "clone"
          ? t("serverProviderForm.toasts.customCreated")
          : t("serverProviderForm.toasts.identityUpdated"),
      );
      setPendingIdentityAction(null);
      if (pendingIdentityAction.kind === "clone") {
        const nextCloneDraft = buildCloneDraft(appId, resource);
        setCloneDraft(nextCloneDraft);
        setCloneBaseline(cloneDraftFingerprint(nextCloneDraft));
      } else {
        setRebindEditing(false);
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setIdentityActionPending(false);
    }
  };

  const customAuthRequiresSecret = profile.formComposition === "custom";
  const showEndpoint =
    profile.endpointPolicy === "custom" ||
    profile.endpointPolicy === "override_allowed" ||
    Boolean(state.endpoint);
  const endpointEditable =
    !isLockedPresetProfile(profile) &&
    (profile.endpointPolicy === "custom" ||
      profile.endpointPolicy === "override_allowed");
  const showCodexOptions =
    driverForProfile(profile)?.driverId === "oauth.openai_codex";

  return (
    <form
      id="provider-form"
      onSubmit={submit}
      className="space-y-6 rounded-lg border border-border/50 bg-background/40 p-5"
    >
      {!isEditMode ? (
        <ProviderPresetSelector
          selectedPresetId={state.profileId}
          presetEntries={presetEntries}
          presetCategoryLabels={presetCategoryLabels}
          onPresetChange={selectProfile}
          customPresetId={`${appId}.custom_http`}
          category={state.draft.category}
        />
      ) : (
        <div className="space-y-4 border-b border-border/50 pb-4">
          <ProviderIconControl
            icon={state.draft.icon}
            iconColor={state.draft.iconColor}
            providerName={state.draft.name}
            onChange={(icon, iconColor) => updateDraft({ icon, iconColor })}
          />
          <div className="flex flex-wrap items-center justify-center gap-2">
            <Badge variant="outline">{profile.label}</Badge>
            <Badge
              variant={profile.maturity === "stable" ? "secondary" : "outline"}
            >
              {profile.maturity === "stable"
                ? t("serverProviderForm.identity.stable")
                : t("serverProviderForm.identity.experimental")}
            </Badge>
            <span className="text-xs text-muted-foreground">
              {profile.profileId}
            </span>
          </div>
        </div>
      )}

      {resource?.identity.warning ? (
        <div className="flex items-start gap-2 rounded-md border border-amber-500/40 bg-amber-500/5 p-3 text-sm text-amber-700 dark:text-amber-300">
          <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
          <span>{resource.identity.warning}</span>
        </div>
      ) : null}

      {resource?.identity.status === "adoption_available" &&
      suggestedProfile ? (
        <Section title={t("serverProviderForm.identity.adoptSection")}>
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex items-center gap-2">
              <Badge variant="secondary">{suggestedProfile.label}</Badge>
              <span className="text-xs text-muted-foreground">
                {suggestedProfile.profileId}
              </span>
            </div>
            <Button
              type="button"
              variant="outline"
              disabled={identityActionPending}
              onClick={() => void beginAdoptProfile()}
            >
              {t("serverProviderForm.identity.adoptPreview")}
            </Button>
          </div>
          {suggestedProfile.credentialPolicy.mode === "managed_account" ? (
            <ManagedAccountSection
              providerType={
                suggestedProfile.credentialPolicy.accountProviderType
              }
              accountId={adoptAccountId}
              onAccountSelect={(accountId) =>
                setAdoptAccountId(accountId ?? "")
              }
            />
          ) : null}
        </Section>
      ) : null}

      {profile.formComposition === "legacy" && resource && cloneCustomPolicy ? (
        <Section title={t("serverProviderForm.identity.cloneSection")}>
          <div className="flex items-start gap-2 text-sm text-muted-foreground">
            <Copy className="mt-0.5 h-4 w-4 shrink-0" />
            {t("serverProviderForm.identity.cloneDescription")}
          </div>
          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2">
              <Label htmlFor="server-provider-clone-id">
                {t("serverProviderForm.identity.newProviderId")}
              </Label>
              <Input
                id="server-provider-clone-id"
                value={cloneDraft.targetProviderId}
                onChange={(event) =>
                  setCloneDraft((current) => ({
                    ...current,
                    targetProviderId: event.target.value,
                  }))
                }
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="server-provider-clone-name">
                {t("serverProviderForm.identity.newName")}
              </Label>
              <Input
                id="server-provider-clone-name"
                value={cloneDraft.targetName}
                onChange={(event) =>
                  setCloneDraft((current) => ({
                    ...current,
                    targetName: event.target.value,
                  }))
                }
              />
            </div>
            <div className="space-y-2">
              <Label>{t("serverProviderForm.binding.upstreamProtocol")}</Label>
              <Select
                value={cloneDraft.customBinding.upstreamProtocol}
                onValueChange={(value) =>
                  setCloneDraft((current) => ({
                    ...current,
                    customBinding: {
                      ...current.customBinding,
                      upstreamProtocol:
                        value as ProviderCustomBinding["upstreamProtocol"],
                    },
                  }))
                }
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {cloneCustomPolicy.protocols.map((protocol) => (
                    <SelectItem key={protocol} value={protocol}>
                      {protocol}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-2">
              <Label>{t("serverProviderForm.binding.authScheme")}</Label>
              <Select
                value={cloneDraft.customBinding.authScheme}
                onValueChange={(value) =>
                  setCloneDraft((current) => ({
                    ...current,
                    customBinding: {
                      ...current.customBinding,
                      authScheme: value as ProviderCustomBinding["authScheme"],
                    },
                  }))
                }
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {cloneCustomPolicy.authSchemes.map((scheme) => (
                    <SelectItem key={scheme} value={scheme}>
                      {scheme}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>
          <div className="flex justify-end">
            <Button
              type="button"
              variant="outline"
              disabled={identityActionPending}
              onClick={() => void beginCloneAsCustom()}
            >
              <Copy className="mr-2 h-4 w-4" />
              {t("serverProviderForm.identity.clonePreview")}
            </Button>
          </div>
        </Section>
      ) : null}

      <Section title={t("serverProviderForm.basic.title")}>
        <div className="grid gap-4 md:grid-cols-2">
          <div className="space-y-2">
            <Label htmlFor="server-provider-name">
              {t("serverProviderForm.basic.name")}
            </Label>
            <Input
              id="server-provider-name"
              value={state.draft.name}
              readOnly={isLockedPresetProfile(profile)}
              className={
                isLockedPresetProfile(profile)
                  ? "bg-muted/40 text-muted-foreground"
                  : undefined
              }
              onChange={(event) => updateDraft({ name: event.target.value })}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="server-provider-website">
              {t("serverProviderForm.basic.website")}
            </Label>
            <Input
              id="server-provider-website"
              type="url"
              value={state.draft.websiteUrl}
              readOnly={isLockedPresetProfile(profile)}
              className={
                isLockedPresetProfile(profile)
                  ? "bg-muted/40 text-muted-foreground"
                  : undefined
              }
              onChange={(event) =>
                updateDraft({ websiteUrl: event.target.value })
              }
            />
          </div>
        </div>
        <div className="space-y-2">
          <Label htmlFor="server-provider-notes">
            {t("serverProviderForm.basic.notes")}
          </Label>
          <Textarea
            id="server-provider-notes"
            value={state.draft.notes}
            rows={2}
            onChange={(event) => updateDraft({ notes: event.target.value })}
          />
        </div>
      </Section>

      {profile.formComposition === "managed_account" &&
      profile.credentialPolicy.mode === "managed_account" ? (
        <Section title={t("serverProviderForm.account.title")}>
          <ManagedAccountSection
            providerType={profile.credentialPolicy.accountProviderType}
            accountId={state.accountId}
            onAccountSelect={(accountId) =>
              setState((current) => ({
                ...current,
                accountId: accountId ?? "",
              }))
            }
          />
        </Section>
      ) : null}

      {profile.formComposition === "static_secret" &&
      state.credentials.primary ? (
        <Section title={t("serverProviderForm.credentials.title")}>
          <CredentialControl
            label={t("serverProviderForm.credentials.apiToken")}
            edit={state.credentials.primary}
            revealedValue={
              revealedCredentialValues[state.credentials.primary.slot]
            }
            revealStatus={
              credentialRevealStatuses[state.credentials.primary.slot]
            }
            onRetryReveal={() =>
              void retryCredentialReveal(state.credentials.primary.slot)
            }
            onChange={(next) => updateCredential("primary", next)}
          />
        </Section>
      ) : null}

      {profile.formComposition === "aws" ? (
        <Section title={t("serverProviderForm.aws.title")}>
          <div className="grid gap-4 md:grid-cols-2">
            <CredentialControl
              label="Access Key ID"
              edit={state.credentials.accessKeyId}
              revealedValue={
                revealedCredentialValues[state.credentials.accessKeyId.slot]
              }
              revealStatus={
                credentialRevealStatuses[state.credentials.accessKeyId.slot]
              }
              onRetryReveal={() =>
                void retryCredentialReveal(state.credentials.accessKeyId.slot)
              }
              onChange={(next) => updateCredential("accessKeyId", next)}
            />
            <CredentialControl
              label="Secret Access Key"
              edit={state.credentials.secretAccessKey}
              revealedValue={
                revealedCredentialValues[state.credentials.secretAccessKey.slot]
              }
              revealStatus={
                credentialRevealStatuses[state.credentials.secretAccessKey.slot]
              }
              onRetryReveal={() =>
                void retryCredentialReveal(
                  state.credentials.secretAccessKey.slot,
                )
              }
              onChange={(next) => updateCredential("secretAccessKey", next)}
            />
          </div>
          <CredentialControl
            label="Session Token"
            edit={state.credentials.sessionToken}
            optional
            revealedValue={
              revealedCredentialValues[state.credentials.sessionToken.slot]
            }
            revealStatus={
              credentialRevealStatuses[state.credentials.sessionToken.slot]
            }
            onRetryReveal={() =>
              void retryCredentialReveal(state.credentials.sessionToken.slot)
            }
            onChange={(next) => updateCredential("sessionToken", next)}
          />
          <div className="space-y-2">
            <Label htmlFor="server-provider-aws-region">Region</Label>
            <Input
              id="server-provider-aws-region"
              value={state.awsRegion}
              onChange={(event) =>
                setState((current) => {
                  const awsRegion = event.target.value;
                  return {
                    ...current,
                    awsRegion,
                    endpoint: canonicalPresetEndpoint(
                      profile,
                      appId,
                      awsRegion,
                    ),
                  };
                })
              }
              placeholder="us-east-1"
            />
          </div>
        </Section>
      ) : null}

      {profile.formComposition === "custom" &&
      state.customBinding &&
      customPolicy ? (
        <Section title={t("serverProviderForm.binding.title")}>
          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2">
              <Label>{t("serverProviderForm.binding.upstreamProtocol")}</Label>
              <Select
                value={state.customBinding.upstreamProtocol}
                disabled={isEditMode && !rebindEditing}
                onValueChange={(value) =>
                  setState((current) => ({
                    ...current,
                    customBinding: {
                      ...current.customBinding!,
                      upstreamProtocol:
                        value as ProviderCustomBinding["upstreamProtocol"],
                    },
                  }))
                }
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {customPolicy.protocols.map((protocol) => (
                    <SelectItem key={protocol} value={protocol}>
                      {protocol}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-2">
              <Label>{t("serverProviderForm.binding.authScheme")}</Label>
              <Select
                value={state.customBinding.authScheme}
                disabled={isEditMode && !rebindEditing}
                onValueChange={(value) =>
                  setState((current) => ({
                    ...current,
                    customBinding: {
                      ...current.customBinding!,
                      authScheme: value as ProviderCustomBinding["authScheme"],
                    },
                  }))
                }
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {customPolicy.authSchemes.map((scheme) => (
                    <SelectItem key={scheme} value={scheme}>
                      {scheme}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>
          {customAuthRequiresSecret && state.credentials.primary ? (
            <CredentialControl
              label={t("serverProviderForm.binding.credential")}
              edit={state.credentials.primary}
              revealedValue={
                revealedCredentialValues[state.credentials.primary.slot]
              }
              revealStatus={
                credentialRevealStatuses[state.credentials.primary.slot]
              }
              onRetryReveal={() =>
                void retryCredentialReveal(state.credentials.primary.slot)
              }
              onChange={(next) => updateCredential("primary", next)}
            />
          ) : null}
          {state.customBinding.authScheme === "custom_header" ||
          state.customBinding.authScheme === "query" ? (
            <div className="space-y-2">
              <Label htmlFor="server-provider-auth-field">
                {state.customBinding.authScheme === "query"
                  ? t("serverProviderForm.binding.queryParameter")
                  : t("serverProviderForm.binding.headerName")}
              </Label>
              <Input
                id="server-provider-auth-field"
                value={state.draft.meta.apiKeyField ?? ""}
                onChange={(event) =>
                  updateMeta({
                    apiKeyField: event.target
                      .value as ProviderMeta["apiKeyField"],
                  })
                }
                placeholder={
                  state.customBinding.authScheme === "query"
                    ? "key"
                    : "x-provider-key"
                }
              />
            </div>
          ) : null}
          {isEditMode ? (
            <div className="flex justify-end gap-2">
              {rebindEditing ? (
                <>
                  <Button
                    type="button"
                    variant="ghost"
                    onClick={() => {
                      setState((current) => ({
                        ...current,
                        customBinding: resource?.customBinding
                          ? clone(resource.customBinding)
                          : current.customBinding,
                      }));
                      setRebindEditing(false);
                    }}
                  >
                    {t("serverProviderForm.binding.cancelChanges")}
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    disabled={identityActionPending}
                    onClick={() => void beginCustomRebind()}
                  >
                    {t("serverProviderForm.binding.previewApply")}
                  </Button>
                </>
              ) : (
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => setRebindEditing(true)}
                >
                  {t("serverProviderForm.binding.change")}
                </Button>
              )}
            </div>
          ) : null}
          {customBindingDirty ? (
            <div className="flex items-start gap-2 text-sm text-amber-700 dark:text-amber-300">
              <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
              {t("serverProviderForm.binding.pendingHint")}
            </div>
          ) : null}
        </Section>
      ) : null}

      {showEndpoint ? (
        <Section title={t("serverProviderForm.endpoint.title")}>
          <div className="grid grid-cols-[minmax(0,1fr)_2.5rem_4.5rem] items-center gap-2">
            <Input
              value={state.endpoint}
              readOnly={!endpointEditable}
              className={
                !endpointEditable
                  ? "bg-muted/40 text-muted-foreground"
                  : undefined
              }
              onChange={(event) => {
                endpointProbeGeneration.current += 1;
                setEndpointProbe(null);
                setState((current) => ({
                  ...current,
                  endpoint: event.target.value,
                }));
              }}
              placeholder="https://api.example.com"
            />
            <Button
              type="button"
              size="icon"
              variant="outline"
              disabled={
                endpointProbe?.pending || !isValidEndpoint(state.endpoint)
              }
              title={t("endpointTest.testSpeed")}
              aria-label={t("endpointTest.testSpeed")}
              onClick={() => void testEndpoint()}
            >
              {endpointProbe?.pending ? (
                <LoaderCircle className="h-4 w-4 animate-spin" />
              ) : (
                <Gauge className="h-4 w-4" />
              )}
            </Button>
            <span
              className="min-w-[4.5rem] text-right font-mono text-xs text-muted-foreground"
              aria-live="polite"
              title={
                endpointProbe?.error ??
                (endpointProbe?.status
                  ? `HTTP ${endpointProbe.status}`
                  : undefined)
              }
            >
              {endpointProbe?.pending
                ? t("endpointTest.testing")
                : endpointProbe?.url === state.endpoint.trim() &&
                    endpointProbe.latency != null
                  ? `${endpointProbe.latency} ms`
                  : endpointProbe?.url === state.endpoint.trim() &&
                      endpointProbe.error
                    ? t("endpointTest.failed")
                    : ""}
            </span>
          </div>
        </Section>
      ) : null}

      <Section title={t("serverProviderForm.model.title")}>
        {profile.modelPolicy === "single" ? (
          <div className="space-y-2">
            <Label htmlFor="server-provider-model">
              {t("serverProviderForm.model.upstreamModel")}
            </Label>
            <Input
              id="server-provider-model"
              value={state.upstreamModel}
              onChange={(event) =>
                setState((current) => ({
                  ...current,
                  upstreamModel: event.target.value,
                }))
              }
            />
          </div>
        ) : (
          <div className="flex items-center gap-2 text-sm">
            <Badge variant="secondary">
              {t("serverProviderForm.model.passthrough")}
            </Badge>
            <span className="text-muted-foreground">
              {t("serverProviderForm.model.passthroughHint")}
            </span>
          </div>
        )}
      </Section>

      {profile.formComposition === "custom" ? (
        <Section title={t("serverProviderForm.headers.title")}>
          <div className="space-y-3">
            {state.extraHeaders
              .filter((header) => !header.removed)
              .map((header) => (
                <div
                  key={header.id}
                  className="grid gap-2 md:grid-cols-[minmax(0,1fr)_minmax(0,1.4fr)_auto]"
                >
                  <Input
                    value={header.name}
                    readOnly={Boolean(header.originalName)}
                    onChange={(event) =>
                      setState((current) => ({
                        ...current,
                        extraHeaders: current.extraHeaders.map((item) =>
                          item.id === header.id
                            ? { ...item, name: event.target.value }
                            : item,
                        ),
                      }))
                    }
                    placeholder="x-tenant-id"
                  />
                  <SecretInput
                    value={header.value}
                    disabled={header.action === "keep"}
                    placeholder={
                      header.configured && header.action === "keep"
                        ? t("serverProviderForm.credentials.configured")
                        : t("serverProviderForm.headers.value")
                    }
                    onChange={(event) =>
                      setState((current) => ({
                        ...current,
                        extraHeaders: current.extraHeaders.map((item) =>
                          item.id === header.id
                            ? {
                                ...item,
                                action: "replace",
                                value: event.target.value,
                              }
                            : item,
                        ),
                      }))
                    }
                  />
                  <Button
                    type="button"
                    size="icon"
                    variant="ghost"
                    title={t("serverProviderForm.headers.remove")}
                    onClick={() =>
                      setState((current) => ({
                        ...current,
                        extraHeaders: current.extraHeaders.map((item) =>
                          item.id === header.id
                            ? { ...item, removed: true }
                            : item,
                        ),
                      }))
                    }
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                </div>
              ))}
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() =>
                setState((current) => ({
                  ...current,
                  extraHeaders: [
                    ...current.extraHeaders,
                    {
                      id: crypto.randomUUID(),
                      name: "",
                      configured: false,
                      action: "replace",
                      value: "",
                      removed: false,
                    },
                  ],
                }))
              }
            >
              <Plus className="mr-2 h-4 w-4" />
              {t("serverProviderForm.headers.add")}
            </Button>
          </div>
        </Section>
      ) : null}

      {showCodexOptions ? (
        <Section title={t("serverProviderForm.codex.title")}>
          <div className="grid gap-3 md:grid-cols-3">
            {[
              ["FAST", "codexFastMode"],
              [
                t("serverProviderForm.codex.imageGeneration"),
                "codexImageGenerationEnabled",
              ],
              ["WebSocket", "codexWebsocketEnabled"],
            ].map(([label, key]) => (
              <div
                key={key}
                className="flex items-center justify-between rounded-md border border-border/50 px-3 py-2"
              >
                <Label>{label}</Label>
                <Switch
                  checked={state[key as keyof EditorState] as boolean}
                  onCheckedChange={(checked) =>
                    setState((current) => ({ ...current, [key]: checked }))
                  }
                />
              </div>
            ))}
          </div>
        </Section>
      ) : null}

      {profile.formComposition !== "legacy" ? (
        <Section title={t("serverProviderForm.usage.title")}>
          <div className="grid gap-4 md:grid-cols-3">
            <div className="space-y-2">
              <Label>{t("serverProviderForm.usage.costMultiplier")}</Label>
              <Input
                inputMode="decimal"
                value={state.costMultiplier}
                onChange={(event) =>
                  setState((current) => ({
                    ...current,
                    costMultiplier: event.target.value,
                  }))
                }
                placeholder="1"
              />
            </div>
            <div className="space-y-2">
              <Label>{t("serverProviderForm.usage.pricingModel")}</Label>
              <Select
                value={state.pricingModelSource}
                onValueChange={(value) =>
                  setState((current) => ({
                    ...current,
                    pricingModelSource:
                      value as EditorState["pricingModelSource"],
                  }))
                }
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="inherit">
                    {t("serverProviderForm.usage.inherit")}
                  </SelectItem>
                  <SelectItem value="request">
                    {t("serverProviderForm.usage.request")}
                  </SelectItem>
                  <SelectItem value="response">
                    {t("serverProviderForm.usage.response")}
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-2">
              <Label>{t("serverProviderForm.usage.quotaLimit")}</Label>
              <Input
                inputMode="numeric"
                value={state.quotaDispatchLimitPercent}
                onChange={(event) =>
                  setState((current) => ({
                    ...current,
                    quotaDispatchLimitPercent: event.target.value,
                  }))
                }
                placeholder={t("serverProviderForm.usage.unlimited")}
              />
            </div>
          </div>
          <div className="space-y-2">
            <Label>{t("serverProviderForm.usage.customUserAgent")}</Label>
            <Input
              value={state.customUserAgent}
              onChange={(event) =>
                setState((current) => ({
                  ...current,
                  customUserAgent: event.target.value,
                }))
              }
            />
          </div>
        </Section>
      ) : (
        <Section title={t("serverProviderForm.legacy.title")}>
          <div className="flex items-start gap-2 text-sm text-muted-foreground">
            <KeyRound className="mt-0.5 h-4 w-4 shrink-0" />
            {t("serverProviderForm.legacy.hint")}
          </div>
        </Section>
      )}

      {providerId ? (
        <ProviderShareSection
          appId={appId}
          providerId={providerId}
          providerName={state.draft.name}
          onOpenShareSettings={onOpenShareSettings}
          onDirtyChange={setShareDirty}
        />
      ) : (
        <ProviderSharePlaceholder />
      )}

      {showButtons ? (
        <div className="flex justify-end gap-2 pt-2">
          <Button type="button" variant="outline" onClick={onCancel}>
            {t("common.cancel")}
          </Button>
          <Button
            type="submit"
            disabled={
              submitting || customBindingDirty || (isEditMode && !providerDirty)
            }
          >
            <Save className="mr-2 h-4 w-4" />
            {submitLabel}
          </Button>
        </div>
      ) : null}
      <ConfirmDialog
        isOpen={pendingIdentityAction !== null}
        title={
          pendingIdentityAction?.title ??
          t("serverProviderForm.identity.confirmTitle")
        }
        message={pendingIdentityAction?.message ?? ""}
        confirmText={t("serverProviderForm.identity.apply")}
        variant="info"
        zIndex="top"
        onConfirm={() => void applyIdentityAction()}
        onCancel={() => {
          if (!identityActionPending) setPendingIdentityAction(null);
        }}
      />
    </form>
  );
}
