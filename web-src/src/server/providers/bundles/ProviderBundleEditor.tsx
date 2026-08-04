import { useEffect, useMemo, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  ArrowLeft,
  ArrowRightLeft,
  Check,
  Copy,
  KeyRound,
  LoaderCircle,
  Plus,
  Save,
  Share2,
  Target,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";

import { ClaudeIcon, CodexIcon, GeminiIcon } from "@/components/BrandIcons";
import { ProviderIcon } from "@/components/ProviderIcon";
import { ProviderIconControl } from "@/components/providers/ProviderIconControl";
import { ManagedAccountSection } from "@/components/providers/forms/ManagedAccountSection";
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import type {
  ProviderBundleView,
  ProviderCustomBinding,
} from "@/lib/api/providers";
import { providersApi } from "@/lib/api/providers";
import { shareApi } from "@/lib/api/share";
import { normalizeManagedAuthProvider } from "@/lib/authBinding";
import { copyText } from "@/lib/clipboard";
import {
  managedAccountKeys,
  shareKeys,
  useManagedAccountsQuery,
  useSharesQuery,
} from "@/lib/query";
import {
  customPolicyForProfile,
  driverForProfile,
  familyById,
  profileById,
  providerRegistry,
  type CoreProviderApp,
  type ProviderFamilySpec,
} from "@/server/providerRegistry";
import {
  createDraftForProfile,
  profileAllowsEndpointEditing,
} from "@/server/providers/editor/providerDraft";
import { SecretInput } from "@/server/ui/SecretInput";
import { cn } from "@/lib/utils";
import { SHARE_TOKEN_PRESETS } from "@/utils/shareFormUtils";
import { DEFAULT_PARALLEL_LIMIT } from "@/utils/shareUtils";
import {
  BUNDLE_SHARE_EXPIRY_PRESETS,
  createBundleShareDraft,
  saveBundleShare,
  shareForBundle,
  type ProviderBundleShareDraft,
} from "./bundleShare";
import {
  createProviderBundleDraft,
  duplicateProviderBundleDraft,
  editProviderBundleDraft,
  familyCredentialSlots,
  modelPoliciesForFamily,
  parseSettings,
  providerBundleIdentityEditable,
  surfaceEndpoint,
  toProviderBundleWriteDraft,
  updateBundleModel,
  updateSurfaceEndpoint,
  validateProviderBundleDraft,
  type BundleSurfaceEditorDraft,
  type ProviderBundleEditorDraft,
} from "./bundleDraft";

interface ProviderBundleEditorProps {
  bundle?: ProviderBundleView;
  duplicate?: boolean;
  initialSection?: "share";
  onCancel: () => void;
  onSaved: (bundle: ProviderBundleView) => void;
  onOpenShareSettings?: () => void;
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

function FamilyLogo({ family }: { family: ProviderFamilySpec }) {
  const profile = profileById(family.credentialProfileId);
  const preset = profile ? createDraftForProfile(profile) : undefined;
  return (
    <ProviderIcon
      icon={preset?.icon}
      name={family.label}
      color={preset?.iconColor}
      size={16}
      className="shrink-0"
      showFallback
    />
  );
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
    <section className="space-y-4 border-b border-border/60 pb-6 last:border-0">
      <h2 className="flex items-center gap-2 text-sm font-semibold">
        {icon}
        {title}
      </h2>
      {children}
    </section>
  );
}

function fieldLabel(logical: string): string {
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

function surfaceSettingsValid(surface: BundleSurfaceEditorDraft): boolean {
  try {
    parseSettings(surface.settingsText);
    return true;
  } catch {
    return false;
  }
}

function SurfaceEditor({
  surface,
  onChange,
}: {
  surface: BundleSurfaceEditorDraft;
  onChange: (surface: BundleSurfaceEditorDraft) => void;
}) {
  const { t } = useTranslation();
  const profile = profileById(surface.profileId);
  if (!profile) return null;
  const settingsValid = surfaceSettingsValid(surface);
  const customPolicy = customPolicyForProfile(profile);
  const endpoint = settingsValid ? surfaceEndpoint(surface) : "";

  const updateMeta = (patch: Record<string, unknown>) =>
    onChange({
      ...surface,
      meta: { ...surface.meta, ...patch },
    });

  return (
    <div className="space-y-6 pt-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <Badge variant="outline">{profile.label}</Badge>
          <span className="text-xs text-muted-foreground">
            {profile.profileId}
          </span>
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

      {profile.formComposition === "custom" && customPolicy ? (
        <div className="space-y-5">
          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2 md:col-span-2">
              <Label>{t("serverProviderForm.endpoint.url")}</Label>
              <Input
                type="url"
                value={endpoint}
                disabled={!settingsValid}
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
                      {protocol}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-2">
              <Label>{t("serverProviderForm.binding.authScheme")}</Label>
              <Select
                value={surface.customBinding?.authScheme}
                onValueChange={(authScheme) =>
                  onChange({
                    ...surface,
                    customBinding: {
                      ...(surface.customBinding ?? {
                        upstreamProtocol: customPolicy
                          .protocols[0]! as ProviderCustomBinding["upstreamProtocol"],
                      }),
                      authScheme:
                        authScheme as ProviderCustomBinding["authScheme"],
                    },
                  })
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

          <div className="space-y-2">
            <Label>
              {t("providerBundle.surfaceCredential", {
                defaultValue: "认证密钥",
              })}
            </Label>
            <SecretInput
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
              value={surface.meta.customUserAgent ?? ""}
              onChange={(event) =>
                updateMeta({ customUserAgent: event.target.value })
              }
            />
          </div>
        </div>
      ) : null}

      <div className="space-y-2">
        <Label>
          {t("providerBundle.advancedSettings", {
            defaultValue: "高级设置 JSON",
          })}
        </Label>
        <Textarea
          className={cn(
            "min-h-44 font-mono text-xs",
            !settingsValid &&
              "border-destructive focus-visible:ring-destructive",
          )}
          spellCheck={false}
          value={surface.settingsText}
          onChange={(event) =>
            onChange({ ...surface, settingsText: event.target.value })
          }
        />
      </div>
    </div>
  );
}

function BundleShareEditor({
  draft,
  onChange,
  shareUrl,
  onOpenShareSettings,
}: {
  draft: ProviderBundleShareDraft;
  onChange: (draft: ProviderBundleShareDraft) => void;
  shareUrl?: string | null;
  onOpenShareSettings?: () => void;
}) {
  const { t } = useTranslation();
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
            <Label>
              {t("provider.share.subdomain", { defaultValue: "分享子域名" })}
            </Label>
            <Input
              value={draft.subdomain}
              onChange={(event) =>
                onChange({ ...draft, subdomain: event.target.value })
              }
            />
          </div>
          <div className="space-y-2">
            <Label>
              {t("provider.share.forSale", { defaultValue: "访问模式" })}
            </Label>
            <Select
              value={draft.forSale}
              onValueChange={(forSale) =>
                onChange({
                  ...draft,
                  forSale: forSale as ProviderBundleShareDraft["forSale"],
                })
              }
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
            <Label>
              {t("provider.share.sharedWith", { defaultValue: "授权邮箱" })}
            </Label>
            <Input
              value={draft.sharedWithEmails}
              onChange={(event) =>
                onChange({ ...draft, sharedWithEmails: event.target.value })
              }
            />
          </div>
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
          <div className="space-y-2 md:col-span-2">
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
            <div className="flex flex-wrap gap-1.5">
              <Button
                type="button"
                variant={draft.expiry === "permanent" ? "secondary" : "outline"}
                size="sm"
                className="h-7 px-2 text-xs"
                onClick={() => onChange({ ...draft, expiry: "permanent" })}
              >
                {t("share.expiry.permanent", { defaultValue: "永久有效" })}
              </Button>
              {BUNDLE_SHARE_EXPIRY_PRESETS.map((preset) => (
                <Button
                  key={preset.value}
                  type="button"
                  variant={
                    draft.expiry === preset.value ? "secondary" : "outline"
                  }
                  size="sm"
                  className="h-7 px-2 text-xs"
                  onClick={() => onChange({ ...draft, expiry: preset.value })}
                >
                  {t(preset.labelKey)}
                </Button>
              ))}
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
  const initialFamily = providerRegistry.families[0]!;
  const [draft, setDraft] = useState<ProviderBundleEditorDraft>(() =>
    bundle
      ? duplicate
        ? duplicateProviderBundleDraft(bundle)
        : editProviderBundleDraft(bundle)
      : createProviderBundleDraft(initialFamily),
  );
  const persisted =
    (Boolean(bundle) && !duplicate) || draft.expectedRevision !== undefined;
  const family = familyById(draft.familyId) ?? initialFamily;
  const identityEditable = providerBundleIdentityEditable(family);
  const [activeApp, setActiveApp] = useState<CoreProviderApp>(
    draft.surfaces[0]?.app ?? "claude",
  );
  const [saving, setSaving] = useState(false);
  const shareSectionRef = useRef<HTMLDivElement>(null);
  const sharesQuery = useSharesQuery();
  const existingShare = shareForBundle(sharesQuery.data, draft.id);
  const [shareDraft, setShareDraft] = useState<ProviderBundleShareDraft>(() =>
    createBundleShareDraft(existingShare),
  );
  const accountsQuery = useManagedAccountsQuery();
  const credentialProfile = profileById(family.credentialProfileId);
  const allowedModelPolicies = modelPoliciesForFamily(family);
  const defaultSharedModel =
    credentialProfile?.defaultUpstreamModel ??
    family.surfaces
      .map((surface) => profileById(surface.profileId)?.defaultUpstreamModel)
      .find(Boolean) ??
    "";
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

  useEffect(() => {
    if (!draft.accountId || draft.accountGeneration != null) return;
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
  }, [accounts, draft.accountGeneration, draft.accountId]);

  useEffect(() => {
    setShareDraft(createBundleShareDraft(existingShare));
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
    void shareApi.suggestShareSlug().then((result) => {
      if (active)
        setShareDraft((current) => ({
          ...current,
          subdomain: result.subdomain,
        }));
    });
    return () => {
      active = false;
    };
  }, [existingShare, shareDraft.enabled, shareDraft.subdomain]);

  const changeFamily = (familyId: string) => {
    if (persisted) return;
    const next = familyById(familyId);
    if (!next) return;
    const nextDraft = createProviderBundleDraft(next);
    setDraft(nextDraft);
    setActiveApp(nextDraft.surfaces[0]?.app ?? "claude");
    setShareDraft(createBundleShareDraft());
  };

  const updateSurface = (next: BundleSurfaceEditorDraft) =>
    setDraft((current) => ({
      ...current,
      surfaces: current.surfaces.map((surface) =>
        surface.app === next.app ? next : surface,
      ),
    }));

  const setDriverOption = (key: string, checked: boolean) =>
    setDraft((current) => ({
      ...current,
      surfaces: current.surfaces.map((surface) => {
        const profile = profileById(surface.profileId);
        if (driverForProfile(profile!)?.driverId !== "oauth.openai_codex")
          return surface;
        return { ...surface, meta: { ...surface.meta, [key]: checked } };
      }),
    }));

  const submit = async () => {
    const validation = validateProviderBundleDraft(draft);
    if (validation) {
      toast.error(validation);
      return;
    }
    setSaving(true);
    try {
      const saved = await providersApi.upsertBundle(
        toProviderBundleWriteDraft(draft),
      );
      setDraft((current) => ({
        ...current,
        expectedRevision: saved.revision,
        clientRequestId: undefined,
        secrets: Object.fromEntries(
          Object.entries(current.secrets).map(([slot, secret]) => [
            slot,
            {
              ...secret,
              configured: secret.configured || Boolean(secret.value.trim()),
              value: "",
              clear: false,
            },
          ]),
        ),
      }));
      await saveBundleShare(saved.id, shareDraft, existingShare);
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

  return (
    <div className="mx-auto flex w-full max-w-5xl flex-col gap-6 pb-24">
      <div className="flex items-center gap-3">
        <Button
          type="button"
          size="icon"
          variant="outline"
          onClick={onCancel}
          title={t("common.back")}
        >
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <div className="min-w-0">
          <h1 className="truncate text-lg font-semibold">
            {persisted
              ? t("providerBundle.edit", { defaultValue: "编辑供应商" })
              : t("providerBundle.create", { defaultValue: "新建供应商" })}
          </h1>
          <div className="mt-1 flex items-center gap-2">
            {family.surfaces.map((surface) => (
              <AppLogo key={surface.app} app={surface.app} />
            ))}
          </div>
        </div>
      </div>

      <div className="space-y-6">
        <Section
          title={t("providerBundle.basic", { defaultValue: "基本信息" })}
        >
          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2 md:col-span-2">
              {persisted ? (
                <div className="flex justify-center">
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
              ) : (
                <div className="space-y-2">
                  <Label>
                    {t("providerBundle.family", {
                      defaultValue: "供应商类型",
                    })}
                  </Label>
                  <div
                    role="radiogroup"
                    aria-label={t("providerBundle.family", {
                      defaultValue: "供应商类型",
                    })}
                    className="grid grid-cols-[repeat(auto-fill,minmax(150px,1fr))] gap-2"
                  >
                    {providerRegistry.families.map((item) => {
                      const selected = item.familyId === draft.familyId;
                      return (
                        <button
                          key={item.familyId}
                          type="button"
                          role="radio"
                          aria-checked={selected}
                          onClick={() => changeFamily(item.familyId)}
                          className={cn(
                            "inline-flex min-h-10 w-full items-center justify-start gap-2 rounded-md px-3 py-2 text-left text-sm font-medium transition-colors",
                            selected
                              ? "bg-primary text-primary-foreground"
                              : "bg-accent text-muted-foreground hover:bg-accent/80 hover:text-foreground",
                          )}
                        >
                          <FamilyLogo family={item} />
                          <span className="truncate">{item.label}</span>
                        </button>
                      );
                    })}
                  </div>
                </div>
              )}
            </div>
            <div className="space-y-2">
              <Label>{t("serverProviderForm.basic.name")}</Label>
              <Input
                value={draft.name}
                readOnly={!identityEditable}
                className={cn(
                  !identityEditable &&
                    "cursor-default bg-muted/40 text-muted-foreground",
                )}
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
                return (
                  <div key={logical} className="space-y-2">
                    <Label>{fieldLabel(logical)}</Label>
                    <SecretInput
                      value={secret.value}
                      placeholder={secret.configured ? "••••••••" : undefined}
                      onChange={(event) =>
                        setDraft({
                          ...draft,
                          secrets: {
                            ...draft.secrets,
                            [actualPointer]: {
                              ...secret,
                              value: event.target.value,
                              clear: false,
                            },
                          },
                        })
                      }
                    />
                  </div>
                );
              })}
              {credentialProfile?.formComposition === "aws" ? (
                <div className="space-y-2">
                  <Label>AWS Region</Label>
                  <Input
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
          <div className="grid gap-4 md:grid-cols-[minmax(0,22rem)_minmax(0,1fr)] md:items-start">
            <div className="space-y-2">
              {allowedModelPolicies.length > 1 ? (
                <Tabs
                  value={draft.modelPolicy}
                  onValueChange={(value) => {
                    if (value !== "single" && value !== "passthrough") return;
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
                        className="min-w-0 gap-2 rounded-sm px-3 data-[state=active]:bg-background data-[state=active]:text-foreground dark:data-[state=active]:bg-background"
                      >
                        {policy === "single" ? (
                          <Target className="hidden h-4 w-4 shrink-0 sm:block" />
                        ) : (
                          <ArrowRightLeft className="hidden h-4 w-4 shrink-0 sm:block" />
                        )}
                        {policy === "single"
                          ? t("providerBundle.modelSingle", {
                              defaultValue: "固定上游模型",
                            })
                          : t("providerBundle.modelPassthrough", {
                              defaultValue: "模型透传",
                            })}
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
                    ? t("providerBundle.modelSingle", {
                        defaultValue: "固定上游模型",
                      })
                    : t("providerBundle.modelPassthrough", {
                        defaultValue: "模型透传",
                      })}
                </div>
              )}
              <p className="text-xs leading-relaxed text-muted-foreground">
                {draft.modelPolicy === "single"
                  ? t("serverProviderForm.model.singleHint")
                  : t("serverProviderForm.model.passthroughHint")}
              </p>
            </div>

            {draft.modelPolicy === "single" ? (
              <div className="space-y-2">
                <Label htmlFor="provider-bundle-model">
                  {t("serverProviderForm.model.upstreamModel")}
                </Label>
                <Input
                  id="provider-bundle-model"
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
          </div>
        </Section>

        {commonEndpointEditable ? (
          <Section title={t("serverProviderForm.endpoint.title")}>
            <Input
              type="url"
              value={draft.endpoint}
              onChange={(event) =>
                setDraft({ ...draft, endpoint: event.target.value })
              }
            />
          </Section>
        ) : null}

        {codexDriverOptions ? (
          <Section
            title={t("providerBundle.driverOptions", {
              defaultValue: "运行选项",
            })}
          >
            <div className="grid gap-3 sm:grid-cols-3">
              {[
                ["FAST", "codexFastMode"],
                [
                  t("serverProviderForm.codex.imageGeneration"),
                  "codexImageGenerationEnabled",
                ],
                ["WebSocket", "codexWebsocketEnabled"],
              ].map(([label, key]) => {
                const checked = draft.surfaces.some((surface) =>
                  Boolean((surface.meta as Record<string, unknown>)[key]),
                );
                return (
                  <div
                    key={key}
                    className="flex items-center justify-between rounded-md border px-3 py-2"
                  >
                    <Label>{label}</Label>
                    <Switch
                      checked={checked}
                      onCheckedChange={(value) => setDriverOption(key, value)}
                    />
                  </div>
                );
              })}
            </div>
          </Section>
        ) : null}

        <Section
          title={t("providerBundle.surfaces", { defaultValue: "API Surface" })}
        >
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
                  {surface.enabled ? <Check className="h-3.5 w-3.5" /> : null}
                </TabsTrigger>
              ))}
            </TabsList>
            {draft.surfaces.map((surface) => (
              <TabsContent key={surface.app} value={surface.app}>
                <SurfaceEditor surface={surface} onChange={updateSurface} />
              </TabsContent>
            ))}
          </Tabs>
        </Section>

        <div ref={shareSectionRef}>
          <BundleShareEditor
            draft={shareDraft}
            onChange={setShareDraft}
            shareUrl={shareUrl}
            onOpenShareSettings={onOpenShareSettings}
          />
        </div>
      </div>

      <div className="sticky bottom-0 z-20 flex items-center justify-end gap-2 border-t bg-background/95 py-4 backdrop-blur">
        <Button
          type="button"
          variant="outline"
          onClick={onCancel}
          disabled={saving}
        >
          {t("common.cancel")}
        </Button>
        <Button type="button" onClick={() => void submit()} disabled={saving}>
          {saving ? (
            <LoaderCircle className="mr-2 h-4 w-4 animate-spin" />
          ) : (
            <Save className="mr-2 h-4 w-4" />
          )}
          {t("common.save")}
        </Button>
      </div>
    </div>
  );
}
