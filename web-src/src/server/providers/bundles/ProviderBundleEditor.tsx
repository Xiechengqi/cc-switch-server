import {
  Fragment,
  useEffect,
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import {
  ArrowLeft,
  ArrowRight,
  ArrowRightLeft,
  ChevronDown,
  Copy,
  KeyRound,
  Layers,
  LoaderCircle,
  Plus,
  RefreshCw,
  Save,
  Share2,
  SlidersHorizontal,
  Stethoscope,
  Target,
  Trash2,
  Wrench,
} from "lucide-react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ProviderIconControl } from "@/components/providers/ProviderIconControl";
import { ShareUserGrantsEditor } from "@/components/providers/ShareUserGrantsEditor";
import { CodexBankedResetPanel } from "@/components/providers/forms/CodexBankedResetPanel";
import { ManagedAccountSection } from "@/components/providers/forms/ManagedAccountSection";
import { CodexReferralPanel } from "@/components/providers/forms/CodexReferralPanel";
import { SubdomainGeneratorButton } from "@/components/SubdomainGeneratorButton";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import type {
  ProviderBundleView,
  ProviderCustomBinding,
  ProviderHealthCheckConfig,
  ProviderRequestDefaults,
} from "@/lib/api/providers";
import { providersApi } from "@/lib/api/providers";
import { shareApi, type ShareUserPolicy } from "@/lib/api/share";
import { normalizeManagedAuthProvider } from "@/lib/authBinding";
import { useUnsavedChangesGuard } from "@/hooks/useUnsavedChangesGuard";
import { copyText } from "@/lib/clipboard";
import { stableStringify } from "@/lib/stableStringify";
import {
  managedAccountKeys,
  shareKeys,
  useClientTunnelQuery,
  useManagedAccountsQuery,
  useSharesQuery,
} from "@/lib/query";
import {
  customPolicyForProfile,
  customRecipesForFamily,
  driverForProfile,
  familyById,
  profileById,
  type CoreProviderApp,
  type ProviderFamilySpec,
} from "@/server/providerRegistry";
import {
  CodexFeatureOptions,
  type CodexFeatureOptionKey,
} from "@/server/providers/CodexFeatureOptions";
import {
  GrokFeatureOptions,
  type GrokFeatureOptionKey,
} from "@/server/providers/GrokFeatureOptions";
import { profileAllowsEndpointEditing } from "@/server/providers/editor/providerDraft";
import {
  credentialInputValue,
  updateCredentialInput,
  type CredentialEdit,
  type CredentialRevealStatus,
} from "@/server/providers/editor/credentialEditing";
import { SecretInput } from "@/server/ui/SecretInput";
import { cn } from "@/lib/utils";
import {
  buildShareUserGrants,
  isValidShareEmail,
  SHARE_TOKEN_PRESETS,
} from "@/utils/shareFormUtils";
import { DEFAULT_PARALLEL_LIMIT } from "@/utils/shareUtils";
import {
  BUNDLE_SHARE_EXPIRY_PRESETS,
  createBundleShareDraft,
  isValidShareSlug,
  bundleShareGrantHandlers,
  saveBundleShare,
  shareForBundle,
  type ProviderBundleShareDraft,
} from "./bundleShare";
import {
  applyCustomRecipeToBundleDraft,
  BUNDLE_TEST_APP_ORDER,
  changeModelPolicyScope,
  customRecipeMatchesBundleDraft,
  defaultUpstreamModelForFamily,
  duplicateProviderBundleDraft,
  editProviderBundleDraft,
  familyCredentialSlots,
  modelPoliciesForFamily,
  modelPoliciesForSurface,
  normalizeBundleTestApp,
  perAppModelPoliciesDiffer,
  providerBundleIdentityEditable,
  requiresPerAppModelPolicy,
  resolvePersistedCodexControlTarget,
  supportsPerAppModelPolicy,
  toProviderBundleWriteDraft,
  updateBundleModel,
  updateSurfaceEndpoint,
  updateSurfaceModel,
  type BundleHeaderDraft,
  type BundleSecretDraft,
  type BundleSurfaceEditorDraft,
  type BundleValidationField,
  type BundleValidationIssue,
  type ProviderBundleEditorDraft,
} from "./bundleDraft";
import { createDraftForSelectedFamily } from "./bundleDefaults";
import {
  bundleValidationFieldId,
  firstBundleValidationIssue,
  matchesBundleValidationIssue,
} from "./bundleValidation";
import {
  familyLabel,
  preferredManagedAccount,
  recommendedFamily,
} from "./familyCatalog";
import { FamilyPicker } from "./FamilyPicker";
import {
  canVisitCreateStep,
  CREATE_STEPS,
  nextCreateStep,
  previousCreateStep,
  unlockCreateStep,
  type CreateStep,
} from "./createStepNavigation";
import { APP_LABELS, AppLogo } from "./bundleApps";
import { ConfigCard } from "./ConfigCard";
import {
  BundleReadinessBar,
  StatusDot,
  readinessTone,
  useGapLabel,
} from "./BundleReadinessBar";
import { bundleReadiness, type BundleGap } from "./bundleReadiness";
import { runtimeSummaryRows, type RuntimeSummaryRowId } from "./runtimeSummary";

interface ProviderBundleEditorProps {
  bundle?: ProviderBundleView;
  duplicate?: boolean;
  initialSection?: "share";
  onCancel: () => void;
  onSaved: (bundle: ProviderBundleView) => void;
  onOpenShareSettings?: () => void;
}

function fieldErrorClass(invalid: boolean): string {
  return invalid ? "border-destructive focus-visible:ring-destructive" : "";
}

/**
 * Two frames, not one: opening a card is a state change, and Radix only mounts the
 * collapsed content on the commit after that, so a single rAF finds nothing.
 */
function scrollToWhenRendered(id: string, block: ScrollLogicalPosition) {
  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      document
        .getElementById(id)
        ?.scrollIntoView({ behavior: "smooth", block });
    });
  });
}

function Section({
  title,
  icon,
  action,
  children,
}: {
  title: string;
  icon?: React.ReactNode;
  action?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="space-y-4 pb-6">
      <h2 className="flex items-center justify-between gap-3 text-sm font-semibold">
        <span className="flex min-w-0 items-center gap-2">
          {icon}
          {title}
        </span>
        {action}
      </h2>
      {children}
    </section>
  );
}

function fieldLabel(logical: string): string {
  if (logical.endsWith("/VOLC_ACCESS_KEY_ID")) {
    return "Volcengine Access Key ID";
  }
  if (logical.endsWith("/VOLC_SECRET_ACCESS_KEY")) {
    return "Volcengine Secret Access Key";
  }
  switch (logical) {
    case "access_key_id":
      return "AWS Access Key ID";
    case "secret_access_key":
      return "AWS Secret Access Key";
    case "session_token":
      return "AWS Session Token";
    default:
      return "API Key / Token";
  }
}

function credentialEditForBundleSecret(
  slot: string,
  secret: BundleSecretDraft,
): CredentialEdit {
  return {
    slot,
    configured: secret.configured,
    action: secret.clear
      ? "clear"
      : secret.value
        ? "replace"
        : secret.configured
          ? "keep"
          : "replace",
    value: secret.value,
  };
}

function bundleSecretFromCredentialEdit(
  edit: CredentialEdit,
): BundleSecretDraft {
  return {
    configured: edit.configured,
    value: edit.action === "replace" ? edit.value : "",
    clear: edit.action === "clear",
  };
}

function protocolLabel(protocol: string): string {
  switch (protocol) {
    case "anthropic_messages":
      return "Claude / Anthropic Messages";
    case "open_ai_chat":
      return "OpenAI Chat Completions";
    case "open_ai_responses":
      return "OpenAI Responses";
    case "gemini_native":
      return "Gemini Native";
    default:
      return protocol;
  }
}

function authSchemeLabel(scheme: string): string {
  switch (scheme) {
    case "api_key":
      return "API Key";
    case "bearer":
      return "Bearer Token";
    case "custom_header":
      return "Custom Header";
    case "query":
      return "Query parameter";
    default:
      return scheme;
  }
}

type ConfigCardId =
  | "connection"
  | "model"
  | "surfaces"
  | "diagnostics"
  | "features"
  | "accountOps";

/**
 * Where a save-blocking issue actually lives now that the form is a card stack.
 * Without this, "保存失败" scrolls to a field inside a collapsed card and nothing moves.
 */
function cardForValidationField(
  field: BundleValidationField,
  endpointScope: ProviderFamilySpec["endpointScope"],
): ConfigCardId | null {
  switch (field) {
    case "account":
    case "credential":
    case "awsRegion":
    case "timeoutSeconds":
    case "streamFirstByteTimeoutSeconds":
    case "streamIdleTimeoutSeconds":
      return "connection";
    case "endpoint":
      return endpointScope === "bundle" ? "connection" : "surfaces";
    case "modelScope":
    case "modelPolicy":
    case "upstreamModel":
      return "model";
    case "testApp":
    case "testModel":
    case "surfaceTestModel":
      return "diagnostics";
    case "surfaces":
    case "customBinding":
    case "authField":
    case "surfaceSecret":
    case "headers":
      return "surfaces";
    default:
      return null;
  }
}

function CardGapStatus({ gap }: { gap: BundleGap | null }) {
  const gapLabel = useGapLabel();
  if (!gap) return null;
  return (
    <span className="inline-flex items-center gap-1.5 rounded-full bg-destructive/10 px-2 py-0.5 text-[11px] font-medium text-destructive">
      <StatusDot tone="gap" />
      {gapLabel(gap)}
    </span>
  );
}

/**
 * One App's model decision on one line. Used both for the per-App scope and for the
 * Profile-fixed Surfaces in global scope, so the two read as the same kind of thing.
 */
function SurfaceModelRow({
  surface,
  validation,
  onChange,
}: {
  surface: BundleSurfaceEditorDraft;
  validation: BundleValidationIssue | null;
  onChange: (surface: BundleSurfaceEditorDraft) => void;
}) {
  const { t } = useTranslation();
  const profile = profileById(surface.profileId);
  if (!profile) return null;
  const allowedModelPolicies = modelPoliciesForSurface(surface);
  const configurable = allowedModelPolicies.length > 1;
  return (
    <div className="grid items-center gap-2 border-t py-2.5 first:border-t-0 sm:grid-cols-[minmax(0,7rem)_minmax(0,1fr)]">
      <div className="flex min-w-0 items-center gap-2">
        <AppLogo app={surface.app} size={15} muted={!surface.enabled} />
        <span
          className={cn(
            "truncate text-sm",
            !surface.enabled && "text-muted-foreground",
          )}
        >
          {APP_LABELS[surface.app]}
        </span>
      </div>
      {configurable ? (
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <Tabs
            value={surface.modelPolicy}
            onValueChange={(value) => {
              if (value !== "single" && value !== "passthrough") return;
              if (!allowedModelPolicies.includes(value)) return;
              onChange(
                updateSurfaceModel(
                  surface,
                  value,
                  value === "single" && !surface.upstreamModel.trim()
                    ? (profile.defaultUpstreamModel ?? "")
                    : surface.upstreamModel,
                ),
              );
            }}
          >
            <TabsList className="h-9 gap-1 border bg-muted/40 p-1">
              {allowedModelPolicies.map((policy) => (
                <TabsTrigger
                  key={policy}
                  value={policy}
                  className="h-7 min-w-0 gap-1.5 rounded-sm px-3 text-xs data-[state=active]:bg-background data-[state=active]:text-foreground"
                >
                  {policy === "single" ? (
                    <Target className="h-3.5 w-3.5 shrink-0" />
                  ) : (
                    <ArrowRightLeft className="h-3.5 w-3.5 shrink-0" />
                  )}
                  {policy === "single"
                    ? t("providerBundle.modelSingle")
                    : t("providerBundle.modelPassthrough")}
                </TabsTrigger>
              ))}
            </TabsList>
          </Tabs>
          {surface.modelPolicy === "single" ? (
            <Input
              id={bundleValidationFieldId({
                code: "surfaceUpstreamModelRequired",
                field: "upstreamModel",
                message: "",
                surface: surface.app,
              })}
              aria-label={t("serverProviderForm.model.upstreamModel")}
              placeholder={profile.defaultUpstreamModel ?? undefined}
              className={cn(
                "h-9 min-w-0 flex-1 basis-48",
                fieldErrorClass(
                  matchesBundleValidationIssue(
                    validation,
                    "upstreamModel",
                    surface.app,
                  ),
                ),
              )}
              value={surface.upstreamModel}
              onChange={(event) =>
                onChange(
                  updateSurfaceModel(
                    surface,
                    surface.modelPolicy,
                    event.target.value,
                  ),
                )
              }
            />
          ) : null}
        </div>
      ) : (
        <div className="flex min-w-0 items-center gap-2 text-sm">
          <Badge variant="secondary">
            {t("providerBundle.modelProfileFixed")}
          </Badge>
          <span className="min-w-0 truncate text-muted-foreground">
            {surface.modelPolicy === "single"
              ? surface.upstreamModel
              : t("providerBundle.modelPassthrough")}
          </span>
        </div>
      )}
    </div>
  );
}
function runtimeRowLabel(id: RuntimeSummaryRowId, t: TFunction): string {
  switch (id) {
    case "endpoint":
      return t("serverProviderForm.endpoint.url");
    case "protocol":
      return t("serverProviderForm.binding.upstreamProtocol");
    case "driver":
      return t("providerBundle.runtimeDriver", { defaultValue: "驱动" });
    case "model":
      return t("serverProviderForm.model.upstreamModel");
    case "timeout":
      return t("providerBundle.connectionTimeouts", {
        defaultValue: "连接与超时",
      });
    case "headers":
      return t("serverProviderForm.headers.title");
    case "region":
      return "AWS Region";
    default:
      return t("providerBundle.runtimeState", { defaultValue: "配置状态" });
  }
}

function SurfaceEditor({
  surface,
  validation,
  onChange,
}: {
  surface: BundleSurfaceEditorDraft;
  validation: BundleValidationIssue | null;
  onChange: (surface: BundleSurfaceEditorDraft) => void;
}) {
  const { t } = useTranslation();
  const [showRuntimeJson, setShowRuntimeJson] = useState(false);
  const profile = profileById(surface.profileId);
  if (!profile) return null;
  const customPolicy = customPolicyForProfile(profile);
  const customCredentialLabel = (() => {
    switch (surface.customBinding?.authScheme) {
      case "bearer":
        return t("providerBundle.bearerToken", {
          defaultValue: "Bearer Token",
        });
      case "api_key":
        return t("providerBundle.apiKey", { defaultValue: "API Key" });
      case "custom_header":
        return t("providerBundle.headerCredential", {
          defaultValue: "Header value",
        });
      case "query":
        return t("providerBundle.queryCredential", {
          defaultValue: "Query value",
        });
      default:
        return t("providerBundle.surfaceCredential", {
          defaultValue: "Authentication credential",
        });
    }
  })();
  const headers = surface.headers.filter((header) => !header.removed);
  const updateDriverOptions = (
    patch: Partial<BundleSurfaceEditorDraft["driverOptions"]>,
  ) =>
    onChange({
      ...surface,
      driverOptions: { ...surface.driverOptions, ...patch },
    });
  const updateHeader = (id: string, patch: Partial<BundleHeaderDraft>) =>
    onChange({
      ...surface,
      headers: surface.headers.map((item) =>
        item.id === id ? { ...item, ...patch } : item,
      ),
    });

  return (
    <div className="space-y-5 pt-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <Badge variant="outline">{profile.label}</Badge>
        <div className="flex items-center gap-2">
          <Label htmlFor={`surface-${surface.app}-enabled`}>
            {t("providerBundle.surfaceEnabled", { defaultValue: "启用 API" })}
          </Label>
          <Switch
            id={`surface-${surface.app}-enabled`}
            checked={surface.enabled}
            onCheckedChange={(enabled) => onChange({ ...surface, enabled })}
          />
        </div>
      </div>

      {profile.formComposition === "custom" && customPolicy ? (
        <div className="space-y-4">
          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2 md:col-span-2">
              <Label htmlFor={`surface-${surface.app}-endpoint`}>
                {t("serverProviderForm.endpoint.url")}
              </Label>
              <Input
                id={bundleValidationFieldId({
                  code: "endpointInvalid",
                  field: "endpoint",
                  message: "",
                  surface: surface.app,
                })}
                type="url"
                placeholder="https://api.example.com/v1"
                className={fieldErrorClass(
                  matchesBundleValidationIssue(
                    validation,
                    "endpoint",
                    surface.app,
                  ),
                )}
                value={surface.endpoint}
                onChange={(event) =>
                  onChange(updateSurfaceEndpoint(surface, event.target.value))
                }
              />
            </div>
            <div className="space-y-2">
              <Label>{t("serverProviderForm.binding.upstreamProtocol")}</Label>
              <Select
                value={surface.customBinding?.upstreamProtocol}
                onValueChange={(upstreamProtocol) =>
                  onChange({
                    ...surface,
                    customBinding: {
                      ...(surface.customBinding ?? {
                        authScheme: customPolicy
                          .authSchemes[0]! as ProviderCustomBinding["authScheme"],
                      }),
                      upstreamProtocol:
                        upstreamProtocol as ProviderCustomBinding["upstreamProtocol"],
                    },
                  })
                }
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {customPolicy.protocols.map((protocol) => (
                    <SelectItem key={protocol} value={protocol}>
                      {protocolLabel(protocol)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-2">
              <Label>{t("serverProviderForm.binding.authScheme")}</Label>
              <Select
                value={surface.customBinding?.authScheme}
                onValueChange={(authScheme) => {
                  const typedAuthScheme =
                    authScheme as ProviderCustomBinding["authScheme"];
                  const requiresField =
                    typedAuthScheme === "custom_header" ||
                    typedAuthScheme === "query";
                  onChange({
                    ...surface,
                    customBinding: {
                      ...(surface.customBinding ?? {
                        upstreamProtocol: customPolicy
                          .protocols[0]! as ProviderCustomBinding["upstreamProtocol"],
                      }),
                      authScheme: typedAuthScheme,
                    },
                    driverOptions: {
                      ...surface.driverOptions,
                      apiKeyField: requiresField
                        ? surface.driverOptions.apiKeyField ||
                          (typedAuthScheme === "query" ? "key" : "x-api-key")
                        : undefined,
                    },
                  });
                }}
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {customPolicy.authSchemes.map((scheme) => (
                    <SelectItem key={scheme} value={scheme}>
                      {authSchemeLabel(scheme)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            {surface.customBinding?.authScheme === "custom_header" ||
            surface.customBinding?.authScheme === "query" ? (
              <div className="space-y-2">
                <Label htmlFor={`surface-${surface.app}-auth-field`}>
                  {surface.customBinding.authScheme === "query"
                    ? t("providerBundle.queryParameter", {
                        defaultValue: "API Key 查询参数名",
                      })
                    : t("providerBundle.authHeader", {
                        defaultValue: "API Key Header 名称",
                      })}
                </Label>
                <Input
                  id={`surface-${surface.app}-auth-field`}
                  value={surface.driverOptions.apiKeyField ?? ""}
                  placeholder={
                    surface.customBinding.authScheme === "query"
                      ? "key"
                      : "x-api-key"
                  }
                  onChange={(event) =>
                    updateDriverOptions({ apiKeyField: event.target.value })
                  }
                />
              </div>
            ) : null}
            <div
              className={cn(
                "space-y-2",
                surface.customBinding?.authScheme !== "custom_header" &&
                  surface.customBinding?.authScheme !== "query" &&
                  "md:col-span-2",
              )}
            >
              <Label htmlFor={`surface-${surface.app}-secret`}>
                {customCredentialLabel}
              </Label>
              <SecretInput
                id={bundleValidationFieldId({
                  code: "surfaceCredentialRequired",
                  field: "surfaceSecret",
                  message: "",
                  surface: surface.app,
                })}
                className={fieldErrorClass(
                  matchesBundleValidationIssue(
                    validation,
                    "surfaceSecret",
                    surface.app,
                  ),
                )}
                value={surface.secret.value}
                placeholder={surface.secret.configured ? "••••••••" : undefined}
                onChange={(event) =>
                  onChange({
                    ...surface,
                    secret: {
                      ...surface.secret,
                      value: event.target.value,
                      clear: false,
                    },
                  })
                }
              />
            </div>
          </div>

          {/* A table, not a stack of loose input pairs: header rows are two columns of
              the same shape repeated, and a header row makes that readable at a glance. */}
          <div className="space-y-2">
            <Label>{t("serverProviderForm.headers.title")}</Label>
            <div className="overflow-hidden rounded-md border">
              {headers.length ? (
                <div className="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_2.25rem] items-center gap-2 border-b bg-muted/40 px-2 py-1.5 text-xs text-muted-foreground">
                  <span>
                    {t("providerBundle.headerName", { defaultValue: "名称" })}
                  </span>
                  <span>
                    {t("providerBundle.headerValue", { defaultValue: "值" })}
                  </span>
                  <span />
                </div>
              ) : null}
              {headers.map((header) => (
                <div
                  key={header.id}
                  className="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_2.25rem] items-center gap-2 border-b px-2 py-2"
                >
                  <Input
                    className="h-9"
                    value={header.name}
                    placeholder="x-header-name"
                    aria-label={t("providerBundle.headerName", {
                      defaultValue: "名称",
                    })}
                    onChange={(event) =>
                      updateHeader(header.id, { name: event.target.value })
                    }
                  />
                  <SecretInput
                    className="h-9"
                    value={header.value}
                    placeholder={header.configured ? "••••••••" : undefined}
                    aria-label={t("providerBundle.headerValue", {
                      defaultValue: "值",
                    })}
                    onChange={(event) =>
                      updateHeader(header.id, { value: event.target.value })
                    }
                  />
                  <Button
                    type="button"
                    size="icon"
                    variant="ghost"
                    className="h-9 w-9"
                    title={t("common.delete")}
                    aria-label={t("common.delete")}
                    onClick={() => updateHeader(header.id, { removed: true })}
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                </div>
              ))}
              <button
                type="button"
                className="flex w-full items-center gap-2 px-3 py-2 text-xs text-muted-foreground transition-colors hover:bg-muted/40 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
                onClick={() =>
                  onChange({
                    ...surface,
                    headers: [
                      ...surface.headers,
                      {
                        id: crypto.randomUUID(),
                        name: "",
                        configured: false,
                        value: "",
                        removed: false,
                      },
                    ],
                  })
                }
              >
                <Plus className="h-3.5 w-3.5" />
                {headers.length
                  ? t("common.add")
                  : t("providerBundle.headersEmpty", {
                      defaultValue: "添加自定义请求头",
                    })}
              </button>
            </div>
          </div>

          <Collapsible>
            <CollapsibleTrigger asChild>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="h-8 px-2 text-xs"
              >
                <ChevronDown className="mr-1.5 h-3.5 w-3.5" />
                {t("providerBundle.moreOptions", { defaultValue: "更多选项" })}
              </Button>
            </CollapsibleTrigger>
            <CollapsibleContent className="pt-3">
              <div className="space-y-2">
                <Label htmlFor={`surface-${surface.app}-user-agent`}>
                  {t("serverProviderForm.usage.customUserAgent")}
                </Label>
                <Input
                  id={`surface-${surface.app}-user-agent`}
                  value={surface.driverOptions.customUserAgent ?? ""}
                  onChange={(event) =>
                    updateDriverOptions({ customUserAgent: event.target.value })
                  }
                />
              </div>
            </CollapsibleContent>
          </Collapsible>
        </div>
      ) : null}

      {surface.runtime ? (
        <div className="space-y-2 rounded-md border bg-muted/20 p-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="text-xs font-medium">
              {t("providerBundle.effectiveConfig", {
                defaultValue: "有效运行配置",
              })}
            </span>
            <div className="flex items-center gap-1">
              <Button
                type="button"
                size="sm"
                variant="ghost"
                className="h-7 px-2 text-xs"
                onClick={() => setShowRuntimeJson((current) => !current)}
              >
                {showRuntimeJson
                  ? t("providerBundle.runtimeTable", { defaultValue: "摘要" })
                  : "JSON"}
              </Button>
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className="h-7 w-7"
                title={t("common.copy")}
                aria-label={t("common.copy")}
                onClick={() =>
                  void copyText(JSON.stringify(surface.runtime, null, 2))
                }
              >
                <Copy className="h-3.5 w-3.5" />
              </Button>
            </div>
          </div>
          {showRuntimeJson ? (
            <pre className="max-h-80 overflow-auto rounded-md border bg-background p-3 font-mono text-xs leading-relaxed">
              {JSON.stringify(surface.runtime, null, 2)}
            </pre>
          ) : (
            <dl className="grid gap-x-4 gap-y-1.5 text-xs sm:grid-cols-[minmax(0,7rem)_minmax(0,1fr)]">
              {runtimeSummaryRows(surface.runtime).map((row) => (
                <Fragment key={row.id}>
                  <dt className="text-muted-foreground">
                    {runtimeRowLabel(row.id, t)}
                  </dt>
                  <dd className="min-w-0 break-all font-mono">
                    {row.value == null ? (
                      <span className="font-sans text-muted-foreground">
                        {row.id === "model"
                          ? t("providerBundle.modelPassthrough")
                          : t("providerBundle.runtimeUnset", {
                              defaultValue: "默认",
                            })}
                      </span>
                    ) : row.id === "protocol" ? (
                      protocolLabel(row.value)
                    ) : (
                      row.value
                    )}
                  </dd>
                </Fragment>
              ))}
            </dl>
          )}
        </div>
      ) : null}
    </div>
  );
}

function positiveShareLimit(value: string): number | undefined {
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : undefined;
}

function defaultBundleUserPolicy(
  draft: ProviderBundleShareDraft,
): ShareUserPolicy {
  const expiryPreset = BUNDLE_SHARE_EXPIRY_PRESETS.find(
    (preset) => preset.value === draft.expiry,
  );
  return {
    parallelLimit: positiveShareLimit(draft.parallelLimit),
    tokenLimit: positiveShareLimit(draft.tokenLimit),
    tokenPeriod: "lifetime",
    expiresAt: expiryPreset
      ? Date.now() + expiryPreset.seconds * 1000
      : undefined,
  };
}

function BundleShareEditor({
  draft,
  onChange,
  ownerEmail,
  shareUrl,
  showGrokMedia,
}: {
  draft: ProviderBundleShareDraft;
  /**
   * A state setter, not a plain callback: ShareUserGrantsEditor reports a
   * grant change and its usage-edit change through two callbacks in the same
   * tick, so both updates must be applied functionally or the second one
   * overwrites the first from a stale draft.
   */
  onChange: Dispatch<SetStateAction<ProviderBundleShareDraft>>;
  ownerEmail: string;
  shareUrl?: string | null;
  showGrokMedia: boolean;
}) {
  const { t } = useTranslation();
  const routerManagedEmails = useMemo(
    () =>
      new Set(
        Object.values(draft.userGrants)
          .filter(
            (grant) =>
              grant.active !== false && grant.manager === "routerShareMarket",
          )
          .map((grant) => grant.email.trim().toLowerCase()),
      ),
    [draft.userGrants],
  );
  const protectedGrantEmails = useMemo(
    () => routerManagedEmails,
    [routerManagedEmails],
  );
  const normalizedOwnerEmail = ownerEmail.trim().toLowerCase();
  const defaultUserPolicy = useMemo(
    () => defaultBundleUserPolicy(draft),
    [draft.expiry, draft.parallelLimit, draft.tokenLimit],
  );
  const displayedUserGrants = useMemo(
    () =>
      buildShareUserGrants({
        source: draft.userGrants,
        ownerEmail: normalizedOwnerEmail,
        aclEmails: Object.values(draft.userGrants)
          .filter((grant) => grant.active !== false && grant.role === "shareto")
          .map((grant) => grant.email),
        defaultPolicy: defaultUserPolicy,
      }),
    [defaultUserPolicy, draft.userGrants, normalizedOwnerEmail],
  );
  const slugInvalid = Boolean(
    draft.subdomain.trim() && !isValidShareSlug(draft.subdomain),
  );
  const {
    onChange: updateUserGrants,
    onUsageEditsChange: updateUserUsageEdits,
  } = bundleShareGrantHandlers(onChange);
  const updateDraft = (patch: Partial<ProviderBundleShareDraft>) =>
    onChange((current) => ({ ...current, ...patch }));

  return (
    <Section
      title={t("provider.share.sectionTitle", { defaultValue: "远程分享" })}
      icon={<Share2 className="h-4 w-4" />}
      action={
        <Switch
          id="bundle-share-enabled"
          checked={draft.enabled}
          aria-label={t("provider.share.enableShare", {
            defaultValue: "启用远程分享",
          })}
          onCheckedChange={(enabled) => updateDraft({ enabled })}
        />
      }
    >
      {draft.enabled ? (
        <div className="grid gap-4 md:grid-cols-2">
          {shareUrl ? (
            <div className="space-y-2 md:col-span-2">
              <Label htmlFor="bundle-share-host">
                {t("share.subdomain", { defaultValue: "Share 完整域名" })}
              </Label>
              <div
                id="bundle-share-host"
                className="flex min-h-10 items-center gap-2 rounded-lg border border-border/60 bg-muted/30 px-3 py-2"
              >
                <p className="min-w-0 flex-1 truncate font-mono text-sm">
                  {shareUrl}
                </p>
                <Button
                  type="button"
                  size="icon"
                  variant="ghost"
                  className="h-8 w-8 shrink-0"
                  title={t("common.copy")}
                  onClick={() => void copyText(shareUrl)}
                >
                  <Copy className="h-4 w-4" />
                </Button>
              </div>
            </div>
          ) : null}
          <div className="space-y-2">
            <Label htmlFor="bundle-share-subdomain">
              {t("share.shareSlug", { defaultValue: "Share slug" })}
            </Label>
            <div className="flex items-center gap-2">
              <Input
                id="bundle-share-subdomain"
                value={draft.subdomain}
                aria-invalid={slugInvalid}
                className={cn(slugInvalid && "border-destructive")}
                onChange={(event) =>
                  updateDraft({ subdomain: event.target.value })
                }
              />
              <SubdomainGeneratorButton
                embedded={false}
                onGenerated={(subdomain) => updateDraft({ subdomain })}
                onError={(message) => toast.error(message)}
                suggest={() => shareApi.suggestShareSlug()}
              />
            </div>
            {slugInvalid ? (
              <p className="text-xs text-destructive">
                {t("share.validation.invalidShareSlug", {
                  defaultValue:
                    "Share slug must be 6-30 lowercase DNS characters without '--'",
                })}
              </p>
            ) : null}
          </div>
          <div className="space-y-2">
            <Label htmlFor="bundle-share-free-access">
              {t("share.freeAccess.label", {
                defaultValue: "公开免费使用",
              })}
            </Label>
            <div className="flex min-h-10 items-center gap-2 rounded-lg border border-border/60 px-3">
              <Checkbox
                id="bundle-share-free-access"
                checked={draft.freeAccess}
                onCheckedChange={(checked) =>
                  updateDraft({ freeAccess: checked === true })
                }
              />
              <span className="text-sm">
                {t("share.freeAccess.short", {
                  defaultValue: "任意已登录 Router 用户可免费调用",
                })}
              </span>
            </div>
          </div>
          <div className="space-y-2 md:col-span-2">
            <Label>
              {t("provider.share.description", { defaultValue: "描述" })}
            </Label>
            <Input
              value={draft.description}
              onChange={(event) =>
                updateDraft({ description: event.target.value })
              }
            />
          </div>
          {showGrokMedia ? (
            <div className="space-y-2 md:col-span-2">
              <Label>
                {t("grokOauth.shareMediaPolicy", {
                  defaultValue: "Share 媒体权限",
                })}
              </Label>
              <GrokFeatureOptions
                values={{
                  grokImageGenerationEnabled:
                    draft.grokMediaPolicy.imageGenerationEnabled,
                  grokImageEditEnabled: draft.grokMediaPolicy.imageEditEnabled,
                  grokVideoGenerationEnabled:
                    draft.grokMediaPolicy.videoGenerationEnabled,
                }}
                onChange={(key, enabled) => {
                  const field =
                    key === "grokImageGenerationEnabled"
                      ? "imageGenerationEnabled"
                      : key === "grokImageEditEnabled"
                        ? "imageEditEnabled"
                        : "videoGenerationEnabled";
                  updateDraft({
                    grokMediaPolicy: {
                      ...draft.grokMediaPolicy,
                      [field]: enabled,
                    },
                  });
                }}
              />
            </div>
          ) : null}
          <ShareUserGrantsEditor
            value={displayedUserGrants}
            ownerEmail={normalizedOwnerEmail}
            defaultPolicy={defaultUserPolicy}
            protectedEmails={protectedGrantEmails}
            usageEdits={draft.userUsageEdits}
            onUsageEditsChange={updateUserUsageEdits}
            onChange={updateUserGrants}
          />
          <div className="grid gap-4 md:col-span-2 md:grid-cols-3">
            <div className="space-y-2">
              <Label>
                {t("provider.share.tokenLimit", { defaultValue: "Token 限额" })}
              </Label>
              <Input
                type="number"
                min={0}
                placeholder={t("share.unlimited", { defaultValue: "无上限" })}
                value={draft.tokenLimit}
                onChange={(event) =>
                  updateDraft({ tokenLimit: event.target.value })
                }
              />
              <div className="flex flex-wrap gap-1.5">
                <Button
                  type="button"
                  variant={draft.tokenLimit === "" ? "secondary" : "outline"}
                  size="sm"
                  className="h-7 px-2 text-xs"
                  onClick={() => updateDraft({ tokenLimit: "" })}
                >
                  {t("share.unlimited", { defaultValue: "无上限" })}
                </Button>
                {SHARE_TOKEN_PRESETS.map((preset) => (
                  <Button
                    key={preset}
                    type="button"
                    variant={
                      draft.tokenLimit === String(preset)
                        ? "secondary"
                        : "outline"
                    }
                    size="sm"
                    className="h-7 px-2 text-xs"
                    onClick={() => updateDraft({ tokenLimit: String(preset) })}
                  >
                    {preset.toLocaleString()}
                  </Button>
                ))}
              </div>
            </div>
            <div className="space-y-2">
              <Label>
                {t("provider.share.parallelLimit", {
                  defaultValue: "并发限额",
                })}
              </Label>
              <Input
                type="number"
                min={1}
                placeholder={t("share.unlimited", { defaultValue: "无上限" })}
                value={draft.parallelLimit}
                onChange={(event) =>
                  updateDraft({ parallelLimit: event.target.value })
                }
              />
              <div className="flex flex-wrap gap-1.5">
                <Button
                  type="button"
                  variant={draft.parallelLimit === "" ? "secondary" : "outline"}
                  size="sm"
                  className="h-7 px-2 text-xs"
                  onClick={() => updateDraft({ parallelLimit: "" })}
                >
                  {t("share.unlimited", { defaultValue: "无上限" })}
                </Button>
                <Button
                  type="button"
                  variant={
                    draft.parallelLimit === String(DEFAULT_PARALLEL_LIMIT)
                      ? "secondary"
                      : "outline"
                  }
                  size="sm"
                  className="h-7 px-2 text-xs"
                  onClick={() =>
                    updateDraft({
                      parallelLimit: String(DEFAULT_PARALLEL_LIMIT),
                    })
                  }
                >
                  {DEFAULT_PARALLEL_LIMIT}
                </Button>
              </div>
            </div>
            <div className="space-y-2">
              <Label>
                {t("provider.share.expiry", { defaultValue: "有效期" })}
              </Label>
              <Select
                value={draft.expiry}
                onValueChange={(expiry) =>
                  updateDraft({
                    expiry: expiry as ProviderBundleShareDraft["expiry"],
                  })
                }
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="permanent">
                    {t("share.expiry.permanent", { defaultValue: "永久有效" })}
                  </SelectItem>
                  {BUNDLE_SHARE_EXPIRY_PRESETS.map((preset) => (
                    <SelectItem key={preset.value} value={preset.value}>
                      {t(preset.labelKey)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>
        </div>
      ) : null}
    </Section>
  );
}

export function ProviderBundleEditor({
  bundle,
  duplicate = false,
  initialSection,
  onCancel,
  onSaved,
  onOpenShareSettings,
}: ProviderBundleEditorProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const initialFamily = recommendedFamily();
  const [draft, setDraft] = useState<ProviderBundleEditorDraft>(() =>
    bundle
      ? duplicate
        ? duplicateProviderBundleDraft(bundle)
        : editProviderBundleDraft(bundle)
      : createDraftForSelectedFamily(initialFamily),
  );
  const persisted =
    (Boolean(bundle) && !duplicate) || draft.expectedRevision !== undefined;
  const family = familyById(draft.familyId) ?? initialFamily;
  const customRecipes = customRecipesForFamily(family);
  const activeCustomRecipe = customRecipes.find((recipe) =>
    customRecipeMatchesBundleDraft(draft, recipe),
  );
  const identityEditable = providerBundleIdentityEditable(family);
  const [activeApp, setActiveApp] = useState<CoreProviderApp>(
    draft.surfaces[0]?.app ?? "claude",
  );
  const [createStep, setCreateStep] = useState<CreateStep>(
    persisted ? "supply" : "family",
  );
  const [highestCreateStep, setHighestCreateStep] = useState<CreateStep>(
    persisted ? "share" : "family",
  );
  const [validation, setValidation] = useState<BundleValidationIssue | null>(
    null,
  );
  const [saving, setSaving] = useState(false);
  const [requestDefaults, setRequestDefaults] =
    useState<ProviderRequestDefaults | null>(null);
  const [healthCheckConfig, setHealthCheckConfig] =
    useState<ProviderHealthCheckConfig | null>(null);
  const [modelScopeConfirmOpen, setModelScopeConfirmOpen] = useState(false);
  // The three cards that always hold a decision start open; the rest are opt-in, so a
  // fresh provider is a short page instead of six equally loud blocks of whitespace.
  const [openCards, setOpenCards] = useState<Record<ConfigCardId, boolean>>({
    connection: true,
    model: true,
    surfaces: true,
    diagnostics: false,
    features: false,
    accountOps: false,
  });
  const [revealedCredentialValues, setRevealedCredentialValues] = useState<
    Record<string, string>
  >({});
  const [credentialRevealStatuses, setCredentialRevealStatuses] = useState<
    Record<string, CredentialRevealStatus>
  >({});
  const credentialRevealGeneration = useRef(0);
  const shareSectionRef = useRef<HTMLDivElement>(null);
  const sharesQuery = useSharesQuery();
  const clientTunnelQuery = useClientTunnelQuery();
  const existingShare = shareForBundle(sharesQuery.data, draft.id);
  const [shareDraft, setShareDraft] = useState<ProviderBundleShareDraft>(() =>
    createBundleShareDraft(existingShare),
  );
  const draftBaselineRef = useRef(stableStringify(draft));
  const shareBaselineRef = useRef(stableStringify(shareDraft));
  const shareDirtyRef = useRef(false);
  const shareDirty = stableStringify(shareDraft) !== shareBaselineRef.current;
  shareDirtyRef.current = shareDirty;
  const dirty =
    stableStringify(draft) !== draftBaselineRef.current || shareDirty;
  const closeGuard = useUnsavedChangesGuard({
    active: true,
    dirty: dirty && !saving,
    onClose: onCancel,
  });
  const accountsQuery = useManagedAccountsQuery();
  const credentialProfile = profileById(family.credentialProfileId);
  const credentialSourceApp = credentialProfile?.app;
  const configuredCredentialSlotsKey = Object.entries(draft.secrets)
    .filter(([, secret]) => secret.configured)
    .map(([slot]) => slot)
    .sort()
    .join("\n");
  const allowedModelPolicies = modelPoliciesForFamily(family);
  const defaultSharedModel = defaultUpstreamModelForFamily(family);
  const perAppModelPolicySupported = supportsPerAppModelPolicy(family);
  const perAppModelPolicyRequired = requiresPerAppModelPolicy(family);
  const fixedModelSurfaces = draft.surfaces.filter(
    (surface) => modelPoliciesForSurface(surface).length === 1,
  );
  const enabledTestApps = BUNDLE_TEST_APP_ORDER.filter((app) =>
    draft.surfaces.some((surface) => surface.app === app && surface.enabled),
  );
  const managedProviderType =
    credentialProfile?.credentialPolicy.mode === "managed_account"
      ? credentialProfile.credentialPolicy.accountProviderType
      : undefined;
  const accounts = useMemo(
    () =>
      accountsQuery.data?.filter(
        (account) =>
          managedProviderType &&
          normalizeManagedAuthProvider(account.provider) ===
            normalizeManagedAuthProvider(managedProviderType),
      ) ?? [],
    [accountsQuery.data, managedProviderType],
  );
  const commonEndpointEditable =
    family.endpointScope === "bundle" &&
    family.surfaces.some((surface) => {
      const profile = profileById(surface.profileId);
      return profile ? profileAllowsEndpointEditing(profile) : false;
    });
  const codexDriverOptions = draft.surfaces.some((surface) => {
    const profile = profileById(surface.profileId);
    return profile
      ? driverForProfile(profile)?.driverId === "oauth.openai_codex"
      : false;
  });
  const codexControlTarget = codexDriverOptions
    ? resolvePersistedCodexControlTarget(bundle, draft, duplicate)
    : null;
  const codexControlAccount = codexControlTarget
    ? accounts.find((account) => account.id === codexControlTarget.accountId)
    : undefined;
  const codexControlWorkspaceId =
    codexControlAccount?.selected_workspace_id ??
    codexControlAccount?.workspaces?.[0]?.id;
  const grokDriverOptions = draft.surfaces.some((surface) => {
    const profile = profileById(surface.profileId);
    return profile
      ? driverForProfile(profile)?.driverId === "oauth.grok_responses"
      : false;
  });
  const ownerEmail =
    existingShare?.ownerEmail ??
    clientTunnelQuery.data?.config.ownerEmail ??
    "";

  useEffect(() => {
    let active = true;
    void Promise.all([
      providersApi.getRequestDefaults(),
      providersApi.getHealthCheckConfig(),
    ])
      .then(([request, health]) => {
        if (active) {
          setRequestDefaults(request);
          setHealthCheckConfig(health);
        }
      })
      .catch((error) => {
        if (active) {
          setRequestDefaults(null);
          setHealthCheckConfig(null);
          toast.error(error instanceof Error ? error.message : String(error));
        }
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    if (credentialProfile?.credentialPolicy.mode !== "managed_account") return;
    if (draft.accountId) {
      if (draft.accountGeneration != null) return;
      const account = accounts.find((item) => item.id === draft.accountId);
      if (account?.authIdentityGeneration == null) return;
      setDraft((current) => {
        if (
          current.accountId !== account.id ||
          current.accountGeneration != null
        ) {
          return current;
        }
        return {
          ...current,
          accountGeneration: account.authIdentityGeneration,
        };
      });
      return;
    }
    const preferred = preferredManagedAccount(accounts);
    if (!preferred) return;
    setDraft((current) => {
      if (current.accountId) return current;
      return {
        ...current,
        accountId: preferred.id,
        accountGeneration: preferred.authIdentityGeneration,
      };
    });
  }, [
    accounts,
    credentialProfile?.credentialPolicy.mode,
    draft.accountGeneration,
    draft.accountId,
  ]);

  useEffect(() => {
    const generation = credentialRevealGeneration.current + 1;
    credentialRevealGeneration.current = generation;
    setRevealedCredentialValues({});
    setCredentialRevealStatuses({});

    const slots = configuredCredentialSlotsKey
      ? configuredCredentialSlotsKey.split("\n")
      : [];
    if (!persisted || !credentialSourceApp || slots.length === 0) return;

    setCredentialRevealStatuses(
      Object.fromEntries(slots.map((slot) => [slot, "loading" as const])),
    );
    for (const slot of slots) {
      void providersApi
        .getCredential(credentialSourceApp, draft.id, slot)
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
  }, [
    configuredCredentialSlotsKey,
    credentialSourceApp,
    draft.expectedRevision,
    draft.id,
    persisted,
  ]);

  useEffect(() => {
    const nextShareDraft = createBundleShareDraft(existingShare);
    if (shareDirtyRef.current) return;
    const nextFingerprint = stableStringify(nextShareDraft);
    if (nextFingerprint === shareBaselineRef.current) return;
    shareBaselineRef.current = nextFingerprint;
    setShareDraft(nextShareDraft);
  }, [existingShare?.id, existingShare?.configRevision]);

  useEffect(() => {
    if (initialSection !== "share") return;
    const frame = requestAnimationFrame(() => {
      shareSectionRef.current?.scrollIntoView?.({
        behavior: "smooth",
        block: "start",
      });
    });
    return () => cancelAnimationFrame(frame);
  }, [initialSection]);

  useEffect(() => {
    if (!shareDraft.enabled || shareDraft.subdomain.trim() || existingShare)
      return;
    let active = true;
    void shareApi
      .suggestShareSlug()
      .then((result) => {
        if (active)
          setShareDraft((current) => ({
            ...current,
            subdomain: result.subdomain,
          }));
      })
      .catch((error) => {
        if (active) {
          toast.error(error instanceof Error ? error.message : String(error));
        }
      });
    return () => {
      active = false;
    };
  }, [existingShare, shareDraft.enabled, shareDraft.subdomain]);

  const applyFamily = (familyId: string) => {
    if (persisted) return;
    const next = familyById(familyId);
    if (!next) return;
    const nextDraft = createDraftForSelectedFamily(next);
    setDraft(nextDraft);
    setActiveApp(nextDraft.surfaces[0]?.app ?? "claude");
    setShareDraft(createBundleShareDraft());
    setValidation(null);
    setCreateStep("family");
    setHighestCreateStep("family");
  };

  const changeFamily = (familyId: string) => {
    if (persisted || familyId === draft.familyId) return;
    applyFamily(familyId);
  };

  const validationMessage = (issue: BundleValidationIssue): string =>
    t(`providerBundle.validation.${issue.code}`, {
      defaultValue: issue.message,
      app: issue.surface ? APP_LABELS[issue.surface] : undefined,
    });

  const setCardOpen = (card: ConfigCardId, open: boolean) =>
    setOpenCards((current) => ({ ...current, [card]: open }));

  const revealCard = (card: ConfigCardId) => {
    setCardOpen(card, true);
    scrollToWhenRendered(`bundle-card-${card}`, "start");
  };

  const focusSurface = (app: CoreProviderApp) => {
    setActiveApp(app);
    revealCard("surfaces");
  };

  const focusIssue = (issue: BundleValidationIssue) => {
    if (issue.surface) setActiveApp(issue.surface);
    if (!persisted) {
      setCreateStep(issue.field === "family" ? "family" : "supply");
    }
    // A collapsed card keeps its content unmounted, so opening it has to happen before
    // there is anything to scroll to.
    const card = cardForValidationField(issue.field, family.endpointScope);
    if (card) setCardOpen(card, true);
    scrollToWhenRendered(bundleValidationFieldId(issue), "center");
  };

  const selectCreateStep = (step: CreateStep) => {
    if (!canVisitCreateStep(step, highestCreateStep)) return;
    setCreateStep(step);
  };

  const advanceCreateStep = () => {
    const next = nextCreateStep(createStep);
    if (!next) return;
    setHighestCreateStep((current) => unlockCreateStep(current, next));
    setCreateStep(next);
  };

  const retreatCreateStep = () => {
    const previous = previousCreateStep(createStep);
    if (previous) setCreateStep(previous);
  };

  const retryCredentialReveal = async (slot: string) => {
    if (!persisted || !credentialSourceApp) return;
    const generation = credentialRevealGeneration.current;
    setCredentialRevealStatuses((current) => ({
      ...current,
      [slot]: "loading",
    }));
    try {
      const value = await providersApi.getCredential(
        credentialSourceApp,
        draft.id,
        slot,
      );
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

  const updateSurface = (next: BundleSurfaceEditorDraft) =>
    setDraft((current) =>
      normalizeBundleTestApp({
        ...current,
        surfaces: current.surfaces.map((surface) =>
          surface.app === next.app ? next : surface,
        ),
      }),
    );

  const setDriverOption = (key: CodexFeatureOptionKey, checked: boolean) =>
    setDraft((current) => ({
      ...current,
      surfaces: current.surfaces.map((surface) => {
        const profile = profileById(surface.profileId);
        if (driverForProfile(profile!)?.driverId !== "oauth.openai_codex")
          return surface;
        return {
          ...surface,
          driverOptions: { ...surface.driverOptions, [key]: checked },
        };
      }),
    }));

  const setGrokDriverOption = (key: GrokFeatureOptionKey, checked: boolean) =>
    setDraft((current) => ({
      ...current,
      surfaces: current.surfaces.map((surface) => {
        const profile = profileById(surface.profileId);
        if (driverForProfile(profile!)?.driverId !== "oauth.grok_responses")
          return surface;
        return {
          ...surface,
          driverOptions: { ...surface.driverOptions, [key]: checked },
        };
      }),
    }));

  const submit = async () => {
    const nextValidation = firstBundleValidationIssue(draft);
    setValidation(nextValidation);
    if (nextValidation) {
      toast.error(validationMessage(nextValidation));
      focusIssue(nextValidation);
      return;
    }
    if (
      shareDraft.enabled &&
      shareDraft.subdomain.trim() &&
      !isValidShareSlug(shareDraft.subdomain)
    ) {
      if (!persisted) setCreateStep("share");
      toast.error(
        t("share.validation.invalidShareSlug", {
          defaultValue:
            "Share slug must be 6-30 lowercase DNS characters without '--'",
        }),
      );
      return;
    }
    if (
      Object.values(shareDraft.userGrants).some(
        (grant) => !isValidShareEmail(grant.email),
      )
    ) {
      if (!persisted) setCreateStep("share");
      toast.error(
        t("share.validation.invalidEmail", {
          defaultValue: "邮箱格式无效",
        }),
      );
      return;
    }
    setSaving(true);
    try {
      const saved = await providersApi.upsertBundle(
        toProviderBundleWriteDraft(draft),
      );
      const savedDraft = editProviderBundleDraft(saved);
      setDraft(savedDraft);
      draftBaselineRef.current = stableStringify(savedDraft);
      const savedShare = await saveBundleShare(
        saved.id,
        shareDraft,
        existingShare,
      );
      const savedShareDraft = savedShare
        ? createBundleShareDraft(savedShare)
        : shareDraft;
      shareBaselineRef.current = stableStringify(savedShareDraft);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["provider-bundles"] }),
        queryClient.invalidateQueries({ queryKey: shareKeys.list() }),
        queryClient.invalidateQueries({ queryKey: managedAccountKeys.all }),
      ]);
      toast.success(
        t("providerBundle.saved", { defaultValue: "供应商已保存" }),
      );
      onSaved(saved);
    } catch (error) {
      await Promise.allSettled([
        queryClient.invalidateQueries({ queryKey: ["provider-bundles"] }),
        queryClient.invalidateQueries({ queryKey: shareKeys.list() }),
        queryClient.invalidateQueries({ queryKey: managedAccountKeys.all }),
      ]);
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setSaving(false);
    }
  };

  const shareUrl = existingShare?.tunnelUrl ?? existingShare?.subdomain;
  const showFamilyStep = !persisted && createStep === "family";
  const showSupplyStep = persisted || createStep === "supply";
  const showShareStep = persisted || createStep === "share";
  const showSurfaceTabs =
    draft.surfaces.length > 1 || family.endpointScope === "surface";
  const activeSurface =
    draft.surfaces.find((surface) => surface.app === activeApp) ??
    draft.surfaces[0];

  const readiness = bundleReadiness(draft);
  const surfaceTone = (app: CoreProviderApp) => {
    const item = readiness.surfaces.find((entry) => entry.app === app);
    return item ? readinessTone(item) : "off";
  };
  // Each card only claims the gaps it can actually fix: a missing per-App model is the
  // model card's problem even though it is a Surface-level value.
  const modelGap = readiness.surfaces.some(
    (surface) => surface.ownGap === "model",
  );
  const surfaceGapCount = readiness.surfaces.filter(
    (surface) => surface.ownGap && surface.ownGap !== "model",
  ).length;
  const selectedAccount = accounts.find(
    (account) => account.id === draft.accountId,
  );
  const connectionSummary =
    [
      credentialProfile?.credentialPolicy.mode === "managed_account"
        ? (selectedAccount?.email ?? selectedAccount?.login ?? null)
        : null,
      commonEndpointEditable ? draft.endpoint.trim() || null : null,
    ]
      .filter(Boolean)
      .join(" · ") || null;
  const modelSummary =
    draft.modelPolicyScope === "per_app"
      ? t("providerBundle.modelScopePerApp")
      : draft.modelPolicy === "single"
        ? draft.upstreamModel.trim() || t("providerBundle.modelSingle")
        : t("providerBundle.modelPassthrough");
  const surfacesSummary =
    draft.surfaces
      .filter((surface) => surface.enabled)
      .map((surface) => APP_LABELS[surface.app])
      .join(" · ") || t("providerBundle.surfaceOff", { defaultValue: "关" });
  const diagnosticsSummary = [
    APP_LABELS[draft.testApp],
    draft.testModel.trim() || healthCheckConfig?.testModels[draft.testApp],
  ]
    .filter(Boolean)
    .join(" · ");
  const surfaceTestModelOverrides = enabledTestApps.filter((app) =>
    draft.surfaceTestModels[app]?.trim(),
  ).length;
  const enabledFeatureCount = [
    "codexFastMode",
    "codexImageGenerationEnabled",
    "codexWebsocketEnabled",
    "grokImageGenerationEnabled",
    "grokImageEditEnabled",
    "grokVideoGenerationEnabled",
  ].filter((key) =>
    draft.surfaces.some(
      (surface) =>
        surface.driverOptions[
          key as keyof BundleSurfaceEditorDraft["driverOptions"]
        ] === true,
    ),
  ).length;
  const featuresSummary = enabledFeatureCount
    ? t("providerBundle.featuresEnabledCount", {
        defaultValue: "已开启 {{count}} 项",
        count: enabledFeatureCount,
      })
    : t("providerBundle.featuresAllOff", { defaultValue: "全部关闭" });

  return (
    <div className="mx-auto flex w-full max-w-5xl flex-col gap-6 pb-24">
      <div className="flex items-center gap-3">
        <Button
          type="button"
          size="icon"
          variant="outline"
          onClick={closeGuard.requestClose}
          title={t("common.back")}
        >
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <div className="min-w-0 flex-1">
          <h1 className="truncate text-lg font-semibold">
            {persisted
              ? t("providerBundle.edit", { defaultValue: "编辑供应商" })
              : t("providerBundle.create", { defaultValue: "新建供应商" })}
          </h1>
          <div className="mt-1 flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
            {family.surfaces.map((surface) => (
              <AppLogo key={surface.app} app={surface.app} />
            ))}
            <span className="truncate">{familyLabel(family, t)}</span>
            {dirty ? (
              <Badge variant="outline">
                {t("providerBundle.unsaved", { defaultValue: "未保存" })}
              </Badge>
            ) : null}
          </div>
        </div>
      </div>

      {!persisted ? (
        <div
          role="tablist"
          aria-label={t("providerBundle.stepNavigation", {
            defaultValue: "Provider creation steps",
          })}
          className="grid grid-cols-3 gap-2"
        >
          {CREATE_STEPS.map((step, index) => (
            <button
              key={step}
              id={`bundle-tab-${step}`}
              type="button"
              role="tab"
              aria-selected={createStep === step}
              aria-controls={`bundle-section-${step}`}
              aria-disabled={!canVisitCreateStep(step, highestCreateStep)}
              disabled={!canVisitCreateStep(step, highestCreateStep)}
              className={cn(
                "min-w-0 rounded-md border px-2 py-2 text-center text-xs font-medium sm:px-3 sm:text-sm",
                createStep === step
                  ? "border-primary bg-primary/5 text-foreground"
                  : "border-border text-muted-foreground",
                !canVisitCreateStep(step, highestCreateStep) &&
                  "cursor-not-allowed opacity-50",
              )}
              onClick={() => selectCreateStep(step)}
            >
              {index + 1}.{" "}
              {step === "family"
                ? t("providerBundle.stepFamily", { defaultValue: "选择类型" })
                : step === "supply"
                  ? t("providerBundle.stepSupply", { defaultValue: "配置" })
                  : t("providerBundle.stepShare", { defaultValue: "远程分享" })}
            </button>
          ))}
        </div>
      ) : (
        <div className="flex flex-wrap gap-2 text-xs">
          {(
            [
              [
                "supply",
                t("providerBundle.stepSupply", { defaultValue: "配置" }),
              ],
              [
                "share",
                t("providerBundle.stepShare", { defaultValue: "远程分享" }),
              ],
            ] as const
          ).map(([section, label]) => (
            <Button
              key={section}
              type="button"
              size="sm"
              variant="outline"
              onClick={() =>
                document
                  .getElementById(`bundle-section-${section}`)
                  ?.scrollIntoView({ behavior: "smooth", block: "start" })
              }
            >
              {label}
            </Button>
          ))}
        </div>
      )}

      <div className="space-y-6">
        {showFamilyStep ? (
          <div
            id="bundle-section-family"
            role="tabpanel"
            aria-labelledby="bundle-tab-family"
          >
            <Section
              title={t("providerBundle.family", { defaultValue: "供应商类型" })}
            >
              <FamilyPicker
                selectedFamilyId={draft.familyId}
                onSelect={changeFamily}
                onAutoSelect={applyFamily}
              />
            </Section>
          </div>
        ) : null}

        {showSupplyStep ? (
          <div
            id="bundle-section-supply"
            role="tabpanel"
            aria-labelledby="bundle-tab-supply"
            className="space-y-4"
          >
            <BundleReadinessBar readiness={readiness} onSelect={focusSurface} />

            <Section
              title={t("providerBundle.basic", { defaultValue: "基本信息" })}
            >
              <div className="grid gap-4 md:grid-cols-2">
                <div className="space-y-2 md:col-span-2">
                  <div className="flex items-center justify-center">
                    <ProviderIconControl
                      icon={draft.icon}
                      iconColor={draft.iconColor}
                      providerName={draft.name}
                      onChange={(icon, iconColor) =>
                        setDraft((current) => ({
                          ...current,
                          icon,
                          iconColor,
                        }))
                      }
                    />
                  </div>
                  {persisted ? (
                    <p className="text-center text-sm text-muted-foreground">
                      {familyLabel(family, t)}
                    </p>
                  ) : null}
                  {duplicate ? (
                    <p className="text-center text-sm text-muted-foreground">
                      {t("providerBundle.duplicateSecretsCleared", {
                        defaultValue:
                          "Credentials are not copied. Enter them again before saving.",
                      })}
                    </p>
                  ) : null}
                </div>
                {customRecipes.length ? (
                  <div className="space-y-2 md:col-span-2">
                    <Label>{t("providerBundle.quickPreset")}</Label>
                    <Select
                      value={activeCustomRecipe?.recipeId ?? ""}
                      onValueChange={(recipeId) => {
                        const recipe = customRecipes.find(
                          (candidate) => candidate.recipeId === recipeId,
                        );
                        if (!recipe) return;
                        setDraft((current) =>
                          applyCustomRecipeToBundleDraft(current, recipe),
                        );
                        const recipeProfile = profileById(recipe.profileId);
                        if (recipeProfile) setActiveApp(recipeProfile.app);
                      }}
                    >
                      <SelectTrigger>
                        <SelectValue
                          placeholder={t("providerBundle.manualConfiguration")}
                        />
                      </SelectTrigger>
                      <SelectContent>
                        {customRecipes.map((recipe) => (
                          <SelectItem
                            key={recipe.recipeId}
                            value={recipe.recipeId}
                          >
                            {t(recipe.labelKey, { defaultValue: recipe.label })}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                ) : null}
                <div className="space-y-2">
                  <Label>{t("serverProviderForm.basic.name")}</Label>
                  <Input
                    id={bundleValidationFieldId({
                      code: "nameRequired",
                      field: "name",
                      message: "",
                    })}
                    value={draft.name}
                    className={cn(
                      fieldErrorClass(
                        matchesBundleValidationIssue(validation, "name"),
                      ),
                      !identityEditable &&
                        "cursor-default bg-muted/40 text-muted-foreground",
                    )}
                    readOnly={!identityEditable}
                    onChange={(event) =>
                      identityEditable &&
                      setDraft({ ...draft, name: event.target.value })
                    }
                  />
                </div>
                <div className="space-y-2">
                  <Label>{t("serverProviderForm.basic.website")}</Label>
                  <Input
                    type="url"
                    value={draft.websiteUrl}
                    readOnly={!identityEditable}
                    className={cn(
                      !identityEditable &&
                        "cursor-default bg-muted/40 text-muted-foreground",
                    )}
                    onChange={(event) =>
                      identityEditable &&
                      setDraft({ ...draft, websiteUrl: event.target.value })
                    }
                  />
                </div>
                <div className="space-y-2 md:col-span-2">
                  <Label>{t("serverProviderForm.basic.notes")}</Label>
                  <Textarea
                    rows={2}
                    value={draft.notes}
                    onChange={(event) =>
                      setDraft({ ...draft, notes: event.target.value })
                    }
                  />
                </div>
              </div>
            </Section>

            <ConfigCard
              id="bundle-card-connection"
              title={t("providerBundle.cardConnection", {
                defaultValue: "上游连接",
              })}
              icon={<KeyRound className="h-4 w-4 text-muted-foreground" />}
              summary={connectionSummary}
              status={<CardGapStatus gap={readiness.connection} />}
              open={openCards.connection}
              onOpenChange={(open) => setCardOpen("connection", open)}
            >
              <div className="space-y-5">
                {credentialProfile?.credentialPolicy.mode ===
                "managed_account" ? (
                  <div
                    id={bundleValidationFieldId({
                      code: "accountRequired",
                      field: "account",
                      message: "",
                    })}
                    className={cn(
                      matchesBundleValidationIssue(validation, "account") &&
                        "rounded-md border border-destructive p-3",
                    )}
                  >
                    <ManagedAccountSection
                      providerType={
                        credentialProfile.credentialPolicy.accountProviderType
                      }
                      selectedAccountId={draft.accountId || null}
                      onAccountSelect={(accountId) => {
                        const account = accounts.find(
                          (item) => item.id === accountId,
                        );
                        setDraft((current) => ({
                          ...current,
                          accountId: accountId ?? "",
                          accountGeneration: account?.authIdentityGeneration,
                        }));
                      }}
                    />
                    {matchesBundleValidationIssue(validation, "account") &&
                    validation ? (
                      <p className="mt-2 text-xs text-destructive">
                        {validationMessage(validation)}
                      </p>
                    ) : null}
                  </div>
                ) : null}

                {familyCredentialSlots(family).length ? (
                  <div className="grid gap-4 md:grid-cols-2">
                    {familyCredentialSlots(family).map(
                      ({ logical, pointer }) => {
                        const actualPointer =
                          Object.keys(draft.secrets).find(
                            (slot) =>
                              slot === pointer ||
                              slot.endsWith(
                                pointer.slice(pointer.lastIndexOf("/")),
                              ),
                          ) ?? pointer;
                        const secret = draft.secrets[actualPointer] ?? {
                          configured: false,
                          value: "",
                          clear: false,
                        };
                        const edit = credentialEditForBundleSecret(
                          actualPointer,
                          secret,
                        );
                        const revealedValue =
                          revealedCredentialValues[actualPointer];
                        const revealStatus =
                          credentialRevealStatuses[actualPointer] ?? "idle";
                        const value = credentialInputValue(edit, revealedValue);
                        const loadingCurrent =
                          edit.configured &&
                          edit.action === "keep" &&
                          revealStatus === "loading";
                        const currentRevealFailed =
                          edit.configured &&
                          edit.action === "keep" &&
                          revealStatus === "error";
                        return (
                          <div key={logical} className="space-y-2">
                            <div className="flex min-h-6 items-center justify-between gap-2">
                              <Label>{fieldLabel(logical)}</Label>
                              {loadingCurrent ? (
                                <LoaderCircle className="h-4 w-4 animate-spin text-muted-foreground" />
                              ) : currentRevealFailed ? (
                                <Button
                                  type="button"
                                  size="icon"
                                  variant="ghost"
                                  className="h-6 w-6"
                                  title={t("common.retry")}
                                  aria-label={t("common.retry")}
                                  onClick={() =>
                                    void retryCredentialReveal(actualPointer)
                                  }
                                >
                                  <RefreshCw className="h-3.5 w-3.5" />
                                </Button>
                              ) : null}
                            </div>
                            <SecretInput
                              id={bundleValidationFieldId({
                                code: "credentialRequired",
                                field: "credential",
                                message: "",
                              })}
                              className={fieldErrorClass(
                                matchesBundleValidationIssue(
                                  validation,
                                  "credential",
                                ),
                              )}
                              value={value}
                              disabled={
                                loadingCurrent || edit.action === "clear"
                              }
                              autoComplete="new-password"
                              placeholder={
                                loadingCurrent
                                  ? t("serverProviderForm.credentials.loading")
                                  : currentRevealFailed
                                    ? t(
                                        "serverProviderForm.credentials.loadFailedPlaceholder",
                                      )
                                    : t(
                                        "serverProviderForm.credentials.placeholder",
                                      )
                              }
                              onChange={(event) => {
                                const next = updateCredentialInput(
                                  edit,
                                  event.target.value,
                                  {
                                    optional: logical === "session_token",
                                    revealedValue,
                                    revealStatus,
                                  },
                                );
                                setDraft((current) => ({
                                  ...current,
                                  secrets: {
                                    ...current.secrets,
                                    [actualPointer]:
                                      bundleSecretFromCredentialEdit(next),
                                  },
                                }));
                              }}
                            />
                            {currentRevealFailed ? (
                              <p className="text-xs text-destructive">
                                {t("serverProviderForm.credentials.loadFailed")}
                              </p>
                            ) : null}
                          </div>
                        );
                      },
                    )}
                    {credentialProfile?.formComposition === "aws" ? (
                      <div className="space-y-2">
                        <Label>AWS Region</Label>
                        <Input
                          id={bundleValidationFieldId({
                            code: "awsRegionRequired",
                            field: "awsRegion",
                            message: "",
                          })}
                          className={fieldErrorClass(
                            matchesBundleValidationIssue(
                              validation,
                              "awsRegion",
                            ),
                          )}
                          value={draft.awsRegion}
                          onChange={(event) =>
                            setDraft({
                              ...draft,
                              awsRegion: event.target.value,
                            })
                          }
                        />
                      </div>
                    ) : null}
                  </div>
                ) : null}

                {commonEndpointEditable ? (
                  <div className="space-y-2">
                    <Label htmlFor="provider-bundle-endpoint">
                      {t("serverProviderForm.endpoint.url")}
                    </Label>
                    <Input
                      id={bundleValidationFieldId({
                        code: "endpointInvalid",
                        field: "endpoint",
                        message: "",
                      })}
                      type="url"
                      placeholder="https://api.example.com/v1"
                      className={fieldErrorClass(
                        matchesBundleValidationIssue(validation, "endpoint"),
                      )}
                      value={draft.endpoint}
                      onChange={(event) =>
                        setDraft({ ...draft, endpoint: event.target.value })
                      }
                    />
                  </div>
                ) : null}

                {/* The address lives on each Surface for this family. Say so and point at
                    it, rather than leaving a connection card that looks half-empty. */}
                {family.endpointScope === "surface" ? (
                  <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-dashed px-3 py-2.5">
                    <span className="text-xs text-muted-foreground">
                      {t("providerBundle.endpointPerSurfaceHint", {
                        defaultValue:
                          "该供应商的 API 地址与凭据在每个应用接口里单独配置。",
                      })}
                    </span>
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      className="h-7 px-2 text-xs"
                      onClick={() => revealCard("surfaces")}
                    >
                      {t("providerBundle.goToSurfaces", {
                        defaultValue: "前往应用接口",
                      })}
                      <ArrowRight className="ml-1 h-3.5 w-3.5" />
                    </Button>
                  </div>
                ) : null}

                <div className="space-y-2 border-t pt-4">
                  <div className="flex flex-wrap items-baseline justify-between gap-2">
                    <Label>
                      {t("providerBundle.connectionTimeouts", {
                        defaultValue: "连接与超时",
                      })}
                    </Label>
                    <span className="text-xs text-muted-foreground">
                      {t("providerBundle.timeoutHint", {
                        defaultValue: "留空即使用全局默认值",
                      })}
                    </span>
                  </div>
                  <div className="grid gap-3 sm:grid-cols-3">
                    {[
                      {
                        key: "timeoutSeconds" as const,
                        defaultKey: "requestTimeoutSeconds" as const,
                        label: t("providerBundle.requestTimeoutShort", {
                          defaultValue: "请求超时",
                        }),
                        max: 3_600,
                      },
                      {
                        key: "streamFirstByteTimeoutSeconds" as const,
                        defaultKey: "streamFirstByteTimeoutSeconds" as const,
                        label: t("providerBundle.firstByteTimeoutShort", {
                          defaultValue: "首字节超时",
                        }),
                        max: 600,
                      },
                      {
                        key: "streamIdleTimeoutSeconds" as const,
                        defaultKey: "streamIdleTimeoutSeconds" as const,
                        label: t("providerBundle.streamIdleTimeoutShort", {
                          defaultValue: "流空闲超时",
                        }),
                        max: 3_600,
                      },
                    ].map(({ key, defaultKey, label, max }) => (
                      <div key={key} className="space-y-1.5">
                        <Label
                          htmlFor={`provider-bundle-${key}`}
                          className="text-xs font-normal text-muted-foreground"
                        >
                          {label}
                        </Label>
                        {/* The unit belongs in the field, not in the label: three labels
                            each ending in "（秒）" made the row read as boilerplate. */}
                        <div className="relative">
                          <Input
                            id={`provider-bundle-${key}`}
                            type="number"
                            min={1}
                            max={max}
                            step={1}
                            value={draft.transport[key]}
                            placeholder={
                              requestDefaults
                                ? String(requestDefaults[defaultKey])
                                : undefined
                            }
                            className="pr-9 focus:placeholder:text-transparent [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none"
                            onChange={(event) =>
                              setDraft((current) => ({
                                ...current,
                                transport: {
                                  ...current.transport,
                                  [key]: event.target.value,
                                },
                              }))
                            }
                          />
                          <span className="pointer-events-none absolute inset-y-0 right-3 flex items-center text-xs text-muted-foreground">
                            {t("providerBundle.secondsUnit", {
                              defaultValue: "秒",
                            })}
                          </span>
                        </div>
                      </div>
                    ))}
                  </div>
                </div>
              </div>
            </ConfigCard>

            <ConfigCard
              id="bundle-card-model"
              title={t("serverProviderForm.model.title")}
              icon={<Target className="h-4 w-4 text-muted-foreground" />}
              summary={modelSummary}
              status={<CardGapStatus gap={modelGap ? "model" : null} />}
              open={openCards.model}
              onOpenChange={(open) => setCardOpen("model", open)}
            >
              <div className="space-y-4">
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <Label className="shrink-0">
                    {t("providerBundle.modelScope")}
                  </Label>
                  {perAppModelPolicyRequired ? (
                    <Badge variant="secondary">
                      {t("providerBundle.modelScopePerApp")}
                    </Badge>
                  ) : perAppModelPolicySupported ? (
                    <Tabs
                      value={draft.modelPolicyScope}
                      onValueChange={(value) => {
                        if (value !== "global" && value !== "per_app") return;
                        if (
                          value === "global" &&
                          draft.modelPolicyScope === "per_app" &&
                          perAppModelPoliciesDiffer(draft)
                        ) {
                          setModelScopeConfirmOpen(true);
                          return;
                        }
                        setDraft((current) =>
                          changeModelPolicyScope(current, value),
                        );
                      }}
                    >
                      <TabsList className="h-9 gap-1 border bg-muted/40 p-1">
                        <TabsTrigger
                          value="global"
                          className="h-7 rounded-sm px-3 text-xs"
                        >
                          {t("providerBundle.modelScopeGlobal")}
                        </TabsTrigger>
                        <TabsTrigger
                          value="per_app"
                          className="h-7 rounded-sm px-3 text-xs"
                        >
                          {t("providerBundle.modelScopePerApp")}
                        </TabsTrigger>
                      </TabsList>
                    </Tabs>
                  ) : (
                    <Badge variant="secondary">
                      {t("providerBundle.modelScopeGlobal")}
                    </Badge>
                  )}
                </div>
                {/* One sentence about the mode you are actually in beats a paragraph
                    describing both of them. */}
                <p className="text-xs text-muted-foreground">
                  {draft.modelPolicyScope === "per_app"
                    ? t("providerBundle.modelScopePerAppHint", {
                        defaultValue: "每个应用接口各自决定使用哪个上游模型。",
                      })
                    : t("providerBundle.modelScopeGlobalHint", {
                        defaultValue:
                          "所有可配置的应用接口共用同一个上游模型。",
                      })}
                </p>

                {draft.modelPolicyScope === "global" ? (
                  <>
                    <div className="flex flex-wrap items-center gap-2">
                      {allowedModelPolicies.length > 1 ? (
                        <Tabs
                          value={draft.modelPolicy}
                          onValueChange={(value) => {
                            if (value !== "single" && value !== "passthrough")
                              return;
                            if (!allowedModelPolicies.includes(value)) return;
                            setDraft((current) =>
                              updateBundleModel(
                                current,
                                value,
                                value === "single" &&
                                  !current.upstreamModel.trim()
                                  ? defaultSharedModel
                                  : current.upstreamModel,
                              ),
                            );
                          }}
                        >
                          <TabsList className="h-9 gap-1 border bg-muted/40 p-1">
                            {allowedModelPolicies.map((policy) => (
                              <TabsTrigger
                                key={policy}
                                value={policy}
                                className="h-7 min-w-0 gap-1.5 rounded-sm px-3 text-xs data-[state=active]:bg-background data-[state=active]:text-foreground"
                              >
                                {policy === "single" ? (
                                  <Target className="h-3.5 w-3.5 shrink-0" />
                                ) : (
                                  <ArrowRightLeft className="h-3.5 w-3.5 shrink-0" />
                                )}
                                {policy === "single"
                                  ? t("providerBundle.modelSingle")
                                  : t("providerBundle.modelPassthrough")}
                              </TabsTrigger>
                            ))}
                          </TabsList>
                        </Tabs>
                      ) : (
                        <div className="inline-flex h-9 items-center gap-2 rounded-md border bg-muted/40 px-3 text-xs font-medium">
                          {draft.modelPolicy === "single" ? (
                            <Target className="h-3.5 w-3.5" />
                          ) : (
                            <ArrowRightLeft className="h-3.5 w-3.5" />
                          )}
                          {draft.modelPolicy === "single"
                            ? t("providerBundle.modelSingle")
                            : t("providerBundle.modelPassthrough")}
                        </div>
                      )}
                      {draft.modelPolicy === "single" ? (
                        <Input
                          id={bundleValidationFieldId({
                            code: "upstreamModelRequired",
                            field: "upstreamModel",
                            message: "",
                          })}
                          aria-label={t(
                            "serverProviderForm.model.upstreamModel",
                          )}
                          placeholder={
                            defaultSharedModel ||
                            t("serverProviderForm.model.upstreamModel")
                          }
                          className={cn(
                            "h-9 min-w-0 flex-1 basis-56",
                            fieldErrorClass(
                              matchesBundleValidationIssue(
                                validation,
                                "upstreamModel",
                              ),
                            ),
                          )}
                          value={draft.upstreamModel}
                          onChange={(event) =>
                            setDraft((current) =>
                              updateBundleModel(
                                current,
                                current.modelPolicy,
                                event.target.value,
                              ),
                            )
                          }
                        />
                      ) : null}
                    </div>
                    {fixedModelSurfaces.length ? (
                      <div className="rounded-md border px-3">
                        {fixedModelSurfaces.map((surface) => (
                          <SurfaceModelRow
                            key={surface.app}
                            surface={surface}
                            validation={validation}
                            onChange={updateSurface}
                          />
                        ))}
                      </div>
                    ) : null}
                  </>
                ) : (
                  /* Per App means every Surface gets a line here — one place to compare
                     them, instead of one model field buried in each Surface tab. */
                  <div className="rounded-md border px-3">
                    {draft.surfaces.map((surface) => (
                      <SurfaceModelRow
                        key={surface.app}
                        surface={surface}
                        validation={validation}
                        onChange={updateSurface}
                      />
                    ))}
                  </div>
                )}
              </div>
            </ConfigCard>

            {showSurfaceTabs || family.endpointScope === "surface" ? (
              <ConfigCard
                id="bundle-card-surfaces"
                title={t("providerBundle.surfaces", {
                  defaultValue: "应用接口",
                })}
                icon={<Layers className="h-4 w-4 text-muted-foreground" />}
                summary={surfacesSummary}
                status={
                  surfaceGapCount ? (
                    <span className="inline-flex items-center gap-1.5 rounded-full bg-destructive/10 px-2 py-0.5 text-[11px] font-medium text-destructive">
                      <StatusDot tone="gap" />
                      {t("providerBundle.cardPending", {
                        defaultValue: "待完善",
                      })}
                      <span className="tabular-nums">{surfaceGapCount}</span>
                    </span>
                  ) : null
                }
                open={openCards.surfaces}
                onOpenChange={(open) => setCardOpen("surfaces", open)}
              >
                {showSurfaceTabs ? (
                  <Tabs
                    value={activeApp}
                    onValueChange={(value) =>
                      setActiveApp(value as CoreProviderApp)
                    }
                  >
                    <TabsList
                      className="grid h-auto w-full"
                      style={{
                        gridTemplateColumns: `repeat(${draft.surfaces.length}, minmax(0, 1fr))`,
                      }}
                    >
                      {draft.surfaces.map((surface) => (
                        <TabsTrigger
                          key={surface.app}
                          value={surface.app}
                          className="min-w-0 gap-2"
                        >
                          <AppLogo app={surface.app} muted={!surface.enabled} />
                          <span className="truncate">
                            {APP_LABELS[surface.app]}
                          </span>
                          <StatusDot tone={surfaceTone(surface.app)} />
                        </TabsTrigger>
                      ))}
                    </TabsList>
                    {draft.surfaces.map((surface) => (
                      <TabsContent key={surface.app} value={surface.app}>
                        <SurfaceEditor
                          surface={surface}
                          validation={validation}
                          onChange={updateSurface}
                        />
                      </TabsContent>
                    ))}
                  </Tabs>
                ) : activeSurface ? (
                  <SurfaceEditor
                    surface={activeSurface}
                    validation={validation}
                    onChange={updateSurface}
                  />
                ) : null}
              </ConfigCard>
            ) : null}

            <ConfigCard
              id="bundle-card-diagnostics"
              title={t("providerBundle.cardDiagnostics", {
                defaultValue: "连通性测试",
              })}
              icon={<Stethoscope className="h-4 w-4 text-muted-foreground" />}
              summary={diagnosticsSummary}
              open={openCards.diagnostics}
              onOpenChange={(open) => setCardOpen("diagnostics", open)}
            >
              <div className="space-y-4">
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <Label className="shrink-0">
                    {t("providerBundle.testApp")}
                  </Label>
                  {enabledTestApps.length > 1 ? (
                    <Tabs
                      value={draft.testApp}
                      onValueChange={(value) =>
                        setDraft((current) => ({
                          ...current,
                          testApp: value as CoreProviderApp,
                        }))
                      }
                    >
                      <TabsList className="h-9 gap-1 border bg-muted/40 p-1">
                        {enabledTestApps.map((app) => (
                          <TabsTrigger
                            key={app}
                            value={app}
                            className="h-7 gap-1.5 rounded-sm px-3 text-xs"
                          >
                            <AppLogo app={app} size={14} />
                            {APP_LABELS[app]}
                          </TabsTrigger>
                        ))}
                      </TabsList>
                    </Tabs>
                  ) : (
                    <span className="inline-flex h-9 items-center gap-1.5 rounded-md border bg-muted/40 px-3 text-xs font-medium">
                      <AppLogo app={draft.testApp} size={14} />
                      {APP_LABELS[draft.testApp]}
                    </span>
                  )}
                </div>

                <div className="space-y-1.5">
                  <Label htmlFor="provider-bundle-test-model">
                    {t("providerBundle.providerTestModel", {
                      defaultValue: "供应商默认测试模型",
                    })}
                  </Label>
                  <Input
                    id="provider-bundle-test-model"
                    value={draft.testModel}
                    placeholder={healthCheckConfig?.testModels[draft.testApp]}
                    className="focus:placeholder:text-transparent"
                    onChange={(event) =>
                      setDraft((current) => ({
                        ...current,
                        testModel: event.target.value,
                      }))
                    }
                  />
                  {/* An empty field with a grey placeholder is ambiguous — it could be a
                      default or it could be nothing. Spell the fallback out. */}
                  {!draft.testModel.trim() &&
                  healthCheckConfig?.testModels[draft.testApp] ? (
                    <p className="text-xs text-muted-foreground">
                      {t("providerBundle.testModelDefaultHint", {
                        defaultValue: "留空则使用 {{model}}",
                        model: healthCheckConfig.testModels[draft.testApp],
                      })}
                    </p>
                  ) : null}
                </div>

                <Collapsible>
                  <CollapsibleTrigger asChild>
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      className="h-8 px-2 text-xs"
                    >
                      <ChevronDown className="mr-1.5 h-3.5 w-3.5" />
                      {t("providerBundle.surfaceTestModelOverrides", {
                        defaultValue: "App 特例",
                      })}
                      {surfaceTestModelOverrides ? (
                        <Badge variant="secondary" className="ml-1.5">
                          {surfaceTestModelOverrides}
                        </Badge>
                      ) : null}
                    </Button>
                  </CollapsibleTrigger>
                  <CollapsibleContent className="pt-3">
                    <div className="grid gap-3 md:grid-cols-3">
                      {enabledTestApps.map((app) => (
                        <div key={app} className="space-y-1.5">
                          <Label
                            htmlFor={`provider-bundle-${app}-test-model`}
                            className="flex items-center gap-2 text-xs font-normal text-muted-foreground"
                          >
                            <AppLogo app={app} size={14} />
                            {APP_LABELS[app]}
                          </Label>
                          <Input
                            id={`provider-bundle-${app}-test-model`}
                            value={draft.surfaceTestModels[app]}
                            placeholder={
                              draft.testModel.trim() ||
                              healthCheckConfig?.testModels[app]
                            }
                            className="focus:placeholder:text-transparent"
                            onChange={(event) =>
                              setDraft((current) => ({
                                ...current,
                                surfaceTestModels: {
                                  ...current.surfaceTestModels,
                                  [app]: event.target.value,
                                },
                              }))
                            }
                          />
                        </div>
                      ))}
                    </div>
                  </CollapsibleContent>
                </Collapsible>
              </div>
            </ConfigCard>

            {codexDriverOptions || grokDriverOptions ? (
              <ConfigCard
                id="bundle-card-features"
                title={t("providerBundle.cardFeatures", {
                  defaultValue: "功能开关",
                })}
                icon={
                  <SlidersHorizontal className="h-4 w-4 text-muted-foreground" />
                }
                summary={featuresSummary}
                open={openCards.features}
                onOpenChange={(open) => setCardOpen("features", open)}
              >
                <div className="space-y-5">
                  {codexDriverOptions ? (
                    <CodexFeatureOptions
                      values={{
                        codexFastMode: draft.surfaces.some(
                          (surface) =>
                            surface.driverOptions.codexFastMode === true,
                        ),
                        codexImageGenerationEnabled: draft.surfaces.some(
                          (surface) =>
                            surface.driverOptions
                              .codexImageGenerationEnabled === true,
                        ),
                        codexWebsocketEnabled: draft.surfaces.some(
                          (surface) =>
                            surface.driverOptions.codexWebsocketEnabled ===
                            true,
                        ),
                      }}
                      onChange={setDriverOption}
                    />
                  ) : null}
                  {grokDriverOptions ? (
                    <div className="space-y-2">
                      {codexDriverOptions ? (
                        <Label>
                          {t("grokOauth.featureOptionsTitle", {
                            defaultValue: "Grok OAuth 媒体能力",
                          })}
                        </Label>
                      ) : null}
                      <GrokFeatureOptions
                        providerId={bundle?.surfaces.codex?.provider.id}
                        values={{
                          grokImageGenerationEnabled: draft.surfaces.some(
                            (surface) =>
                              surface.driverOptions
                                .grokImageGenerationEnabled === true,
                          ),
                          grokImageEditEnabled: draft.surfaces.some(
                            (surface) =>
                              surface.driverOptions.grokImageEditEnabled ===
                              true,
                          ),
                          grokVideoGenerationEnabled: draft.surfaces.some(
                            (surface) =>
                              surface.driverOptions
                                .grokVideoGenerationEnabled === true,
                          ),
                        }}
                        onChange={setGrokDriverOption}
                      />
                    </div>
                  ) : null}
                </div>
              </ConfigCard>
            ) : null}

            {codexControlTarget ? (
              <ConfigCard
                id="bundle-card-account-ops"
                title={t("providerBundle.cardAccountOps", {
                  defaultValue: "账号运维",
                })}
                icon={<Wrench className="h-4 w-4 text-muted-foreground" />}
                summary={t("providerBundle.cardAccountOpsSummary", {
                  defaultValue: "额度重置 · 邀请码",
                })}
                open={openCards.accountOps}
                onOpenChange={(open) => setCardOpen("accountOps", open)}
              >
                <div className="space-y-6">
                  <CodexBankedResetPanel
                    accountId={codexControlTarget.accountId}
                    providerId={codexControlTarget.providerId}
                    expectedRevision={codexControlTarget.expectedRevision}
                    workspaceId={codexControlWorkspaceId}
                  />
                  <div className="space-y-3 border-t pt-5">
                    <h3 className="text-sm font-semibold">
                      {t("codexReferrals.sectionTitle")}
                    </h3>
                    <CodexReferralPanel
                      providerId={codexControlTarget.providerId}
                      expectedRevision={codexControlTarget.expectedRevision}
                    />
                  </div>
                </div>
              </ConfigCard>
            ) : null}
          </div>
        ) : null}

        {showShareStep ? (
          <div
            id="bundle-section-share"
            role="tabpanel"
            aria-labelledby="bundle-tab-share"
            ref={shareSectionRef}
          >
            <BundleShareEditor
              draft={shareDraft}
              onChange={setShareDraft}
              ownerEmail={ownerEmail}
              shareUrl={shareUrl}
              showGrokMedia={grokDriverOptions}
            />
          </div>
        ) : null}
      </div>

      <div className="sticky bottom-0 z-20 flex flex-wrap items-center justify-end gap-2 bg-background/95 py-4 backdrop-blur">
        <Button
          type="button"
          variant="outline"
          onClick={closeGuard.requestClose}
          disabled={saving}
        >
          {t("common.cancel")}
        </Button>
        {!persisted && createStep !== "family" ? (
          <Button
            type="button"
            variant="outline"
            onClick={retreatCreateStep}
            disabled={saving}
          >
            {t("common.previous")}
          </Button>
        ) : null}
        {!persisted && createStep !== "share" ? (
          <Button type="button" onClick={advanceCreateStep} disabled={saving}>
            {t("common.next")}
          </Button>
        ) : (
          <Button
            type="button"
            onClick={() => void submit()}
            disabled={saving || (persisted && !dirty)}
          >
            {saving ? (
              <LoaderCircle className="mr-2 h-4 w-4 animate-spin" />
            ) : (
              <Save className="mr-2 h-4 w-4" />
            )}
            {t("common.save")}
          </Button>
        )}
      </div>
      <ConfirmDialog
        isOpen={modelScopeConfirmOpen}
        title={t("providerBundle.modelScopeMergeTitle")}
        message={t("providerBundle.modelScopeMergeMessage")}
        confirmText={t("providerBundle.modelScopeMergeConfirm")}
        cancelText={t("common.cancel")}
        variant="info"
        zIndex="top"
        onConfirm={() => {
          setDraft((current) => changeModelPolicyScope(current, "global"));
          setModelScopeConfirmOpen(false);
        }}
        onCancel={() => setModelScopeConfirmOpen(false)}
      />
      <ConfirmDialog
        isOpen={closeGuard.confirmOpen}
        title={t("provider.unsavedChanges.title")}
        message={t("provider.unsavedChanges.editMessage")}
        confirmText={t("provider.unsavedChanges.discard")}
        cancelText={t("provider.unsavedChanges.keepEditing")}
        variant="destructive"
        zIndex="top"
        onConfirm={closeGuard.discardAndClose}
        onCancel={closeGuard.keepEditing}
      />
    </div>
  );
}
