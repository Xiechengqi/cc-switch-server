import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ChevronDown,
  ChevronRight,
  Copy,
  ExternalLink,
  Loader2,
  Save,
  Share2,
} from "lucide-react";
import type { AppId, ShareReuseCandidate } from "@/lib/api";
import type {
  ShareUserGrantMap,
  ShareUserPolicy,
  ShareUserUsageEditMap,
  ShareTotalUsageEdit,
  ShareRecord,
} from "@/lib/api/share";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { Badge } from "@/components/ui/badge";
import { Switch } from "@/components/ui/switch";
import { Checkbox } from "@/components/ui/checkbox";
import { SubdomainGeneratorButton } from "@/components/SubdomainGeneratorButton";
import { ShareUserGrantsEditor } from "@/components/providers/ShareUserGrantsEditor";
import { ProviderShareReuseDialog } from "@/components/providers/ProviderShareReuseDialog";
import { shareApi } from "@/lib/api/share";
import { copyText } from "@/lib/clipboard";
import { stableStringify } from "@/lib/stableStringify";
import { extractErrorMessage } from "@/utils/errorUtils";
import { toast } from "sonner";
import { cn } from "@/lib/utils";
import {
  useClientTunnelQuery,
  useAddShareBindingMutation,
  useCreateShareMutation,
  useEnableShareMutation,
  useRemoveShareBindingMutation,
  useSaveProviderShareMutation,
  useSettingsQuery,
} from "@/lib/query";
import {
  getProviderShareState,
  isShareableApp,
  resolveShareOwnerEmail,
  useProviderShare,
  type ProviderShareState,
} from "@/hooks/useProviderShare";
import { isShareRunning } from "@/utils/shareUtils";
import {
  DEFAULT_PARALLEL_LIMIT,
  formatShareLimitInput,
  getTunnelConfigFromSettings,
  isPermanentExpiry,
  normalizeShareLimitValue,
  PERMANENT_EXPIRES_AT,
  permanentExpiresInSecs,
  UNLIMITED_PARALLEL_LIMIT,
  UNLIMITED_TOKEN_LIMIT,
} from "@/utils/shareUtils";
import { formatShareRouterDisplay } from "@/utils/shareRouter";
import {
  buildShareUserGrants,
  isValidShareEmail,
  normalizeShareEmails,
  SHARE_EXPIRY_PRESETS,
  SHARE_TOKEN_PRESETS,
  shareAppDisplayLabel,
  uniqueSortedEmails,
} from "@/utils/shareFormUtils";

/** Shown on the add-provider form before a provider id exists. */
export function ProviderSharePlaceholder() {
  const { t } = useTranslation();

  return (
    <div className="rounded-lg border border-dashed border-border/50 bg-muted/10">
      <div className="flex items-center justify-between gap-4 p-4">
        <div className="flex min-w-0 items-center gap-3">
          <Share2 className="h-4 w-4 shrink-0 text-muted-foreground/70" />
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <span className="font-medium text-muted-foreground">
              {t("provider.share.sectionTitle", { defaultValue: "远程分享" })}
            </span>
            <Badge variant="outline" className="text-muted-foreground">
              {t("provider.share.addPageBadge", { defaultValue: "保存后可用" })}
            </Badge>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2 opacity-50">
          <Label
            htmlFor="provider-share-placeholder"
            className="text-sm text-muted-foreground"
          >
            {t("provider.share.enableShare", { defaultValue: "启用远程分享" })}
          </Label>
          <Switch id="provider-share-placeholder" checked={false} disabled />
        </div>
      </div>
      <div className="border-t border-border/40 px-4 pb-4 pt-3">
        <p className="text-sm text-muted-foreground">
          {t("provider.share.addPagePlaceholder", {
            defaultValue:
              "请先保存供应商；保存后重新打开编辑页即可配置远程分享。",
          })}
        </p>
      </div>
    </div>
  );
}

interface ProviderShareSectionProps {
  appId: AppId;
  providerId: string;
  providerName: string;
  onOpenShareSettings?: () => void;
  onDirtyChange?: (dirty: boolean) => void;
}

function shareStateLabel(
  state: ProviderShareState,
  t: (key: string, options?: Record<string, unknown>) => string,
) {
  if (state === "active") {
    return t("provider.share.stateActive", { defaultValue: "分享已启用" });
  }
  if (state === "paused") {
    return t("provider.share.statePaused", { defaultValue: "分享已暂停" });
  }
  if (state === "error") {
    return t("provider.share.stateError", { defaultValue: "分享异常" });
  }
  return t("provider.share.stateNone", { defaultValue: "未启用分享" });
}

function shareStateVariant(
  state: ProviderShareState,
): "default" | "secondary" | "destructive" | "outline" {
  if (state === "active") return "default";
  if (state === "paused") return "secondary";
  if (state === "error") return "destructive";
  return "outline";
}

export function ProviderShareSection({
  appId,
  providerId,
  providerName,
  onOpenShareSettings,
  onDirtyChange,
}: ProviderShareSectionProps) {
  const { t } = useTranslation();
  const { share, state } = useProviderShare(appId, providerId);
  const { data: clientTunnel } = useClientTunnelQuery();
  const { data: settings } = useSettingsQuery();
  const tunnelConfig = useMemo(
    () => getTunnelConfigFromSettings(settings),
    [settings],
  );

  const createMutation = useCreateShareMutation();
  const addBindingMutation = useAddShareBindingMutation();
  const removeBindingMutation = useRemoveShareBindingMutation();
  const enableMutation = useEnableShareMutation();
  const saveMutation = useSaveProviderShareMutation();

  const [isShareOpen, setIsShareOpen] = useState(false);
  const [reuseCandidates, setReuseCandidates] = useState<
    ShareReuseCandidate[] | null
  >(null);
  // Limit/expiry touched flags live in refs. Incrementing this signal guarantees
  // every same-value interaction still causes a fingerprint render.
  const [, setShareDraftRevision] = useState(0);
  const markShareDraftChanged = () => {
    setShareDraftRevision((current) => current + 1);
  };
  const [shareDraftBaseline, setShareDraftBaseline] = useState<{
    key: string;
    fingerprint: string;
  } | null>(null);

  const [subdomainInput, setSubdomainInput] = useState("");
  const [descriptionInput, setDescriptionInput] = useState("");
  const [freeAccess, setFreeAccess] = useState(false);
  const [userGrants, setUserGrants] = useState<ShareUserGrantMap>({});
  const [userUsageEdits, setUserUsageEdits] =
    useState<ShareUserUsageEditMap>({});
  const [tokenLimitInput, setTokenLimitInput] = useState("");
  const [tokensUsedInput, setTokensUsedInput] = useState("");
  const [parallelLimitInput, setParallelLimitInput] = useState("");
  const [expiresInSecsInput, setExpiresInSecsInput] = useState(
    String(permanentExpiresInSecs()),
  );
  const [isPermanent, setIsPermanent] = useState(true);
  const [allowPersonalCredits, setAllowPersonalCredits] = useState(false);
  const [autoConsumeBankedReset, setAutoConsumeBankedReset] = useState(false);
  const [bankedResetExpiryLeadMinutes, setBankedResetExpiryLeadMinutes] =
    useState("60");
  const [previousResponseCacheEnabled, setPreviousResponseCacheEnabled] =
    useState(true);

  const subdomainManualRef = useRef(false);
  const shareInitRef = useRef<string | null>(null);
  const tokenLimitTouchedRef = useRef(false);
  const tokensUsedTouchedRef = useRef(false);
  const parallelLimitTouchedRef = useRef(false);
  const expiresTouchedRef = useRef(false);

  const shareableApp = isShareableApp(appId) ? appId : null;
  const shareExists = Boolean(share);
  const shareRunning = share ? isShareRunning(share) : false;
  const routerSyncPending = Boolean(
    share &&
    (share.descriptorGeneration === 0 ||
      share.routerSyncedDescriptorGeneration !== share.descriptorGeneration ||
      share.routerSyncedDescriptorFingerprint !== share.descriptorFingerprint),
  );
  const ownerEmail = useMemo(
    () => resolveShareOwnerEmail(clientTunnel?.config?.ownerEmail),
    [clientTunnel?.config?.ownerEmail],
  );

  const routerConsoleUrl = useMemo(() => {
    const domain = tunnelConfig.domain;
    if (!domain) return null;
    const host = domain.split(":")[0] ?? domain;
    const isLocal =
      host === "localhost" || host === "127.0.0.1" || host === "0.0.0.0";
    return `${isLocal ? "http" : "https"}://${domain}`;
  }, [tunnelConfig.domain]);

  useEffect(() => {
    if (!shareableApp) return;
    const initKey = share?.id ?? "new";
    if (shareInitRef.current === initKey) return;
    shareInitRef.current = initKey;
    tokenLimitTouchedRef.current = false;
    tokensUsedTouchedRef.current = false;
    parallelLimitTouchedRef.current = false;
    expiresTouchedRef.current = false;

    setDescriptionInput(share?.description?.trim() ?? "");
    setFreeAccess(share?.freeAccess ?? false);
    setSubdomainInput(share?.shareSlug?.trim() ?? "");
    setAllowPersonalCredits(share?.allowPersonalCredits ?? false);
    setAutoConsumeBankedReset(share?.autoConsumeBankedReset ?? false);
    setBankedResetExpiryLeadMinutes(
      String(share?.bankedResetExpiryLeadMinutes ?? 60),
    );
    setPreviousResponseCacheEnabled(
      share ? Boolean(share.previousResponseCacheEnabled) : true,
    );
    subdomainManualRef.current = Boolean(share?.shareSlug?.trim());

    const existingGrants = share?.userGrants ?? {};

    const defaultPolicy: ShareUserPolicy = {
      parallelLimit:
        share && normalizeShareLimitValue(share.parallelLimit) >= 0
          ? normalizeShareLimitValue(share.parallelLimit)
          : undefined,
      tokenLimit:
        share && normalizeShareLimitValue(share.tokenLimit) >= 0
          ? normalizeShareLimitValue(share.tokenLimit)
          : undefined,
      tokenPeriod: "lifetime",
      expiresAt:
        share?.expiresAt && !isPermanentExpiry(share.expiresAt)
          ? new Date(share.expiresAt).getTime()
          : undefined,
    };
    const nextGrants: ShareUserGrantMap = { ...existingGrants };
    const normalizedOwner = ownerEmail.trim().toLowerCase();
    if (normalizedOwner && !nextGrants[normalizedOwner]) {
      nextGrants[normalizedOwner] = {
        email: normalizedOwner,
        role: "owner",
        active: true,
        policy: { ...defaultPolicy },
      };
    }
    setUserGrants(nextGrants);
    setUserUsageEdits({});

    setTokenLimitInput(formatShareLimitInput(share?.tokenLimit));
    setTokensUsedInput(String(Math.max(0, share?.tokensUsed ?? 0)));
    setParallelLimitInput(formatShareLimitInput(share?.parallelLimit));
    const permanent = share ? isPermanentExpiry(share.expiresAt) : true;
    setIsPermanent(permanent);
    if (share?.expiresAt && !permanent) {
      const remaining = Math.max(
        1,
        Math.floor((new Date(share.expiresAt).getTime() - Date.now()) / 1000),
      );
      setExpiresInSecsInput(String(remaining));
    } else {
      setExpiresInSecsInput(String(permanentExpiresInSecs()));
    }
  }, [share, shareableApp, ownerEmail]);

  useEffect(() => {
    if (shareExists || subdomainManualRef.current || share) return;
    let active = true;
    void shareApi
      .suggestShareSlug()
      .then((outcome) => {
        if (active && !subdomainManualRef.current) {
          setSubdomainInput(outcome.subdomain);
        }
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, [shareExists, share]);

  const busy =
    createMutation.isPending ||
    addBindingMutation.isPending ||
    removeBindingMutation.isPending ||
    enableMutation.isPending ||
    saveMutation.isPending ||
    reuseCandidates !== null;

  const shareDraftInitializationKey = `${appId}:${providerId}:${share?.id ?? "new"}`;
  const shareDraftFingerprint = stableStringify({
    subdomain: subdomainInput.trim(),
    description: descriptionInput.trim(),
    freeAccess,
    userGrants,
    userUsageEdits,
    tokenLimit:
      !tokenLimitTouchedRef.current && share
        ? normalizeShareLimitValue(share.tokenLimit)
        : tokenLimitInput.trim()
          ? Number(tokenLimitInput)
          : UNLIMITED_TOKEN_LIMIT,
    tokensUsed: tokensUsedTouchedRef.current ? tokensUsedInput.trim() : null,
    parallelLimit:
      !parallelLimitTouchedRef.current && share
        ? normalizeShareLimitValue(share.parallelLimit)
        : parallelLimitInput.trim()
          ? Number(parallelLimitInput)
          : UNLIMITED_PARALLEL_LIMIT,
    expiry:
      !expiresTouchedRef.current && share?.expiresAt
        ? isPermanentExpiry(share.expiresAt)
          ? { permanent: true }
          : { persisted: share.expiresAt }
        : isPermanent
          ? { permanent: true }
          : { permanent: false, seconds: expiresInSecsInput.trim() },
    allowPersonalCredits,
    autoConsumeBankedReset,
    bankedResetExpiryLeadMinutes,
    previousResponseCacheEnabled,
  });
  const shareDraftFingerprintRef = useRef(shareDraftFingerprint);
  shareDraftFingerprintRef.current = shareDraftFingerprint;

  useEffect(() => {
    let secondFrame: number | null = null;
    const firstFrame = requestAnimationFrame(() => {
      secondFrame = requestAnimationFrame(() => {
        setShareDraftBaseline({
          key: shareDraftInitializationKey,
          fingerprint: shareDraftFingerprintRef.current,
        });
      });
    });

    return () => {
      cancelAnimationFrame(firstFrame);
      if (secondFrame !== null) cancelAnimationFrame(secondFrame);
    };
  }, [shareDraftInitializationKey]);

  const shareDraftDirty = Boolean(
    share &&
    shareDraftBaseline?.key === shareDraftInitializationKey &&
    shareDraftBaseline.fingerprint !== shareDraftFingerprint,
  );

  useEffect(() => {
    onDirtyChange?.(shareDraftDirty);
  }, [onDirtyChange, shareDraftDirty]);

  useEffect(
    () => () => {
      onDirtyChange?.(false);
    },
    [onDirtyChange],
  );

  if (!shareableApp) {
    return null;
  }

  const normalizedOwnerEmail = ownerEmail.trim().toLowerCase();
  const ownerEmailInvalid =
    !normalizedOwnerEmail || !isValidShareEmail(normalizedOwnerEmail);
  const resolveTokenLimit = () =>
    tokenLimitInput.trim() ? Number(tokenLimitInput) : UNLIMITED_TOKEN_LIMIT;
  const resolveParallelLimit = () =>
    parallelLimitInput.trim()
      ? Number(parallelLimitInput)
      : UNLIMITED_PARALLEL_LIMIT;
  const resolveBankedResetLeadMinutes = () =>
    Number(bankedResetExpiryLeadMinutes);
  const bankedResetLeadIsValid = (value: number) =>
    Number.isSafeInteger(value) && value >= 10 && value <= 10080;

  /**
   * The Share total counter is only ever sent when the operator actually
   * edited it.  An untouched field must not resend the value the editor was
   * opened with: requests that landed since then would be silently erased.
   */
  const resolveShareUsageEditForSave = (): ShareTotalUsageEdit | undefined => {
    if (!tokensUsedTouchedRef.current) return undefined;
    const raw = tokensUsedInput.trim();
    if (!raw) return undefined;
    const tokensUsed = Number(raw);
    if (!Number.isSafeInteger(tokensUsed) || tokensUsed < 0) return undefined;
    if (share && tokensUsed === share.tokensUsed) return undefined;
    return tokensUsed === 0
      ? { action: "clear" }
      : { action: "set", tokensUsed };
  };

  const shareUsageEditIsInvalid = () => {
    if (!tokensUsedTouchedRef.current) return false;
    const raw = tokensUsedInput.trim();
    if (!raw) return false;
    const tokensUsed = Number(raw);
    return !Number.isSafeInteger(tokensUsed) || tokensUsed < 0;
  };

  const resolveTokenLimitForSave = () => {
    if (!tokenLimitTouchedRef.current && share) {
      return normalizeShareLimitValue(share.tokenLimit);
    }
    return resolveTokenLimit();
  };

  const resolveParallelLimitForSave = () => {
    if (!parallelLimitTouchedRef.current && share) {
      return normalizeShareLimitValue(share.parallelLimit);
    }
    return resolveParallelLimit();
  };

  const resolveExpiresAtForSave = () => {
    if (!expiresTouchedRef.current && share?.expiresAt) return share.expiresAt;
    return resolveExpiresAt();
  };

  const resolveExpiresAt = () => {
    if (isPermanent) return PERMANENT_EXPIRES_AT;
    const seconds = Number(expiresInSecsInput);
    if (!Number.isFinite(seconds) || seconds <= 0) {
      return new Date(Date.now() + 24 * 3600 * 1000).toISOString();
    }
    return new Date(Date.now() + seconds * 1000).toISOString();
  };

  const userGrantDefaultPolicy: ShareUserPolicy = {
    parallelLimit:
      resolveParallelLimitForSave() >= 0
        ? resolveParallelLimitForSave()
        : undefined,
    tokenLimit:
      resolveTokenLimitForSave() >= 0 ? resolveTokenLimitForSave() : undefined,
    tokenPeriod: "lifetime",
    expiresAt: isPermanent ? undefined : new Date(resolveExpiresAt()).getTime(),
  };

  const routerManagedGrantEmails = new Set(
    Object.values(userGrants)
      .filter((grant) => grant.manager === "routerShareMarket")
      .map((grant) => grant.email.trim().toLowerCase()),
  );
  const protectedGrantEmails = routerManagedGrantEmails;

  const activeShareToEmails = (grants: ShareUserGrantMap) =>
    normalizeShareEmails(
      Object.values(grants)
        .filter((grant) => grant.active !== false && grant.role === "shareto")
        .map((grant) => grant.email),
    );

  const canonicalUserGrants = (grants: ShareUserGrantMap) =>
    buildShareUserGrants({
      source: grants,
      ownerEmail: normalizedOwnerEmail,
      aclEmails: activeShareToEmails(grants),
      defaultPolicy: userGrantDefaultPolicy,
    });

  const canonicalUserUsageEdits = (grants: ShareUserGrantMap) => {
    const allowed = new Set(
      Object.values(grants)
        .filter(
          (grant) =>
            grant.active !== false && grant.manager !== "routerShareMarket",
        )
        .map((grant) => grant.email.trim().toLowerCase()),
    );
    return Object.fromEntries(
      Object.entries(userUsageEdits).filter(([email]) =>
        allowed.has(email.trim().toLowerCase()),
      ),
    );
  };

  const displayedUserGrants = buildShareUserGrants({
    source: userGrants,
    ownerEmail: normalizedOwnerEmail,
    aclEmails: activeShareToEmails(userGrants),
    defaultPolicy: userGrantDefaultPolicy,
  });

  const handleUserGrantsChange = (nextGrants: ShareUserGrantMap) => {
    setUserGrants(nextGrants);
    markShareDraftChanged();
  };

  const handleUserUsageEditsChange = (nextEdits: ShareUserUsageEditMap) => {
    setUserUsageEdits(nextEdits);
    markShareDraftChanged();
  };

  const createConfiguredShare = async () => {
    if (ownerEmailInvalid) {
      toast.error(
        t("share.validation.invalidEmail", { defaultValue: "邮箱格式无效" }),
      );
      return;
    }
    const tokenLimit = resolveTokenLimitForSave();
    const parallelLimit = resolveParallelLimitForSave();
    const resetLeadMinutes = resolveBankedResetLeadMinutes();
    if (Number.isNaN(tokenLimit) || Number.isNaN(parallelLimit)) {
      toast.error(
        t("provider.share.invalidNumber", { defaultValue: "请输入有效数字" }),
      );
      return;
    }
    if (!bankedResetLeadIsValid(resetLeadMinutes)) {
      toast.error(t("codexSharePolicy.resetLeadInvalid"));
      return;
    }

    const expiresAt = resolveExpiresAt();
    const expiresAtMs = Date.parse(expiresAt);
    if (!Number.isFinite(expiresAtMs)) {
      toast.error(
        t("provider.share.invalidNumber", {
          defaultValue: "请输入有效数字",
        }),
      );
      return;
    }

    const payloadUserGrants = canonicalUserGrants(userGrants);
    const created = await createMutation.mutateAsync({
      bindings: { [shareableApp]: providerId },
      freeAccess,
      tokenLimit,
      parallelLimit,
      expiresAt: expiresAtMs,
      subdomain: subdomainInput.trim() || undefined,
      description: descriptionInput.trim() || undefined,
      allowPersonalCredits,
      autoConsumeBankedReset,
      bankedResetExpiryLeadMinutes: resetLeadMinutes,
      previousResponseCacheEnabled,
      userGrants: payloadUserGrants,
    });
    return created;
  };

  const handleCreate = async () => {
    if (ownerEmailInvalid) {
      toast.error(
        t("share.validation.invalidEmail", { defaultValue: "邮箱格式无效" }),
      );
      return;
    }
    let candidates: ShareReuseCandidate[];
    try {
      candidates = await shareApi.listReuseCandidates(shareableApp, providerId);
    } catch (error) {
      toast.error(
        t("share.toggle.enableFailed", {
          defaultValue: "开启分享失败：{{error}}",
          error: extractErrorMessage(error),
        }),
      );
      return;
    }
    if (candidates.length > 0) {
      setReuseCandidates(candidates);
      return;
    }
    return createConfiguredShare();
  };

  const confirmShareReuse = async (reuse: boolean, shareId: string) => {
    const candidate = reuseCandidates?.find((item) => item.shareId === shareId);
    setReuseCandidates(null);
    if (!reuse || !candidate) {
      await createConfiguredShare();
      return;
    }
    await addBindingMutation.mutateAsync({
      shareId: candidate.shareId,
      app: shareableApp,
      providerId,
      expectedConfigRevision: candidate.configRevision,
    });
  };

  const handleSave = async (): Promise<boolean> => {
    if (busy) return false;
    if (!share || !shareDraftDirty) return true;
    if (ownerEmailInvalid) {
      toast.error(
        t("share.validation.invalidEmail", { defaultValue: "邮箱格式无效" }),
      );
      return false;
    }
    const tokenLimit = resolveTokenLimitForSave();
    const parallelLimit = resolveParallelLimitForSave();
    const resetLeadMinutes = resolveBankedResetLeadMinutes();
    if (Number.isNaN(tokenLimit) || Number.isNaN(parallelLimit)) {
      toast.error(
        t("provider.share.invalidNumber", { defaultValue: "请输入有效数字" }),
      );
      return false;
    }
    if (shareUsageEditIsInvalid()) {
      toast.error(
        t("provider.share.invalidNumber", { defaultValue: "请输入有效数字" }),
      );
      return false;
    }
    if (!bankedResetLeadIsValid(resetLeadMinutes)) {
      toast.error(t("codexSharePolicy.resetLeadInvalid"));
      return false;
    }

    const nextExpiresAt = resolveExpiresAtForSave();
    const payloadUserGrants = canonicalUserGrants(userGrants);
    const payloadUserUsageEdits = canonicalUserUsageEdits(payloadUserGrants);
    const saved = (await saveMutation.mutateAsync({
      shareId: share.id,
      expectedConfigRevision: share.configRevision,
      subdomain: subdomainInput.trim(),
      description: descriptionInput.trim() || undefined,
      freeAccess,
      tokenLimit,
      parallelLimit,
      expiresAt: nextExpiresAt,
      allowPersonalCredits,
      autoConsumeBankedReset,
      bankedResetExpiryLeadMinutes: resetLeadMinutes,
      previousResponseCacheEnabled,
      userGrants: payloadUserGrants,
      userUsageEdits: payloadUserUsageEdits,
      shareUsageEdit: resolveShareUsageEditForSave(),
    })) as ShareRecord;
    if (saved?.userGrants) setUserGrants(saved.userGrants);
    setUserUsageEdits({});
    tokensUsedTouchedRef.current = false;
    setTokensUsedInput(String(Math.max(0, saved?.tokensUsed ?? 0)));
    setShareDraftBaseline({
      key: shareDraftInitializationKey,
      fingerprint: shareDraftFingerprintRef.current,
    });
    return true;
  };

  const handleShareToggle = async (checked: boolean) => {
    if (busy) return;
    if (checked) {
      if (!share) {
        await handleCreate();
        return;
      }
      if (!isShareRunning(share)) {
        await enableMutation.mutateAsync(share.id);
      }
      return;
    }
    if (share && isShareRunning(share)) {
      await removeBindingMutation.mutateAsync({
        shareId: share.id,
        app: shareableApp,
        providerId,
        expectedConfigRevision: share.configRevision,
      });
    }
  };

  const tunnelLabel =
    share?.tunnelUrl || share?.subdomain
      ? formatShareRouterDisplay(share.tunnelUrl || share.subdomain || "")
      : null;
  const clientSubdomain = clientTunnel?.config?.subdomain?.trim() ?? "";
  const shareSlugPreview = subdomainInput.trim();
  const routerHost =
    tunnelConfig.domain.split(":")[0]?.trim() || tunnelConfig.domain.trim();
  const shareHostPreview =
    clientSubdomain && shareSlugPreview && routerHost
      ? `${shareSlugPreview}--${clientSubdomain}.${routerHost}`
      : null;

  return (
    <div className="rounded-lg border border-border/50 bg-muted/20">
      <button
        type="button"
        className="flex w-full items-center justify-between p-4 hover:bg-muted/30 transition-colors"
        onClick={() => setIsShareOpen(!isShareOpen)}
      >
        <div className="flex items-center gap-3">
          <Share2 className="h-4 w-4 text-muted-foreground" />
          <div className="flex items-center gap-2">
            <span className="font-medium">
              {t("provider.share.sectionTitle", { defaultValue: "远程分享" })}
            </span>
            <Badge variant={shareStateVariant(state)}>
              {shareStateLabel(state, t)}
            </Badge>
          </div>
        </div>
        <div className="flex items-center gap-3">
          <div
            className="flex items-center gap-2"
            onClick={(event) => event.stopPropagation()}
          >
            <Label
              htmlFor="provider-share-enabled"
              className="text-sm text-muted-foreground"
            >
              {t("provider.share.enableShare", {
                defaultValue: "启用远程分享",
              })}
            </Label>
            <Switch
              id="provider-share-enabled"
              checked={shareRunning}
              disabled={busy}
              onCheckedChange={(checked) => void handleShareToggle(checked)}
            />
          </div>
          {busy ? (
            <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
          ) : isShareOpen ? (
            <ChevronDown className="h-4 w-4 text-muted-foreground" />
          ) : (
            <ChevronRight className="h-4 w-4 text-muted-foreground" />
          )}
        </div>
      </button>

      <div
        className={cn(
          "overflow-hidden transition-all duration-200",
          isShareOpen ? "max-h-[5000px] opacity-100" : "max-h-0 opacity-0",
        )}
      >
        <div
          hidden={!isShareOpen}
          className="space-y-4 border-t border-border/50 p-4"
        >
          <p className="text-sm text-muted-foreground">
            {t("provider.share.sectionHint", {
              defaultValue:
                "每个 Provider 对应一个 Share。在此配置分享参数；Router Console 可管理运营侧高级选项。",
            })}
          </p>
          {routerConsoleUrl ? (
            <a
              href={routerConsoleUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex w-fit items-center gap-1 text-xs font-medium text-primary hover:underline"
            >
              {t("provider.share.openRouterConsole", {
                defaultValue: "打开 Router Console",
              })}
              <ExternalLink className="h-3 w-3" />
            </a>
          ) : null}

          {!shareExists ? (
            <p className="text-sm text-muted-foreground">
              {t("provider.share.disabledHint", {
                defaultValue: "开启后可配置远程分享参数并创建 Share。",
              })}
            </p>
          ) : (
            <>
              <div className="grid gap-4 md:grid-cols-2">
                <div className="space-y-2 md:col-span-2">
                  <Label htmlFor="provider-share-router">
                    {t("share.tunnel.region", { defaultValue: "路由节点" })}
                  </Label>
                  <div className="flex flex-col gap-2 rounded-lg border border-border/60 bg-muted/30 px-3 py-2 sm:flex-row sm:items-center sm:justify-between">
                    <p
                      id="provider-share-router"
                      className="text-sm font-medium"
                    >
                      {formatShareRouterDisplay(tunnelConfig.domain)}
                    </p>
                    {onOpenShareSettings ? (
                      <Button
                        type="button"
                        variant="link"
                        size="sm"
                        className="h-auto shrink-0 px-0"
                        onClick={onOpenShareSettings}
                      >
                        {t("provider.share.openShareSettings", {
                          defaultValue: "前往设置修改",
                        })}
                      </Button>
                    ) : null}
                  </div>
                  <p className="text-xs text-muted-foreground">
                    {shareExists
                      ? t("share.routerLockedAfterCreate", {
                          defaultValue: "路由节点已绑定。",
                        })
                      : t("provider.share.routerFromSettingsHint", {
                          defaultValue:
                            "使用设置 → 分享中的默认 Router 节点创建 share。",
                        })}
                  </p>
                </div>

                <div className="space-y-2">
                  <Label htmlFor="provider-share-owner-email">
                    {t("share.ownerEmail", { defaultValue: "Owner Email" })}
                  </Label>
                  <Input
                    id="provider-share-owner-email"
                    type="email"
                    value={ownerEmail}
                    disabled
                    readOnly
                    placeholder={t("provider.share.ownerNotConfigured", {
                      defaultValue: "请先配置 Client Owner",
                    })}
                  />
                  <p className="text-xs text-muted-foreground">
                    {t("provider.share.ownerManagedHint", {
                      defaultValue:
                        "Share Owner 与 Client Owner 保持一致。可在「设置 → 分享」通过安装签名的 Owner 换绑进行修改。",
                    })}
                  </p>
                  {ownerEmail && ownerEmailInvalid ? (
                    <p className="text-xs text-destructive">
                      {t("share.validation.invalidEmail", {
                        defaultValue: "邮箱格式无效",
                      })}
                    </p>
                  ) : null}
                </div>

                <div className="space-y-2">
                  <Label htmlFor="provider-share-subdomain">
                    {t("share.shareSlug", { defaultValue: "Share slug" })}
                  </Label>
                  <div className="flex items-center gap-2">
                    <Input
                      id="provider-share-subdomain"
                      value={subdomainInput}
                      disabled={busy}
                      placeholder="my-share"
                      onChange={(event) => {
                        subdomainManualRef.current = true;
                        setSubdomainInput(event.target.value);
                        markShareDraftChanged();
                      }}
                    />
                    <SubdomainGeneratorButton
                      disabled={busy}
                      onGenerated={(value) => {
                        subdomainManualRef.current = true;
                        setSubdomainInput(value);
                        markShareDraftChanged();
                      }}
                      onError={(message) => toast.error(message)}
                      suggest={() => shareApi.suggestShareSlug()}
                    />
                  </div>
                  <div className="space-y-1">
                    <p className="text-xs text-muted-foreground">
                      {t("share.subdomainHint")}
                    </p>
                    {shareHostPreview ? (
                      <p className="text-xs font-mono text-muted-foreground">
                        {t("share.shareHostPreview", {
                          defaultValue: "Public host: {{host}}",
                          host: shareHostPreview,
                        })}
                      </p>
                    ) : null}
                    <p className="text-xs text-muted-foreground">
                      {t("share.subdomainEditHint")}
                    </p>
                  </div>
                </div>

                <div className="space-y-2 md:col-span-2">
                  <Label htmlFor="provider-share-description">
                    {t("share.description", { defaultValue: "描述" })}
                  </Label>
                  <Textarea
                    id="provider-share-description"
                    rows={2}
                    maxLength={200}
                    value={descriptionInput}
                    placeholder={providerName}
                    disabled={busy}
                    onChange={(event) => {
                      setDescriptionInput(event.target.value);
                      markShareDraftChanged();
                    }}
                  />
                </div>

                <div className="space-y-2 md:col-span-2">
                  <div className="flex items-center gap-2">
                    <Checkbox
                      id="provider-share-free-access"
                      checked={freeAccess}
                      disabled={busy}
                      onCheckedChange={(checked) => {
                        setFreeAccess(checked === true);
                        markShareDraftChanged();
                      }}
                    />
                    <Label
                      htmlFor="provider-share-free-access"
                      className="cursor-pointer font-normal"
                    >
                      {t("share.freeAccess.label", {
                        defaultValue: "公开免费使用",
                      })}
                    </Label>
                  </div>
                  <p className="text-xs text-muted-foreground">
                    {t("share.freeAccess.hint", {
                      defaultValue:
                        "默认私有。勾选后，任意已登录 Router 用户可免费调用；下方授权用户仍可设置个人配额。",
                    })}
                  </p>
                </div>

                <ShareUserGrantsEditor
                  value={displayedUserGrants}
                  ownerEmail={normalizedOwnerEmail}
                  defaultPolicy={userGrantDefaultPolicy}
                  protectedEmails={protectedGrantEmails}
                  usageEdits={userUsageEdits}
                  onUsageEditsChange={handleUserUsageEditsChange}
                  disabled={busy}
                  onChange={handleUserGrantsChange}
                />

                <div className="grid gap-4 md:col-span-2 md:grid-cols-3">
                  <div className="space-y-2">
                    <Label htmlFor="provider-share-token-limit">
                      {t("share.tokenLimit", { defaultValue: "Token 限额" })}
                    </Label>
                    <Input
                      id="provider-share-token-limit"
                      type="number"
                      min={0}
                      disabled={busy}
                      placeholder={t("share.unlimited", {
                        defaultValue: "无上限",
                      })}
                      value={tokenLimitInput}
                      onChange={(event) => {
                        tokenLimitTouchedRef.current = true;
                        setTokenLimitInput(event.target.value);
                        markShareDraftChanged();
                      }}
                    />
                    <div className="flex flex-wrap gap-1.5">
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        className="h-7 px-2 text-xs"
                        disabled={busy}
                        onClick={() => {
                          tokenLimitTouchedRef.current = true;
                          setTokenLimitInput("");
                          markShareDraftChanged();
                        }}
                      >
                        {t("share.unlimited", { defaultValue: "无上限" })}
                      </Button>
                      {SHARE_TOKEN_PRESETS.map((preset) => (
                        <Button
                          key={preset}
                          type="button"
                          variant="outline"
                          size="sm"
                          className="h-7 px-2 text-xs"
                          disabled={busy}
                          onClick={() => {
                            tokenLimitTouchedRef.current = true;
                            setTokenLimitInput(String(preset));
                            markShareDraftChanged();
                          }}
                        >
                          {preset.toLocaleString()}
                        </Button>
                      ))}
                    </div>
                    <div className="space-y-1.5 pt-1">
                      <Label htmlFor="provider-share-tokens-used">
                        {t("share.totalUsageEdit.label")}
                      </Label>
                      <Input
                        id="provider-share-tokens-used"
                        type="number"
                        min={0}
                        step={1}
                        disabled={busy || !shareExists}
                        value={tokensUsedInput}
                        onChange={(event) => {
                          tokensUsedTouchedRef.current = true;
                          setTokensUsedInput(event.target.value);
                          markShareDraftChanged();
                        }}
                      />
                      <p className="text-xs text-muted-foreground">
                        {t("share.totalUsageEdit.hint")}
                      </p>
                    </div>
                  </div>

                  <div className="space-y-2">
                    <Label htmlFor="provider-share-parallel-limit">
                      {t("share.parallelLimit", { defaultValue: "并发限额" })}
                    </Label>
                    <Input
                      id="provider-share-parallel-limit"
                      type="number"
                      min={1}
                      disabled={busy}
                      placeholder={t("share.unlimited", {
                        defaultValue: "无上限",
                      })}
                      value={parallelLimitInput}
                      onChange={(event) => {
                        parallelLimitTouchedRef.current = true;
                        setParallelLimitInput(event.target.value);
                        markShareDraftChanged();
                      }}
                    />
                    <div className="flex flex-wrap gap-1.5">
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        className="h-7 px-2 text-xs"
                        disabled={busy}
                        onClick={() => {
                          parallelLimitTouchedRef.current = true;
                          setParallelLimitInput("");
                          markShareDraftChanged();
                        }}
                      >
                        {t("share.unlimited", { defaultValue: "无上限" })}
                      </Button>
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        className="h-7 px-2 text-xs"
                        disabled={busy}
                        onClick={() => {
                          parallelLimitTouchedRef.current = true;
                          setParallelLimitInput(String(DEFAULT_PARALLEL_LIMIT));
                          markShareDraftChanged();
                        }}
                      >
                        {DEFAULT_PARALLEL_LIMIT}
                      </Button>
                    </div>
                  </div>

                  <div className="space-y-2">
                    <Label htmlFor="provider-share-expires">
                      {t("provider.share.expiry", { defaultValue: "有效期" })}
                    </Label>
                    <Input
                      id="provider-share-expires"
                      type="number"
                      disabled={busy || isPermanent}
                      value={expiresInSecsInput}
                      onChange={(event) => {
                        expiresTouchedRef.current = true;
                        setExpiresInSecsInput(event.target.value);
                        markShareDraftChanged();
                      }}
                    />
                    <div className="flex flex-wrap gap-1.5">
                      {SHARE_EXPIRY_PRESETS.map((preset) => (
                        <Button
                          key={preset.value}
                          type="button"
                          variant="outline"
                          size="sm"
                          className="h-7 px-2 text-xs"
                          disabled={busy || isPermanent}
                          onClick={() => {
                            expiresTouchedRef.current = true;
                            setExpiresInSecsInput(String(preset.value));
                            markShareDraftChanged();
                          }}
                        >
                          {t(preset.labelKey)}
                        </Button>
                      ))}
                    </div>
                    <div className="flex items-center gap-2">
                      <Checkbox
                        id="provider-share-expires-permanent"
                        checked={isPermanent}
                        disabled={busy}
                        onCheckedChange={(checked) => {
                          expiresTouchedRef.current = true;
                          markShareDraftChanged();
                          const next = checked === true;
                          setIsPermanent(next);
                          if (next) {
                            setExpiresInSecsInput(
                              String(permanentExpiresInSecs()),
                            );
                          } else {
                            setExpiresInSecsInput(String(24 * 3600));
                          }
                        }}
                      />
                      <Label
                        htmlFor="provider-share-expires-permanent"
                        className="cursor-pointer text-sm font-normal"
                      >
                        {t("share.expiry.permanent", {
                          defaultValue: "永久有效",
                        })}
                      </Label>
                    </div>
                  </div>
                </div>
              </div>

              {tunnelLabel ? (
                <div className="flex flex-wrap items-center gap-2 rounded-lg border bg-background px-3 py-2 text-sm">
                  <span className="font-mono text-xs">{tunnelLabel}</span>
                  <Button
                    type="button"
                    size="sm"
                    variant="ghost"
                    onClick={() => void copyText(tunnelLabel)}
                  >
                    <Copy className="h-3.5 w-3.5" />
                  </Button>
                </div>
              ) : null}

              {share && (share.routerLastSyncError || routerSyncPending) ? (
                <p
                  className={cn(
                    "text-xs",
                    share.routerLastSyncError
                      ? "text-destructive"
                      : "text-muted-foreground",
                  )}
                >
                  {share.routerLastSyncError
                    ? t("provider.share.routerSyncFailed", {
                        defaultValue: "Router 同步失败：{{error}}",
                        error: share.routerLastSyncError,
                      })
                    : t("provider.share.routerSyncPending", {
                        defaultValue: "正在同步到 Router",
                      })}
                </p>
              ) : null}

              <div className="flex justify-end border-t border-border/50 pt-4">
                <Button
                  type="button"
                  disabled={busy || !shareDraftDirty}
                  onClick={() => void handleSave()}
                >
                  {saveMutation.isPending ? (
                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  ) : (
                    <Save className="mr-2 h-4 w-4" />
                  )}
                  {t("common.save", { defaultValue: "保存" })}
                </Button>
              </div>
            </>
          )}
        </div>
      </div>

      <ProviderShareReuseDialog
        candidates={reuseCandidates}
        onConfirm={(reuse, shareId) => {
          void confirmShareReuse(reuse, shareId);
        }}
        onCancel={() => setReuseCandidates(null)}
      />
    </div>
  );
}

export { getProviderShareState };
