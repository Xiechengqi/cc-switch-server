import { useEffect, useMemo, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  ArrowLeft,
  ArrowRightLeft,
  Check,
  ChevronDown,
  Copy,
  KeyRound,
  LoaderCircle,
  Plus,
  RefreshCw,
  Save,
  Share2,
  Target,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";

import { ClaudeIcon, CodexIcon, GeminiIcon } from "@/components/BrandIcons";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ProviderIconControl } from "@/components/providers/ProviderIconControl";
import { MarketSelectorField } from "@/components/providers/ProviderShareSection";
import { ShareUserGrantsEditor } from "@/components/providers/ShareUserGrantsEditor";
import { ManagedAccountSection } from "@/components/providers/forms/ManagedAccountSection";
import { CodexReferralPanel } from "@/components/providers/forms/CodexReferralPanel";
import { SubdomainGeneratorButton } from "@/components/SubdomainGeneratorButton";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import { EmailTagsInput } from "@/components/ui/tags-input";
import { Textarea } from "@/components/ui/textarea";
import type {
  ProviderBundleView,
  ProviderCustomBinding,
  ProviderRuntimeDefaults,
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
  useTokenMarketsQuery,
} from "@/lib/query";
import {
  customPolicyForProfile,
  customRecipesForFamily,
  driverForProfile,
  familyById,
  profileById,
  type CoreProviderApp,
} from "@/server/providerRegistry";
import {
  CodexFeatureOptions,
  type CodexFeatureOptionKey,
} from "@/server/providers/CodexFeatureOptions";
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
  buildShareUserGrantsForAcl,
  isValidShareEmail,
  SHARE_TOKEN_PRESETS,
  uniqueSortedEmails,
} from "@/utils/shareFormUtils";
import { DEFAULT_PARALLEL_LIMIT } from "@/utils/shareUtils";
import {
  BUNDLE_SHARE_EXPIRY_PRESETS,
  createBundleShareDraft,
  isValidShareSlug,
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
  supportsPerAppModelPolicy,
  toProviderBundleWriteDraft,
  updateBundleModel,
  updateSurfaceEndpoint,
  updateSurfaceModel,
  type BundleSecretDraft,
  type BundleSurfaceEditorDraft,
  type BundleValidationIssue,
  type ProviderBundleEditorDraft,
} from "./bundleDraft";
import { createDraftForSelectedFamily } from "./bundleDefaults";
import {
  bundleValidationFieldId,
  firstBundleValidationIssue,
  matchesBundleValidationIssue,
} from "./bundleValidation";
import { preferredManagedAccount, recommendedFamily } from "./familyCatalog";
import { FamilyPicker } from "./FamilyPicker";

interface ProviderBundleEditorProps {
  bundle?: ProviderBundleView;
  duplicate?: boolean;
  initialSection?: "share";
  onCancel: () => void;
  onSaved: (bundle: ProviderBundleView) => void;
  onOpenShareSettings?: () => void;
}

type CreateStep = "family" | "supply" | "share";
const CREATE_STEPS: CreateStep[] = ["family", "supply", "share"];

function fieldErrorClass(invalid: boolean): string {
  return invalid ? "border-destructive focus-visible:ring-destructive" : "";
}

const APP_LABELS: Record<CoreProviderApp, string> = {
  claude: "Claude",
  codex: "Codex",
  gemini: "Gemini",
};

function AppLogo({ app, size = 16 }: { app: CoreProviderApp; size?: number }) {
  if (app === "claude") return <ClaudeIcon size={size} />;
  if (app === "codex") return <CodexIcon size={size} />;
  return <GeminiIcon size={size} />;
}

function Section({
  title,
  icon,
  children,
}: {
  title: string;
  icon?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="space-y-4 pb-6">
      <h2 className="flex items-center gap-2 text-sm font-semibold">
        {icon}
        {title}
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

function SurfaceEditor({
  surface,
  modelPolicyScope,
  validation,
  onChange,
}: {
  surface: BundleSurfaceEditorDraft;
  modelPolicyScope: ProviderBundleEditorDraft["modelPolicyScope"];
  validation: BundleValidationIssue | null;
  onChange: (surface: BundleSurfaceEditorDraft) => void;
}) {
  const { t } = useTranslation();
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
  const allowedModelPolicies = modelPoliciesForSurface(surface);
  const modelPolicyConfigurable = allowedModelPolicies.length > 1;
  const updateDriverOptions = (
    patch: Partial<BundleSurfaceEditorDraft["driverOptions"]>,
  ) =>
    onChange({
      ...surface,
      driverOptions: { ...surface.driverOptions, ...patch },
    });

  return (
    <div className="space-y-6 pt-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <Badge variant="outline">{profile.label}</Badge>
        </div>
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

      {modelPolicyScope === "per_app" ? (
        <div className="space-y-4 border-b pb-6">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <Label>{t("serverProviderForm.model.title")}</Label>
            <Badge variant="secondary">
              {modelPolicyConfigurable
                ? t("providerBundle.modelScopePerApp")
                : t("providerBundle.modelProfileFixed")}
            </Badge>
          </div>
          {modelPolicyConfigurable ? (
            <>
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
                <TabsList className="grid h-10 w-full grid-cols-2 gap-1 border bg-muted/40 p-1">
                  {allowedModelPolicies.map((policy) => (
                    <TabsTrigger
                      key={policy}
                      value={policy}
                      className="min-w-0 gap-2 rounded-sm px-3 data-[state=active]:bg-background data-[state=active]:text-foreground"
                    >
                      {policy === "single" ? (
                        <Target className="hidden h-4 w-4 shrink-0 sm:block" />
                      ) : (
                        <ArrowRightLeft className="hidden h-4 w-4 shrink-0 sm:block" />
                      )}
                      {policy === "single"
                        ? t("providerBundle.modelSingle")
                        : t("providerBundle.modelPassthrough")}
                    </TabsTrigger>
                  ))}
                </TabsList>
              </Tabs>
              {surface.modelPolicy === "single" ? (
                <div className="space-y-2">
                  <Label htmlFor={`surface-${surface.app}-model`}>
                    {t("serverProviderForm.model.upstreamModel")}
                  </Label>
                  <Input
                    id={bundleValidationFieldId({
                      code: "surfaceUpstreamModelRequired",
                      field: "upstreamModel",
                      message: "",
                      surface: surface.app,
                    })}
                    className={fieldErrorClass(
                      matchesBundleValidationIssue(
                        validation,
                        "upstreamModel",
                        surface.app,
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
                </div>
              ) : null}
            </>
          ) : (
            <div className="inline-flex h-10 items-center gap-2 rounded-md border bg-muted/40 px-3 text-sm font-medium">
              {surface.modelPolicy === "single" ? (
                <Target className="h-4 w-4" />
              ) : (
                <ArrowRightLeft className="h-4 w-4" />
              )}
              {surface.modelPolicy === "single"
                ? t("providerBundle.modelSingle")
                : t("providerBundle.modelPassthrough")}
              {surface.modelPolicy === "single" && surface.upstreamModel ? (
                <span className="text-muted-foreground">
                  {surface.upstreamModel}
                </span>
              ) : null}
            </div>
          )}
        </div>
      ) : null}

      {profile.formComposition === "custom" && customPolicy ? (
        <div className="space-y-5">
          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2 md:col-span-2">
              <Label>{t("serverProviderForm.endpoint.url")}</Label>
              <Input
                id={bundleValidationFieldId({
                  code: "endpointInvalid",
                  field: "endpoint",
                  message: "",
                  surface: surface.app,
                })}
                type="url"
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
              <div className="space-y-2 md:col-span-2">
                <Label>
                  {surface.customBinding.authScheme === "query"
                    ? t("providerBundle.queryParameter", {
                        defaultValue: "API Key 查询参数名",
                      })
                    : t("providerBundle.authHeader", {
                        defaultValue: "API Key Header 名称",
                      })}
                </Label>
                <Input
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
          </div>

          <div className="space-y-2">
            <Label>{customCredentialLabel}</Label>
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

          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <Label>{t("serverProviderForm.headers.title")}</Label>
              <Button
                type="button"
                size="sm"
                variant="outline"
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
                <Plus className="mr-2 h-4 w-4" />
                {t("common.add")}
              </Button>
            </div>
            {surface.headers
              .filter((header) => !header.removed)
              .map((header) => (
                <div
                  key={header.id}
                  className="grid gap-2 sm:grid-cols-[1fr_1fr_auto]"
                >
                  <Input
                    value={header.name}
                    placeholder="x-header-name"
                    onChange={(event) =>
                      onChange({
                        ...surface,
                        headers: surface.headers.map((item) =>
                          item.id === header.id
                            ? { ...item, name: event.target.value }
                            : item,
                        ),
                      })
                    }
                  />
                  <SecretInput
                    value={header.value}
                    placeholder={header.configured ? "••••••••" : undefined}
                    onChange={(event) =>
                      onChange({
                        ...surface,
                        headers: surface.headers.map((item) =>
                          item.id === header.id
                            ? { ...item, value: event.target.value }
                            : item,
                        ),
                      })
                    }
                  />
                  <Button
                    type="button"
                    size="icon"
                    variant="outline"
                    title={t("common.delete")}
                    onClick={() =>
                      onChange({
                        ...surface,
                        headers: surface.headers.map((item) =>
                          item.id === header.id
                            ? { ...item, removed: true }
                            : item,
                        ),
                      })
                    }
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                </div>
              ))}
          </div>

          <div className="space-y-2">
            <Label>{t("serverProviderForm.usage.customUserAgent")}</Label>
            <Input
              value={surface.driverOptions.customUserAgent ?? ""}
              onChange={(event) =>
                updateDriverOptions({ customUserAgent: event.target.value })
              }
            />
          </div>
        </div>
      ) : null}

      {surface.runtime ? (
        <Collapsible className="pt-4">
          <div className="flex items-center justify-between gap-2">
            <CollapsibleTrigger asChild>
              <Button type="button" variant="ghost" size="sm">
                <ChevronDown className="mr-2 h-4 w-4" />
                {t("providerBundle.effectiveConfig", {
                  defaultValue: "有效运行配置",
                })}
              </Button>
            </CollapsibleTrigger>
            <Button
              type="button"
              size="icon"
              variant="ghost"
              title={t("common.copy")}
              onClick={() =>
                void copyText(JSON.stringify(surface.runtime, null, 2))
              }
            >
              <Copy className="h-4 w-4" />
            </Button>
          </div>
          <CollapsibleContent className="pt-2">
            <pre className="max-h-80 overflow-auto rounded-md border bg-muted/30 p-3 font-mono text-xs leading-relaxed">
              {JSON.stringify(surface.runtime, null, 2)}
            </pre>
          </CollapsibleContent>
        </Collapsible>
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
  onOpenShareSettings,
}: {
  draft: ProviderBundleShareDraft;
  onChange: (draft: ProviderBundleShareDraft) => void;
  ownerEmail: string;
  shareUrl?: string | null;
  onOpenShareSettings?: () => void;
}) {
  const { t } = useTranslation();
  const [marketSelectKey, setMarketSelectKey] = useState(0);
  const marketsQuery = useTokenMarketsQuery(
    draft.enabled && draft.forSale === "Yes",
  );
  const markets = marketsQuery.data ?? [];
  const marketEmails = useMemo(
    () => new Set(markets.map((market) => market.email.trim().toLowerCase())),
    [markets],
  );
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
  const selectedMarketEmails = useMemo(
    () =>
      draft.marketAccessMode === "selected"
        ? draft.sharedWithEmails.filter(
            (email) =>
              marketEmails.has(email.toLowerCase()) &&
              !routerManagedEmails.has(email.toLowerCase()),
          )
        : [],
    [
      draft.marketAccessMode,
      draft.sharedWithEmails,
      marketEmails,
      routerManagedEmails,
    ],
  );
  const protectedGrantEmails = useMemo(
    () => new Set([...selectedMarketEmails, ...routerManagedEmails]),
    [routerManagedEmails, selectedMarketEmails],
  );
  const directEmails = useMemo(
    () =>
      draft.sharedWithEmails.filter(
        (email) =>
          !marketEmails.has(email.toLowerCase()) &&
          !routerManagedEmails.has(email.toLowerCase()),
      ),
    [draft.sharedWithEmails, marketEmails, routerManagedEmails],
  );
  const normalizedOwnerEmail = ownerEmail.trim().toLowerCase();
  const defaultUserPolicy = useMemo(
    () => defaultBundleUserPolicy(draft),
    [draft.expiry, draft.parallelLimit, draft.tokenLimit],
  );
  const displayedUserGrants = useMemo(
    () =>
      buildShareUserGrantsForAcl({
        source: draft.userGrants,
        ownerEmail: normalizedOwnerEmail,
        aclEmails: draft.sharedWithEmails,
        defaultPolicy: defaultUserPolicy,
      }),
    [
      defaultUserPolicy,
      draft.sharedWithEmails,
      draft.userGrants,
      normalizedOwnerEmail,
    ],
  );
  const slugInvalid = Boolean(
    draft.subdomain.trim() && !isValidShareSlug(draft.subdomain),
  );
  const directEmailInvalid = directEmails.some(
    (email) => !isValidShareEmail(email),
  );
  const marketsError =
    marketsQuery.error instanceof Error
      ? marketsQuery.error.message
      : marketsQuery.error
        ? String(marketsQuery.error)
        : undefined;

  const updateAccessEmails = (
    emails: string[],
    patch: Partial<ProviderBundleShareDraft> = {},
  ) => {
    const normalizedEmails = uniqueSortedEmails(
      emails.map((email) => email.trim().toLowerCase()).filter(Boolean),
    );
    const sourceGrants = patch.userGrants ?? draft.userGrants;
    onChange({
      ...draft,
      ...patch,
      sharedWithEmails: normalizedEmails,
      userGrants: buildShareUserGrantsForAcl({
        source: sourceGrants,
        ownerEmail: normalizedOwnerEmail,
        aclEmails: normalizedEmails,
        defaultPolicy: defaultUserPolicy,
      }),
    });
  };

  const updateDirectEmails = (emails: string[]) =>
    updateAccessEmails([...emails, ...selectedMarketEmails]);

  const updateMarketEmails = (emails: string[]) =>
    updateAccessEmails([...directEmails, ...emails], {
      marketAccessMode: "selected",
    });

  const updateUserGrants = (
    userGrants: ProviderBundleShareDraft["userGrants"],
  ) => {
    const sharedWithEmails = uniqueSortedEmails(
      Object.values(userGrants)
        .filter(
          (grant) =>
            grant.active !== false &&
            grant.role === "shareto" &&
            grant.manager !== "routerShareMarket",
        )
        .map((grant) => grant.email.trim().toLowerCase())
        .filter(Boolean),
    );
    onChange({ ...draft, sharedWithEmails, userGrants });
  };

  return (
    <Section
      title={t("provider.share.sectionTitle", { defaultValue: "远程分享" })}
      icon={<Share2 className="h-4 w-4" />}
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          {shareUrl ? (
            <>
              <code className="max-w-full truncate rounded bg-muted px-2 py-1 text-xs">
                {shareUrl}
              </code>
              <Button
                type="button"
                size="icon"
                variant="outline"
                title={t("common.copy")}
                onClick={() => void copyText(shareUrl)}
              >
                <Copy className="h-4 w-4" />
              </Button>
            </>
          ) : (
            <Badge variant="outline">
              {t("provider.share.stateNone", { defaultValue: "未启用分享" })}
            </Badge>
          )}
        </div>
        <div className="flex items-center gap-2">
          {onOpenShareSettings ? (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={onOpenShareSettings}
            >
              {t("common.settings")}
            </Button>
          ) : null}
          <Label htmlFor="bundle-share-enabled">
            {t("provider.share.enableShare", { defaultValue: "启用远程分享" })}
          </Label>
          <Switch
            id="bundle-share-enabled"
            checked={draft.enabled}
            onCheckedChange={(enabled) => onChange({ ...draft, enabled })}
          />
        </div>
      </div>
      {draft.enabled ? (
        <div className="grid gap-4 md:grid-cols-2">
          <div className="space-y-2">
            <Label htmlFor="bundle-share-subdomain">
              {t("provider.share.subdomain", { defaultValue: "分享子域名" })}
            </Label>
            <div className="flex items-center gap-2">
              <Input
                id="bundle-share-subdomain"
                value={draft.subdomain}
                aria-invalid={slugInvalid}
                className={cn(slugInvalid && "border-destructive")}
                onChange={(event) =>
                  onChange({ ...draft, subdomain: event.target.value })
                }
              />
              <SubdomainGeneratorButton
                embedded={false}
                onGenerated={(subdomain) => onChange({ ...draft, subdomain })}
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
            <Label>
              {t("provider.share.forSale", { defaultValue: "访问模式" })}
            </Label>
            <Select
              value={draft.forSale}
              onValueChange={(forSale) => {
                const next = forSale as ProviderBundleShareDraft["forSale"];
                if (next === "Yes") {
                  onChange({ ...draft, forSale: next });
                  return;
                }
                updateAccessEmails(directEmails, { forSale: next });
              }}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="No">
                  {t("provider.share.private", { defaultValue: "私有" })}
                </SelectItem>
                <SelectItem value="Free">
                  {t("provider.share.free", { defaultValue: "免费" })}
                </SelectItem>
                <SelectItem value="Yes">
                  {t("provider.share.market", { defaultValue: "市场" })}
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-2 md:col-span-2">
            <Label>
              {t("provider.share.description", { defaultValue: "描述" })}
            </Label>
            <Input
              value={draft.description}
              onChange={(event) =>
                onChange({ ...draft, description: event.target.value })
              }
            />
          </div>
          <div className="space-y-2 md:col-span-2">
            <Label htmlFor="bundle-share-authorized-emails">
              {t("provider.share.sharedWith", { defaultValue: "授权邮箱" })}
            </Label>
            <EmailTagsInput
              inputId="bundle-share-authorized-emails"
              value={directEmails}
              invalid={directEmailInvalid}
              placeholder={t("share.sharedWithEmails", {
                defaultValue: "输入邮箱后按 Enter 添加",
              })}
              onChange={updateDirectEmails}
            />
            {directEmailInvalid ? (
              <p className="text-xs text-destructive">
                {t("share.validation.invalidEmail", {
                  defaultValue: "邮箱格式无效",
                })}
              </p>
            ) : null}
          </div>
          {draft.forSale === "Yes" ? (
            <MarketSelectorField
              markets={markets}
              marketAccessMode={draft.marketAccessMode}
              selectedMarketEmails={selectedMarketEmails}
              marketSelectKey={marketSelectKey}
              disabled={marketsQuery.isLoading || marketsQuery.isError}
              marketsLoading={marketsQuery.isLoading}
              marketsError={marketsError}
              onRetryMarkets={() => void marketsQuery.refetch()}
              onMarketAccessModeChange={(marketAccessMode) => {
                if (marketAccessMode === "all") {
                  updateAccessEmails(directEmails, { marketAccessMode });
                  return;
                }
                updateAccessEmails([...directEmails, ...selectedMarketEmails], {
                  marketAccessMode,
                });
              }}
              onSelectedMarketEmailsChange={updateMarketEmails}
              onMarketSelectKeyChange={setMarketSelectKey}
            />
          ) : null}
          <ShareUserGrantsEditor
            value={displayedUserGrants}
            ownerEmail={normalizedOwnerEmail}
            defaultPolicy={defaultUserPolicy}
            protectedEmails={protectedGrantEmails}
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
                  onChange({ ...draft, tokenLimit: event.target.value })
                }
              />
              <div className="flex flex-wrap gap-1.5">
                <Button
                  type="button"
                  variant={draft.tokenLimit === "" ? "secondary" : "outline"}
                  size="sm"
                  className="h-7 px-2 text-xs"
                  onClick={() => onChange({ ...draft, tokenLimit: "" })}
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
                    onClick={() =>
                      onChange({ ...draft, tokenLimit: String(preset) })
                    }
                  >
                    {preset.toLocaleString()}
                  </Button>
                ))}
              </div>
            </div>
            <div className="space-y-2">
              <Label>
                {t("provider.share.parallelLimit", { defaultValue: "并发限额" })}
              </Label>
              <Input
                type="number"
                min={1}
                placeholder={t("share.unlimited", { defaultValue: "无上限" })}
                value={draft.parallelLimit}
                onChange={(event) =>
                  onChange({ ...draft, parallelLimit: event.target.value })
                }
              />
              <div className="flex flex-wrap gap-1.5">
                <Button
                  type="button"
                  variant={draft.parallelLimit === "" ? "secondary" : "outline"}
                  size="sm"
                  className="h-7 px-2 text-xs"
                  onClick={() => onChange({ ...draft, parallelLimit: "" })}
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
                    onChange({
                      ...draft,
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
                  onChange({
                    ...draft,
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
  const [validation, setValidation] = useState<BundleValidationIssue | null>(
    null,
  );
  const [pendingFamilyId, setPendingFamilyId] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [runtimeDefaults, setRuntimeDefaults] =
    useState<ProviderRuntimeDefaults | null>(null);
  const [modelScopeConfirmOpen, setModelScopeConfirmOpen] = useState(false);
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
  const dirty =
    stableStringify(draft) !== draftBaselineRef.current ||
    stableStringify(shareDraft) !== shareBaselineRef.current;
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
  const ownerEmail =
    existingShare?.ownerEmail ??
    clientTunnelQuery.data?.config.ownerEmail ??
    "";

  useEffect(() => {
    let active = true;
    void providersApi
      .getRuntimeDefaults()
      .then((defaults) => {
        if (active) setRuntimeDefaults(defaults);
      })
      .catch((error) => {
        if (active) {
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
    shareBaselineRef.current = stableStringify(nextShareDraft);
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
  };

  const changeFamily = (familyId: string) => {
    if (persisted || familyId === draft.familyId) return;
    if (dirty) {
      setPendingFamilyId(familyId);
      return;
    }
    applyFamily(familyId);
  };

  const validationMessage = (issue: BundleValidationIssue): string =>
    t(`providerBundle.validation.${issue.code}`, {
      defaultValue: issue.message,
      app: issue.surface ? APP_LABELS[issue.surface] : undefined,
    });

  const focusIssue = (issue: BundleValidationIssue) => {
    if (issue.surface) setActiveApp(issue.surface);
    if (!persisted) {
      setCreateStep(
        issue.field === "family" ? "family" : "supply",
      );
    }
    requestAnimationFrame(() => {
      document
        .getElementById(bundleValidationFieldId(issue))
        ?.scrollIntoView({ behavior: "smooth", block: "center" });
    });
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
      shareDraft.sharedWithEmails.some((email) => !isValidShareEmail(email))
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
            <span className="truncate">{family.label}</span>
            {dirty ? (
              <Badge variant="outline">
                {t("providerBundle.unsaved", { defaultValue: "未保存" })}
              </Badge>
            ) : null}
          </div>
        </div>
      </div>

      {!persisted ? (
        <div className="grid grid-cols-3 gap-2">
          {CREATE_STEPS.map((step, index) => (
            <button
              key={step}
              type="button"
              className={cn(
                "rounded-md border px-3 py-2 text-left text-xs font-medium sm:text-sm",
                createStep === step
                  ? "border-primary bg-primary/5 text-foreground"
                  : "border-border text-muted-foreground",
              )}
              onClick={() => setCreateStep(step)}
            >
              {index + 1}.{" "}
              {step === "family"
                ? t("providerBundle.stepFamily", { defaultValue: "选择类型" })
                : step === "supply"
                  ? t("providerBundle.stepSupply", { defaultValue: "完成供给" })
                  : t("providerBundle.stepShare", { defaultValue: "远程分享" })}
            </button>
          ))}
        </div>
      ) : (
        <div className="flex flex-wrap gap-2 text-xs">
          {(
            [
              ["supply", t("providerBundle.stepSupply", { defaultValue: "供给" })],
              ["share", t("providerBundle.stepShare", { defaultValue: "远程分享" })],
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
          <Section
            title={t("providerBundle.family", { defaultValue: "供应商类型" })}
          >
            <FamilyPicker
              selectedFamilyId={draft.familyId}
              onSelect={changeFamily}
              onAutoSelect={applyFamily}
            />
          </Section>
        ) : null}

        {showSupplyStep ? (
          <div id="bundle-section-supply" className="space-y-6">
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
                  {family.label}
                </p>
              ) : null}
              {duplicate ? (
                <p className="text-center text-sm text-muted-foreground">
                  {t("providerBundle.duplicateSecretsCleared", {
                    defaultValue: "Credentials are not copied. Enter them again before saving.",
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
                      <SelectItem key={recipe.recipeId} value={recipe.recipeId}>
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

        {credentialProfile?.credentialPolicy.mode === "managed_account" ? (
          <Section
            title={t("providerBundle.account", { defaultValue: "OAuth 账号" })}
            icon={<KeyRound className="h-4 w-4" />}
          >
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
                const account = accounts.find((item) => item.id === accountId);
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
          </Section>
        ) : null}

        {familyCredentialSlots(family).length ? (
          <Section
            title={t("providerBundle.credential", { defaultValue: "共享凭据" })}
            icon={<KeyRound className="h-4 w-4" />}
          >
            <div className="grid gap-4 md:grid-cols-2">
              {familyCredentialSlots(family).map(({ logical, pointer }) => {
                const actualPointer =
                  Object.keys(draft.secrets).find(
                    (slot) =>
                      slot === pointer ||
                      slot.endsWith(pointer.slice(pointer.lastIndexOf("/"))),
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
                const revealedValue = revealedCredentialValues[actualPointer];
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
                        matchesBundleValidationIssue(validation, "credential"),
                      )}
                      value={value}
                      disabled={loadingCurrent || edit.action === "clear"}
                      autoComplete="new-password"
                      placeholder={
                        loadingCurrent
                          ? t("serverProviderForm.credentials.loading")
                          : currentRevealFailed
                            ? t(
                                "serverProviderForm.credentials.loadFailedPlaceholder",
                              )
                            : t("serverProviderForm.credentials.placeholder")
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
              })}
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
                      matchesBundleValidationIssue(validation, "awsRegion"),
                    )}
                    value={draft.awsRegion}
                    onChange={(event) =>
                      setDraft({ ...draft, awsRegion: event.target.value })
                    }
                  />
                </div>
              ) : null}
            </div>
          </Section>
        ) : null}

        <Section title={t("serverProviderForm.model.title")}>
          <div className="max-w-xl space-y-4">
            <p className="text-sm text-muted-foreground">
              {t("providerBundle.modelScopeHint", {
                defaultValue:
                  "Global uses one upstream model for every configurable App. Per App lets each Surface keep its own model.",
              })}
            </p>
            <div className="space-y-2">
              <Label>{t("providerBundle.modelScope")}</Label>
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
                  <TabsList className="grid h-10 w-full grid-cols-2 gap-1 border bg-muted/40 p-1">
                    <TabsTrigger value="global" className="rounded-sm">
                      {t("providerBundle.modelScopeGlobal")}
                    </TabsTrigger>
                    <TabsTrigger value="per_app" className="rounded-sm">
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

            {draft.modelPolicyScope === "global" ? (
              <>
                <div className="space-y-2">
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
                            value === "single" && !current.upstreamModel.trim()
                              ? defaultSharedModel
                              : current.upstreamModel,
                          ),
                        );
                      }}
                    >
                      <TabsList className="grid h-10 w-full grid-cols-2 gap-1 border bg-muted/40 p-1">
                        {allowedModelPolicies.map((policy) => (
                          <TabsTrigger
                            key={policy}
                            value={policy}
                            className="min-w-0 gap-2 rounded-sm px-3 data-[state=active]:bg-background data-[state=active]:text-foreground"
                          >
                            {policy === "single" ? (
                              <Target className="hidden h-4 w-4 shrink-0 sm:block" />
                            ) : (
                              <ArrowRightLeft className="hidden h-4 w-4 shrink-0 sm:block" />
                            )}
                            {policy === "single"
                              ? t("providerBundle.modelSingle")
                              : t("providerBundle.modelPassthrough")}
                          </TabsTrigger>
                        ))}
                      </TabsList>
                    </Tabs>
                  ) : (
                    <div className="inline-flex h-10 items-center gap-2 rounded-md border bg-muted/40 px-3 text-sm font-medium">
                      {draft.modelPolicy === "single" ? (
                        <Target className="h-4 w-4" />
                      ) : (
                        <ArrowRightLeft className="h-4 w-4" />
                      )}
                      {draft.modelPolicy === "single"
                        ? t("providerBundle.modelSingle")
                        : t("providerBundle.modelPassthrough")}
                    </div>
                  )}
                </div>
                {draft.modelPolicy === "single" ? (
                  <div className="space-y-2">
                    <Label htmlFor="provider-bundle-model">
                      {t("serverProviderForm.model.upstreamModel")}
                    </Label>
                    <Input
                      id={bundleValidationFieldId({
                        code: "upstreamModelRequired",
                        field: "upstreamModel",
                        message: "",
                      })}
                      className={fieldErrorClass(
                        matchesBundleValidationIssue(
                          validation,
                          "upstreamModel",
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
                  </div>
                ) : null}
                {fixedModelSurfaces.length ? (
                  <div className="grid gap-2 border-t pt-3 text-sm">
                    {fixedModelSurfaces.map((surface) => (
                      <div
                        key={surface.app}
                        className="flex min-w-0 items-center gap-2"
                      >
                        <span className="shrink-0 font-medium">
                          {APP_LABELS[surface.app]}
                        </span>
                        <Badge variant="secondary">
                          {t("providerBundle.modelProfileFixed")}
                        </Badge>
                        <span
                          className="min-w-0 truncate text-muted-foreground"
                          title={
                            surface.modelPolicy === "single"
                              ? surface.upstreamModel
                              : t("providerBundle.modelPassthrough")
                          }
                        >
                          {surface.modelPolicy === "single"
                            ? surface.upstreamModel
                            : t("providerBundle.modelPassthrough")}
                        </span>
                      </div>
                    ))}
                  </div>
                ) : null}
              </>
            ) : null}
          </div>
        </Section>

        <Collapsible>
          <CollapsibleTrigger asChild>
            <Button type="button" variant="ghost" size="sm">
              <ChevronDown className="mr-2 h-4 w-4" />
              {t("providerBundle.advanced", { defaultValue: "高级设置" })}
            </Button>
          </CollapsibleTrigger>
          <CollapsibleContent className="space-y-6 pt-4">
        <Section
          title={t("providerBundle.testModel", {
            defaultValue: "测试模型",
          })}
        >
          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2">
              <Label>{t("providerBundle.testApp")}</Label>
              <Select
                value={draft.testApp}
                onValueChange={(value) =>
                  setDraft((current) => ({
                    ...current,
                    testApp: value as CoreProviderApp,
                  }))
                }
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {enabledTestApps.map((app) => (
                    <SelectItem key={app} value={app}>
                      <span className="flex items-center gap-2">
                        <AppLogo app={app} />
                        {APP_LABELS[app]}
                      </span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-2">
              <Label htmlFor="provider-bundle-test-model">
                {t("providerBundle.providerTestModel", {
                  defaultValue: "供应商默认测试模型",
                })}
              </Label>
              <Input
                id="provider-bundle-test-model"
                value={draft.testModel}
                placeholder={runtimeDefaults?.testModels[draft.testApp]}
                className="focus:placeholder:text-transparent"
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    testModel: event.target.value,
                  }))
                }
              />
            </div>
            <div className="space-y-3 md:col-span-2">
              <Label>
                {t("providerBundle.surfaceTestModelOverrides", {
                  defaultValue: "App 特例",
                })}
              </Label>
              <div className="grid gap-4 md:grid-cols-3">
                {enabledTestApps.map((app) => (
                  <div key={app} className="space-y-2">
                    <Label
                      htmlFor={`provider-bundle-${app}-test-model`}
                      className="flex items-center gap-2 text-xs text-muted-foreground"
                    >
                      <AppLogo app={app} />
                      {APP_LABELS[app]}
                    </Label>
                    <Input
                      id={`provider-bundle-${app}-test-model`}
                      value={draft.surfaceTestModels[app]}
                      placeholder={
                        draft.testModel.trim() ||
                        runtimeDefaults?.testModels[app]
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
            </div>
          </div>
        </Section>

        {commonEndpointEditable ? (
          <Section title={t("serverProviderForm.endpoint.title")}>
            <Input
              id={bundleValidationFieldId({
                code: "endpointInvalid",
                field: "endpoint",
                message: "",
              })}
              type="url"
              className={fieldErrorClass(
                matchesBundleValidationIssue(validation, "endpoint"),
              )}
              value={draft.endpoint}
              onChange={(event) =>
                setDraft({ ...draft, endpoint: event.target.value })
              }
            />
          </Section>
        ) : null}

        <Section
          title={t("providerBundle.connectionTimeouts", {
            defaultValue: "连接与超时",
          })}
        >
          <div className="grid gap-4 md:grid-cols-3">
            {[
              {
                key: "timeoutMs" as const,
                label: t("providerBundle.requestTimeout", {
                  defaultValue: "请求超时（毫秒）",
                }),
                max: 3_600_000,
              },
              {
                key: "streamFirstByteTimeoutMs" as const,
                label: t("providerBundle.firstByteTimeout", {
                  defaultValue: "首字节超时（毫秒）",
                }),
                max: 600_000,
              },
              {
                key: "streamIdleTimeoutMs" as const,
                label: t("providerBundle.streamIdleTimeout", {
                  defaultValue: "流空闲超时（毫秒）",
                }),
                max: 3_600_000,
              },
            ].map(({ key, label, max }) => (
              <div key={key} className="space-y-2">
                <Label>{label}</Label>
                <Input
                  type="number"
                  min={1_000}
                  max={max}
                  step={1_000}
                  value={draft.transport[key]}
                  placeholder={
                    runtimeDefaults
                      ? String(runtimeDefaults.transport[key])
                      : undefined
                  }
                  className="focus:placeholder:text-transparent"
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
              </div>
            ))}
          </div>
        </Section>

        {codexDriverOptions ? (
          <Section title={t("codexOauth.featureOptionsTitle")}>
            <CodexFeatureOptions
              values={{
                codexFastMode: draft.surfaces.some(
                  (surface) => surface.driverOptions.codexFastMode === true,
                ),
                codexImageGenerationEnabled: draft.surfaces.some(
                  (surface) =>
                    surface.driverOptions.codexImageGenerationEnabled === true,
                ),
                codexWebsocketEnabled: draft.surfaces.some(
                  (surface) =>
                    surface.driverOptions.codexWebsocketEnabled === true,
                ),
              }}
              onChange={setDriverOption}
            />
          </Section>
        ) : null}

        {codexDriverOptions &&
        persisted &&
        draft.accountId &&
        draft.expectedRevision != null ? (
          <Section title={t("codexReferrals.sectionTitle")}>
            <CodexReferralPanel
              providerId={draft.id}
              expectedRevision={draft.expectedRevision}
            />
          </Section>
        ) : null}
          </CollapsibleContent>
        </Collapsible>

        {showSurfaceTabs || family.endpointScope === "surface" ? (
        <Section
          title={t("providerBundle.surfaces", { defaultValue: "应用接口" })}
        >
          {showSurfaceTabs ? (
          <Tabs
            value={activeApp}
            onValueChange={(value) => setActiveApp(value as CoreProviderApp)}
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
                  <AppLogo app={surface.app} />
                  <span className="truncate">{APP_LABELS[surface.app]}</span>
                  {surface.enabled ? (
                    <Check className="h-3.5 w-3.5" />
                  ) : (
                    <span className="text-[10px] text-muted-foreground">
                      {t("providerBundle.surfaceOff", { defaultValue: "关" })}
                    </span>
                  )}
                </TabsTrigger>
              ))}
            </TabsList>
            {draft.surfaces.map((surface) => (
              <TabsContent key={surface.app} value={surface.app}>
                <SurfaceEditor
                  surface={surface}
                  modelPolicyScope={draft.modelPolicyScope}
                  validation={validation}
                  onChange={updateSurface}
                />
              </TabsContent>
            ))}
          </Tabs>
          ) : activeSurface ? (
            <SurfaceEditor
              surface={activeSurface}
              modelPolicyScope={draft.modelPolicyScope}
              validation={validation}
              onChange={updateSurface}
            />
          ) : null}
        </Section>
        ) : null}
          </div>
        ) : null}

        {showShareStep ? (
        <div id="bundle-section-share" ref={shareSectionRef}>
          <BundleShareEditor
            draft={shareDraft}
            onChange={setShareDraft}
            ownerEmail={ownerEmail}
            shareUrl={shareUrl}
            onOpenShareSettings={onOpenShareSettings}
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
            onClick={() =>
              setCreateStep(createStep === "share" ? "supply" : "family")
            }
            disabled={saving}
          >
            {t("common.previous")}
          </Button>
        ) : null}
        {!persisted && createStep !== "share" ? (
          <Button
            type="button"
            onClick={() =>
              setCreateStep(createStep === "family" ? "supply" : "share")
            }
            disabled={saving}
          >
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
        isOpen={pendingFamilyId !== null}
        title={t("providerBundle.changeFamilyTitle", {
          defaultValue: "更换供应商类型？",
        })}
        message={t("providerBundle.changeFamilyMessage", {
          defaultValue: "未保存的供给和分享设置将被新的类型默认值替换。",
        })}
        confirmText={t("providerBundle.changeFamilyConfirm", {
          defaultValue: "更换类型",
        })}
        cancelText={t("common.cancel")}
        variant="destructive"
        zIndex="top"
        onConfirm={() => {
          if (pendingFamilyId) applyFamily(pendingFamilyId);
          setPendingFamilyId(null);
        }}
        onCancel={() => setPendingFamilyId(null)}
      />
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
