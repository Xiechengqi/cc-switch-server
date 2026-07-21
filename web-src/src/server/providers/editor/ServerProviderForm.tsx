import { useEffect, useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  Copy,
  KeyRound,
  Plus,
  Save,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";

import {
  ProviderSharePlaceholder,
  ProviderShareSection,
} from "@/components/providers/ProviderShareSection";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { AntigravityOAuthSection } from "@/components/providers/forms/AntigravityOAuthSection";
import { ClaudeOAuthSection } from "@/components/providers/forms/ClaudeOAuthSection";
import { CodexOAuthSection } from "@/components/providers/forms/CodexOAuthSection";
import { CopilotAuthSection } from "@/components/providers/forms/CopilotAuthSection";
import { CursorOAuthSection } from "@/components/providers/forms/CursorOAuthSection";
import { DeepSeekAccountSection } from "@/components/providers/forms/DeepSeekAccountSection";
import { GeminiOAuthSection } from "@/components/providers/forms/GeminiOAuthSection";
import { GrokOAuthSection } from "@/components/providers/forms/GrokOAuthSection";
import { KiroOAuthSection } from "@/components/providers/forms/KiroOAuthSection";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import {
  createDraftForProfile,
  defaultSingleModel,
  ensureObject,
  readEndpoint,
  readUpstreamModel,
  setEndpoint,
  setPassthroughModel,
  setSingleModel,
  type CoreProviderDraft,
} from "./providerDraft";

const KEEP_SENTINEL = "__CC_SWITCH_SECRET_KEEP__";
const PRIMARY_CREDENTIAL_SLOT = "/settingsConfig/apiKey";
const EXTRA_HEADER_PREFIX = "/settingsConfig/extraHeaders/";

type CredentialAction = "keep" | "replace" | "clear";

interface CredentialEdit {
  slot: string;
  configured: boolean;
  action: CredentialAction;
  value: string;
}

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
    if (!sessionToken.configured) sessionToken.action = "clear";
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
    awsRegion:
      String(
        (draft.settingsConfig.env as Record<string, unknown> | undefined)
          ?.AWS_REGION ?? "us-east-1",
      ).trim() || "us-east-1",
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
): string | null {
  if (!state.draft.name.trim()) return "供应商名称不能为空";
  if (profile.formComposition === "legacy") return null;
  if (profile.credentialPolicy.mode === "managed_account" && !state.accountId) {
    return "请选择一个已认证账号";
  }
  if (
    (profile.endpointPolicy === "custom" ||
      profile.endpointPolicy === "override_allowed") &&
    !isValidEndpoint(state.endpoint)
  ) {
    return "请输入有效的 HTTP 或 HTTPS Endpoint";
  }
  if (profile.modelPolicy === "single" && !state.upstreamModel.trim()) {
    return "实际请求模型不能为空";
  }
  if (profile.formComposition === "aws" && !state.awsRegion.trim()) {
    return "AWS Region 不能为空";
  }
  for (const [name, edit] of Object.entries(state.credentials)) {
    const required = name !== "sessionToken";
    if (edit.action === "replace" && required && !edit.value.trim()) {
      return "请填写需要替换的凭据";
    }
    if (edit.action === "clear" && required) {
      return "必需凭据不能清除；请选择替换";
    }
  }
  if (profile.formComposition === "custom") {
    if (!state.customBinding) return "Custom Provider 协议配置缺失";
    const customPolicy = customPolicyForProfile(profile);
    if (
      !customPolicy?.protocols.includes(
        state.customBinding.upstreamProtocol as ProviderUpstreamProtocol,
      ) ||
      !customPolicy.authSchemes.includes(
        state.customBinding.authScheme as ProviderAuthScheme,
      )
    ) {
      return "当前协议与认证组合不受支持";
    }
    const authRequired = true;
    const primary = state.credentials.primary;
    if (
      authRequired &&
      primary?.action === "replace" &&
      !primary.value.trim()
    ) {
      return "请填写认证凭据";
    }
    if (authRequired && primary?.action === "clear") {
      return "当前认证方式需要凭据";
    }
    const names = new Set<string>();
    for (const header of state.extraHeaders.filter((item) => !item.removed)) {
      const name = header.name.trim().toLowerCase();
      if (!isValidHeaderName(name)) return `Header 名称无效: ${header.name}`;
      if (HEADER_DENYLIST.has(name)) return `Header 由驱动管理: ${header.name}`;
      if (names.has(name)) return `Header 名称重复: ${header.name}`;
      names.add(name);
      if (header.action === "replace" && !header.value.trim()) {
        return `请填写 Header ${header.name} 的值`;
      }
    }
  }
  if (state.costMultiplier.trim()) {
    const value = Number(state.costMultiplier);
    if (!Number.isFinite(value) || value < 0) return "成本倍率必须是非负数字";
  }
  if (state.quotaDispatchLimitPercent.trim()) {
    const value = Number(state.quotaDispatchLimitPercent);
    if (!Number.isInteger(value) || value < 1 || value > 100) {
      return "调度用量上限必须是 1-100 的整数";
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

function CredentialControl({
  label,
  edit,
  optional = false,
  onChange,
}: {
  label: string;
  edit: CredentialEdit;
  optional?: boolean;
  onChange: (next: CredentialEdit) => void;
}) {
  const actions: CredentialAction[] = edit.configured
    ? optional
      ? ["keep", "replace", "clear"]
      : ["keep", "replace"]
    : optional
      ? ["replace", "clear"]
      : ["replace"];
  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <Label>{label}</Label>
        <div className="flex h-8 items-center rounded-md border border-border/60 p-0.5">
          {actions.map((action) => (
            <Button
              key={action}
              type="button"
              size="sm"
              variant={edit.action === action ? "secondary" : "ghost"}
              className="h-7 rounded-sm px-2 text-xs"
              onClick={() => onChange({ ...edit, action, value: "" })}
            >
              {action === "keep"
                ? "保留"
                : action === "replace"
                  ? "替换"
                  : "清除"}
            </Button>
          ))}
        </div>
      </div>
      {edit.action === "replace" ? (
        <SecretInput
          value={edit.value}
          onChange={(event) => onChange({ ...edit, value: event.target.value })}
          autoComplete="new-password"
          placeholder={edit.configured ? "输入新凭据" : "输入凭据"}
        />
      ) : null}
      <div className="text-xs text-muted-foreground">
        {edit.configured ? "已配置" : optional ? "未配置（可选）" : "未配置"}
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
          不支持的账号类型：{providerType}
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
  const queryClient = useQueryClient();
  const isEditMode = Boolean(initialData && providerId);
  const initializationKey = `${appId}:${providerId ?? "new"}:${resource?.revision ?? 0}`;
  const initial = useMemo(
    () => buildEditorState(appId, resource, initialData),
    // The explicit key prevents query refreshes from replacing an active draft.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [initializationKey],
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

  useEffect(() => {
    setState(initial);
    setBaseline(stableStringify(initial));
    setShareDirty(false);
    setRebindEditing(false);
    setAdoptAccountId("");
    const nextCloneDraft = buildCloneDraft(appId, resource);
    setCloneDraft(nextCloneDraft);
    setCloneBaseline(cloneDraftFingerprint(nextCloneDraft));
    // `initial` already includes the app/provider/revision initialization key.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initial]);

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

  const selectProfile = (profileId: string) => {
    const nextProfile = profileById(profileId);
    if (!nextProfile || nextProfile.app !== appId) return;
    setState(buildEditorState(appId, undefined, undefined, nextProfile));
  };

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (customBindingDirty) {
      toast.error("请先预览并应用协议绑定，再保存其他配置");
      return;
    }
    const validationError = validateState(state, profile);
    if (validationError) {
      toast.error(validationError);
      return;
    }
    const settings = prepareSettingsForSubmit(state, profile, appId);
    const credentialPatches = collectCredentialPatches(state);
    setSubmitting(true);
    onSubmittingChange?.(true);
    try {
      await onSubmit({
        name: state.draft.name.trim(),
        websiteUrl: state.draft.websiteUrl.trim() || undefined,
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
      toast.error("请选择一个已认证账号");
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
        title: "采用供应商配置",
        message: [
          `将 ${resource.provider.name} 绑定到 ${suggestedProfile.label}。`,
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
      toast.error("Custom Provider 配置不可用");
      return;
    }
    const targetProviderId = cloneDraft.targetProviderId.trim();
    const targetName = cloneDraft.targetName.trim();
    if (!targetProviderId || targetProviderId === providerId) {
      toast.error("请输入与原 Provider 不同的新 ID");
      return;
    }
    if (!targetName) {
      toast.error("请输入新 Provider 名称");
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
      toast.error("当前协议与认证组合不受支持");
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
        title: "复制为 Custom Provider",
        message: [
          `创建 ${request.targetName}（${request.targetProviderId}）。`,
          `上游协议：${request.customBinding.upstreamProtocol}`,
          `认证方式：${request.customBinding.authScheme}`,
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
    const validationError = validateState(state, profile);
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
        title: "更改 Custom 绑定",
        message: [
          `上游协议：${state.customBinding.upstreamProtocol}`,
          `认证方式：${state.customBinding.authScheme}`,
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
          ? "Custom Provider 已创建"
          : "供应商身份配置已更新",
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
    profile.endpointPolicy === "custom" ||
    profile.endpointPolicy === "override_allowed";
  const showCodexOptions =
    driverForProfile(profile)?.driverId === "oauth.openai_codex";

  return (
    <form
      id="provider-form"
      onSubmit={submit}
      className="space-y-6 rounded-lg border border-border/50 bg-background/40 p-5"
    >
      {!isEditMode ? (
        <Section title="供应商类型">
          <Select value={state.profileId} onValueChange={selectProfile}>
            <SelectTrigger className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {profileList(appId).map((item) => (
                <SelectItem key={item.profileId} value={item.profileId}>
                  {item.label}
                  {item.maturity === "experimental" ? " · 实验性" : ""}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Section>
      ) : (
        <div className="flex flex-wrap items-center gap-2 border-b border-border/50 pb-4">
          <Badge variant="outline">{profile.label}</Badge>
          <Badge
            variant={profile.maturity === "stable" ? "secondary" : "outline"}
          >
            {profile.maturity === "stable" ? "稳定" : "实验性"}
          </Badge>
          <span className="text-xs text-muted-foreground">
            {profile.profileId}
          </span>
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
        <Section title="采用标准配置">
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
              预览并采用
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
        <Section title="复制为 Custom Provider">
          <div className="flex items-start gap-2 text-sm text-muted-foreground">
            <Copy className="mt-0.5 h-4 w-4 shrink-0" />
            保留当前
            Provider，创建一份具有显式协议身份的新配置。应用前会预览运行时差异。
          </div>
          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2">
              <Label htmlFor="server-provider-clone-id">新 Provider ID</Label>
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
              <Label htmlFor="server-provider-clone-name">新名称</Label>
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
              <Label>上游协议</Label>
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
              <Label>认证方式</Label>
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
              预览并创建
            </Button>
          </div>
        </Section>
      ) : null}

      <Section title="基本信息">
        <div className="grid gap-4 md:grid-cols-2">
          <div className="space-y-2">
            <Label htmlFor="server-provider-name">名称</Label>
            <Input
              id="server-provider-name"
              value={state.draft.name}
              onChange={(event) => updateDraft({ name: event.target.value })}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="server-provider-website">网站</Label>
            <Input
              id="server-provider-website"
              type="url"
              value={state.draft.websiteUrl}
              onChange={(event) =>
                updateDraft({ websiteUrl: event.target.value })
              }
            />
          </div>
        </div>
        <div className="space-y-2">
          <Label htmlFor="server-provider-notes">备注</Label>
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
        <Section title="认证账号">
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
        <Section title="认证凭据">
          <CredentialControl
            label="API Key / Bearer Token"
            edit={state.credentials.primary}
            onChange={(next) => updateCredential("primary", next)}
          />
        </Section>
      ) : null}

      {profile.formComposition === "aws" ? (
        <Section title="AWS 凭据">
          <div className="grid gap-4 md:grid-cols-2">
            <CredentialControl
              label="Access Key ID"
              edit={state.credentials.accessKeyId}
              onChange={(next) => updateCredential("accessKeyId", next)}
            />
            <CredentialControl
              label="Secret Access Key"
              edit={state.credentials.secretAccessKey}
              onChange={(next) => updateCredential("secretAccessKey", next)}
            />
          </div>
          <CredentialControl
            label="Session Token"
            edit={state.credentials.sessionToken}
            optional
            onChange={(next) => updateCredential("sessionToken", next)}
          />
          <div className="space-y-2">
            <Label htmlFor="server-provider-aws-region">Region</Label>
            <Input
              id="server-provider-aws-region"
              value={state.awsRegion}
              onChange={(event) =>
                setState((current) => ({
                  ...current,
                  awsRegion: event.target.value,
                }))
              }
              placeholder="us-east-1"
            />
          </div>
        </Section>
      ) : null}

      {profile.formComposition === "custom" &&
      state.customBinding &&
      customPolicy ? (
        <Section title="协议与认证">
          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2">
              <Label>上游协议</Label>
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
              <Label>认证方式</Label>
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
              label="认证凭据"
              edit={state.credentials.primary}
              onChange={(next) => updateCredential("primary", next)}
            />
          ) : null}
          {state.customBinding.authScheme === "custom_header" ||
          state.customBinding.authScheme === "query" ? (
            <div className="space-y-2">
              <Label htmlFor="server-provider-auth-field">
                {state.customBinding.authScheme === "query"
                  ? "Query 参数名"
                  : "Header 名称"}
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
                    取消更改
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    disabled={identityActionPending}
                    onClick={() => void beginCustomRebind()}
                  >
                    预览并应用绑定
                  </Button>
                </>
              ) : (
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => setRebindEditing(true)}
                >
                  更改协议绑定
                </Button>
              )}
            </div>
          ) : null}
          {customBindingDirty ? (
            <div className="flex items-start gap-2 text-sm text-amber-700 dark:text-amber-300">
              <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
              协议身份尚未应用。请先完成“预览并应用绑定”，再保存
              Endpoint、模型或请求配置。
            </div>
          ) : null}
        </Section>
      ) : null}

      {showEndpoint ? (
        <Section title="Endpoint">
          <Input
            value={state.endpoint}
            readOnly={!endpointEditable}
            className={
              !endpointEditable
                ? "bg-muted/40 text-muted-foreground"
                : undefined
            }
            onChange={(event) =>
              setState((current) => ({
                ...current,
                endpoint: event.target.value,
              }))
            }
            placeholder="https://api.example.com"
          />
        </Section>
      ) : null}

      <Section title="模型策略">
        {profile.modelPolicy === "single" ? (
          <div className="space-y-2">
            <Label htmlFor="server-provider-model">实际上游模型</Label>
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
            <Badge variant="secondary">透传</Badge>
            <span className="text-muted-foreground">使用请求中的模型</span>
          </div>
        )}
      </Section>

      {profile.formComposition === "custom" ? (
        <Section title="额外请求 Header">
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
                        ? "已配置"
                        : "Header 值"
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
                    title="删除 Header"
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
              添加 Header
            </Button>
          </div>
        </Section>
      ) : null}

      {showCodexOptions ? (
        <Section title="Codex 运行选项">
          <div className="grid gap-3 md:grid-cols-3">
            {[
              ["FAST", "codexFastMode"],
              ["图片生成", "codexImageGenerationEnabled"],
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
        <Section title="用量与请求">
          <div className="grid gap-4 md:grid-cols-3">
            <div className="space-y-2">
              <Label>成本倍率</Label>
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
              <Label>计费模型</Label>
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
                  <SelectItem value="inherit">继承全局</SelectItem>
                  <SelectItem value="request">请求模型</SelectItem>
                  <SelectItem value="response">返回模型</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-2">
              <Label>调度用量上限（%）</Label>
              <Input
                inputMode="numeric"
                value={state.quotaDispatchLimitPercent}
                onChange={(event) =>
                  setState((current) => ({
                    ...current,
                    quotaDispatchLimitPercent: event.target.value,
                  }))
                }
                placeholder="不限制"
              />
            </div>
          </div>
          <div className="space-y-2">
            <Label>自定义 User-Agent</Label>
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
        <Section title="兼容状态">
          <div className="flex items-start gap-2 text-sm text-muted-foreground">
            <KeyRound className="mt-0.5 h-4 w-4 shrink-0" />
            历史 Provider 仅允许修改显示信息；运行配置保持冻结。
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
            取消
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
        title={pendingIdentityAction?.title ?? "确认身份变更"}
        message={pendingIdentityAction?.message ?? ""}
        confirmText="应用"
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
