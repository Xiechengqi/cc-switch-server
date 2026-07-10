import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ChevronDown,
  ChevronRight,
  Copy,
  ExternalLink,
  Loader2,
  Pause,
  Play,
  Share2,
  Trash2,
  X,
} from "lucide-react";
import type { AppId, PublicMarket, ShareSaleMarketKind } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { Badge } from "@/components/ui/badge";
import { Switch } from "@/components/ui/switch";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { EmailTagsInput } from "@/components/ui/tags-input";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { copyText } from "@/lib/clipboard";
import { toast } from "sonner";
import { cn } from "@/lib/utils";
import {
  useClientTunnelQuery,
  useCreateShareMutation,
  useDeleteShareMutation,
  useDisableShareMutation,
  useEnableShareMutation,
  usePauseShareMutation,
  useResumeShareMutation,
  useSaveProviderShareMutation,
  useSettingsQuery,
  useShareMarketsQuery,
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
  MIN_PARALLEL_LIMIT,
  normalizeShareLimitValue,
  PERMANENT_EXPIRES_AT,
  permanentExpiresInSecs,
  UNLIMITED_PARALLEL_LIMIT,
  UNLIMITED_TOKEN_LIMIT,
} from "@/utils/shareUtils";
import { formatShareRouterDisplay } from "@/utils/shareRouter";
import {
  buildShareAclPayload,
  deriveSubdomainFromEmail,
  formatMarketSelectLabel,
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
            defaultValue: "请先保存供应商；保存后重新打开编辑页即可配置远程分享。",
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

function splitShareToEmails(
  emails: string[],
  marketEmailSet: Set<string>,
): string[] {
  return emails.filter((email) => !marketEmailSet.has(email.toLowerCase()));
}

export function ProviderShareSection({
  appId,
  providerId,
  providerName,
  onOpenShareSettings,
}: ProviderShareSectionProps) {
  const { t } = useTranslation();
  const { share, state, data: shares = [] } = useProviderShare(appId, providerId);
  const { data: clientTunnel } = useClientTunnelQuery();
  const { data: settings } = useSettingsQuery();
  const tunnelConfig = useMemo(
    () => getTunnelConfigFromSettings(settings),
    [settings],
  );

  const createMutation = useCreateShareMutation();
  const deleteMutation = useDeleteShareMutation();
  const enableMutation = useEnableShareMutation();
  const disableMutation = useDisableShareMutation();
  const pauseMutation = usePauseShareMutation();
  const resumeMutation = useResumeShareMutation();
  const saveMutation = useSaveProviderShareMutation();

  const [isShareOpen, setIsShareOpen] = useState(false);
  const [confirmFreeOpen, setConfirmFreeOpen] = useState(false);

  const [ownerEmailInput, setOwnerEmailInput] = useState("");
  const [subdomainInput, setSubdomainInput] = useState("");
  const [descriptionInput, setDescriptionInput] = useState("");
  const [forSaleValue, setForSaleValue] = useState<"Yes" | "No" | "Free">("Yes");
  const [saleMarketKind, setSaleMarketKind] = useState<ShareSaleMarketKind>("token");
  const [marketAccessMode, setMarketAccessMode] = useState<"selected" | "all">("all");
  const [selectedMarketEmails, setSelectedMarketEmails] = useState<string[]>([]);
  const [selectedShareMarketEmail, setSelectedShareMarketEmail] = useState("");
  const [marketSelectKey, setMarketSelectKey] = useState(0);
  const [shareToEmails, setShareToEmails] = useState<string[]>([]);
  const [tokenLimitInput, setTokenLimitInput] = useState("");
  const [parallelLimitInput, setParallelLimitInput] = useState("");
  const [expiresInSecsInput, setExpiresInSecsInput] = useState(
    String(permanentExpiresInSecs()),
  );
  const [isPermanent, setIsPermanent] = useState(true);

  const subdomainManualRef = useRef(false);
  const shareInitRef = useRef<string | null>(null);
  const tokenLimitTouchedRef = useRef(false);
  const parallelLimitTouchedRef = useRef(false);
  const expiresTouchedRef = useRef(false);

  const shareableApp = isShareableApp(appId) ? appId : null;
  const shareExists = Boolean(share);
  const shareRunning = share ? isShareRunning(share) : false;
  const marketsQueryEnabled = shareExists && isShareOpen;
  const { data: markets = [], isLoading: marketsLoading, error: marketsError, refetch: refetchMarkets } =
    useShareMarketsQuery(marketsQueryEnabled);

  const usageMarkets = useMemo(
    () => markets.filter((market) => (market.marketKind ?? "usage") !== "share"),
    [markets],
  );
  const shareMarkets = useMemo(
    () => markets.filter((market) => market.marketKind === "share"),
    [markets],
  );
  const usageMarketEmailSet = useMemo(
    () => new Set(usageMarkets.map((market) => market.email.toLowerCase())),
    [usageMarkets],
  );
  const shareMarketEmailSet = useMemo(
    () => new Set(shareMarkets.map((market) => market.email.toLowerCase())),
    [shareMarkets],
  );
  const allMarketEmailSet = useMemo(
    () => new Set([...usageMarketEmailSet, ...shareMarketEmailSet]),
    [usageMarketEmailSet, shareMarketEmailSet],
  );

  const ownerEmail = useMemo(
    () => resolveShareOwnerEmail(clientTunnel?.config?.ownerEmail, shares),
    [clientTunnel?.config?.ownerEmail, shares],
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
    parallelLimitTouchedRef.current = false;
    expiresTouchedRef.current = false;

    const resolvedOwner = share?.ownerEmail?.trim() || ownerEmail;
    setOwnerEmailInput(resolvedOwner);
    setDescriptionInput(share?.description?.trim() ?? "");
    setForSaleValue(share?.forSale ?? "Yes");
    setSaleMarketKind(share?.saleMarketKind ?? "token");
    setMarketAccessMode(share?.marketAccessMode ?? "all");
    setSubdomainInput(share?.subdomain?.trim() ?? "");
    subdomainManualRef.current = Boolean(share?.subdomain?.trim());

    const appAccess = share?.accessByApp?.[shareableApp];
    const allEmails = normalizeShareEmails(
      appAccess?.sharedWithEmails ?? share?.sharedWithEmails ?? [],
    );
    setShareToEmails(splitShareToEmails(allEmails, allMarketEmailSet));

    const tokenEmails = allEmails.filter((email) =>
      usageMarketEmailSet.has(email),
    );
    setSelectedMarketEmails(tokenEmails);
    const shareMarketEmail = allEmails.find((email) =>
      shareMarketEmailSet.has(email),
    );
    setSelectedShareMarketEmail(shareMarketEmail ?? "");

    setTokenLimitInput(formatShareLimitInput(share?.tokenLimit));
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
  }, [
    share,
    shareableApp,
    ownerEmail,
    allMarketEmailSet,
    usageMarketEmailSet,
    shareMarketEmailSet,
  ]);

  useEffect(() => {
    if (!shareExists || subdomainManualRef.current || share) return;
    setSubdomainInput(deriveSubdomainFromEmail(ownerEmailInput));
  }, [shareExists, ownerEmailInput, share]);

  const busy =
    createMutation.isPending ||
    deleteMutation.isPending ||
    enableMutation.isPending ||
    disableMutation.isPending ||
    pauseMutation.isPending ||
    resumeMutation.isPending ||
    saveMutation.isPending;

  if (!shareableApp) {
    return null;
  }

  const normalizedOwnerEmail = ownerEmailInput.trim().toLowerCase();
  const ownerEmailInvalid =
    !normalizedOwnerEmail || !isValidShareEmail(normalizedOwnerEmail);
  const shareToInvalid = shareToEmails.some(
    (email) => email && !isValidShareEmail(email),
  );
  const marketDisabled = forSaleValue !== "Yes";
  const normalizedSelectedMarketEmails = uniqueSortedEmails(
    selectedMarketEmails
      .map((email) => email.trim().toLowerCase())
      .filter((email) => usageMarketEmailSet.has(email)),
  );
  const normalizedSelectedShareMarketEmail = shareMarketEmailSet.has(
    selectedShareMarketEmail.trim().toLowerCase(),
  )
    ? selectedShareMarketEmail.trim().toLowerCase()
    : forSaleValue === "Yes" && saleMarketKind === "share"
      ? (shareMarkets[0]?.email?.trim().toLowerCase() ?? "")
      : "";
  const marketInvalid =
    forSaleValue === "Yes" &&
    saleMarketKind === "share" &&
    normalizedSelectedShareMarketEmail.length === 0;

  const resolveTokenLimit = () =>
    tokenLimitInput.trim() ? Number(tokenLimitInput) : UNLIMITED_TOKEN_LIMIT;
  const resolveParallelLimit = () =>
    parallelLimitInput.trim()
      ? Number(parallelLimitInput)
      : UNLIMITED_PARALLEL_LIMIT;

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

  const buildAclPayload = (
    tokenLimit: number,
    parallelLimit: number,
    expiresAt: string,
  ) =>
    buildShareAclPayload({
      app: shareableApp,
      forSale: forSaleValue,
      saleMarketKind,
      marketAccessMode,
      shareToEmails,
      selectedTokenMarketEmails: normalizedSelectedMarketEmails,
      selectedShareMarketEmail: normalizedSelectedShareMarketEmail,
      tokenLimit,
      parallelLimit,
      expiresAt,
    });

  const handleCreate = async () => {
    if (ownerEmailInvalid) {
      toast.error(
        t("share.validation.invalidEmail", { defaultValue: "邮箱格式无效" }),
      );
      return;
    }
    if (shareToInvalid || marketInvalid) return;

    const tokenLimit = resolveTokenLimitForSave();
    const parallelLimit = resolveParallelLimitForSave();
    if (Number.isNaN(tokenLimit) || Number.isNaN(parallelLimit)) {
      toast.error(
        t("provider.share.invalidNumber", { defaultValue: "请输入有效数字" }),
      );
      return;
    }

    const aclPayload = buildAclPayload(
      tokenLimit,
      parallelLimit,
      resolveExpiresAt(),
    );
    const created = await createMutation.mutateAsync({
      ownerEmail: normalizedOwnerEmail,
      bindings: { [shareableApp]: providerId },
      forSale: forSaleValue,
      saleMarketKind,
      tokenLimit,
      parallelLimit,
      expiresInSecs: isPermanent
        ? permanentExpiresInSecs()
        : Math.max(1, Number(expiresInSecsInput) || 3600),
      subdomain: subdomainInput.trim() || undefined,
      description: descriptionInput.trim() || undefined,
      sharedWithEmails: aclPayload.sharedWithEmails,
      marketAccessMode: aclPayload.marketAccessMode,
      accessByApp: aclPayload.accessByApp,
      appSettings: aclPayload.appSettings,
    });
    return created;
  };

  const handleSave = async () => {
    if (!share) return;
    if (ownerEmailInvalid || shareToInvalid || marketInvalid) return;

    const tokenLimit = resolveTokenLimitForSave();
    const parallelLimit = resolveParallelLimitForSave();
    if (Number.isNaN(tokenLimit) || Number.isNaN(parallelLimit)) {
      toast.error(
        t("provider.share.invalidNumber", { defaultValue: "请输入有效数字" }),
      );
      return;
    }

    const nextExpiresAt = resolveExpiresAtForSave();
    const aclPayload = buildAclPayload(
      tokenLimit,
      parallelLimit,
      nextExpiresAt,
    );
    await saveMutation.mutateAsync({
      shareId: share.id,
      ownerEmail: normalizedOwnerEmail,
      subdomain: subdomainInput.trim(),
      description: descriptionInput.trim() || undefined,
      forSale: forSaleValue,
      saleMarketKind,
      sharedWithEmails: aclPayload.sharedWithEmails,
      marketAccessMode: aclPayload.marketAccessMode,
      accessByApp: aclPayload.accessByApp,
      appSettings: aclPayload.appSettings,
      tokenLimit,
      parallelLimit,
      expiresAt: nextExpiresAt,
    });
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
      await disableMutation.mutateAsync(share.id);
    }
  };

  const tunnelLabel = share?.tunnelUrl || share?.subdomain
    ? formatShareRouterDisplay(share.tunnelUrl || share.subdomain || "")
    : null;

  const marketsErrorMessage =
    marketsError instanceof Error ? marketsError.message : undefined;

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
              {t("provider.share.enableShare", { defaultValue: "启用远程分享" })}
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
        <div className="space-y-4 border-t border-border/50 p-4">
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
                    value={ownerEmailInput}
                    disabled={busy}
                    onChange={(event) => setOwnerEmailInput(event.target.value)}
                    placeholder="owner@example.com"
                  />
                  {ownerEmailInput.trim() && ownerEmailInvalid ? (
                    <p className="text-xs text-destructive">
                      {t("share.validation.invalidEmail", {
                        defaultValue: "邮箱格式无效",
                      })}
                    </p>
                  ) : null}
                </div>

                <div className="space-y-2">
                  <Label htmlFor="provider-share-subdomain">
                    {t("share.subdomain", { defaultValue: "子域名" })}
                  </Label>
                  <Input
                    id="provider-share-subdomain"
                    value={subdomainInput}
                    disabled={busy}
                    placeholder="my-share"
                    onChange={(event) => {
                      subdomainManualRef.current = true;
                      setSubdomainInput(event.target.value);
                    }}
                  />
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
                    onChange={(event) => setDescriptionInput(event.target.value)}
                  />
                </div>

                <div className="space-y-2">
                  <Label htmlFor="provider-share-for-sale">
                    {t("share.forSale", { defaultValue: "For Sale" })}
                  </Label>
                  <Select
                    value={forSaleValue}
                    disabled={busy}
                    onValueChange={(value) => {
                      const next = value as "Yes" | "No" | "Free";
                      if (next === "Free") {
                        setConfirmFreeOpen(true);
                      } else {
                        setForSaleValue(next);
                      }
                    }}
                  >
                    <SelectTrigger id="provider-share-for-sale">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="No">
                        {t("share.forSaleOptions.no", { defaultValue: "否" })}
                      </SelectItem>
                      <SelectItem value="Yes">
                        {t("share.forSaleOptions.yes", { defaultValue: "是" })}
                      </SelectItem>
                      <SelectItem value="Free">
                        {t("share.forSaleOptions.free", { defaultValue: "免费" })}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>

                <div className="space-y-2">
                  <Label>
                    {t("share.saleMarketKind.title", { defaultValue: "Market 类型" })}
                  </Label>
                  <div className="flex flex-wrap items-center gap-4 pt-1">
                    {(["token", "share"] as const).map((value) => (
                      <label
                        key={value}
                        htmlFor={`provider-share-market-kind-${value}`}
                        className={cn(
                          "flex cursor-pointer items-center gap-2 text-sm",
                          marketDisabled && "cursor-not-allowed opacity-60",
                        )}
                      >
                        <input
                          id={`provider-share-market-kind-${value}`}
                          type="radio"
                          name="provider-share-market-kind"
                          value={value}
                          checked={saleMarketKind === value}
                          disabled={marketDisabled || busy}
                          onChange={() => {
                            setSaleMarketKind(value);
                            if (value === "token") {
                              setMarketAccessMode("all");
                              setSelectedMarketEmails([]);
                              setSelectedShareMarketEmail("");
                            } else {
                              setMarketAccessMode("selected");
                              setSelectedMarketEmails([]);
                            }
                            setMarketSelectKey((current) => current + 1);
                          }}
                          className="h-4 w-4 accent-primary"
                        />
                        <span>
                          {value === "token"
                            ? t("share.saleMarketKind.token", {
                                defaultValue: "Token Market",
                              })
                            : t("share.saleMarketKind.share", {
                                defaultValue: "Share Market",
                              })}
                        </span>
                      </label>
                    ))}
                  </div>
                </div>

                {saleMarketKind === "token" ? (
                  <MarketSelectorField
                    markets={usageMarkets}
                    marketAccessMode={marketAccessMode}
                    selectedMarketEmails={normalizedSelectedMarketEmails}
                    marketSelectKey={marketSelectKey}
                    disabled={marketDisabled || busy}
                    marketsLoading={marketsLoading}
                    marketsError={marketsErrorMessage}
                    onRetryMarkets={() => void refetchMarkets()}
                    onMarketAccessModeChange={setMarketAccessMode}
                    onSelectedMarketEmailsChange={setSelectedMarketEmails}
                    onMarketSelectKeyChange={setMarketSelectKey}
                  />
                ) : (
                  <ShareMarketSelectorField
                    markets={shareMarkets}
                    selectedShareMarketEmail={normalizedSelectedShareMarketEmail}
                    disabled={marketDisabled || busy}
                    marketsLoading={marketsLoading}
                    marketsError={marketsErrorMessage}
                    onRetryMarkets={() => void refetchMarkets()}
                    onSelectedShareMarketEmailChange={setSelectedShareMarketEmail}
                    invalid={marketInvalid}
                  />
                )}

                <div className="space-y-2 md:col-span-2">
                  <Label>
                    {t("share.sharedWithEmails", { defaultValue: "Share To" })}
                  </Label>
                  <div className="text-xs text-muted-foreground">
                    {shareAppDisplayLabel(shareableApp)} ·{" "}
                    {t("share.createDialog.shareToHint", {
                      defaultValue: "可访问邮箱；留空则仅 owner 可见。",
                    })}
                  </div>
                  <EmailTagsInput
                    value={shareToEmails}
                    invalid={shareToInvalid}
                    disabled={busy}
                    onChange={setShareToEmails}
                    placeholder={t("share.sharedWithEmailsPlaceholder", {
                      defaultValue: "friend@example.com",
                    })}
                  />
                </div>

                <div className="space-y-2">
                  <Label htmlFor="provider-share-token-limit">
                    {t("share.tokenLimit", { defaultValue: "Token 限额" })}
                  </Label>
                  <Input
                    id="provider-share-token-limit"
                    type="number"
                    min={0}
                    disabled={busy}
                    placeholder={t("share.unlimited", { defaultValue: "无上限" })}
                    value={tokenLimitInput}
                    onChange={(event) => {
                      tokenLimitTouchedRef.current = true;
                      setTokenLimitInput(event.target.value);
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
                        }}
                      >
                        {preset.toLocaleString()}
                      </Button>
                    ))}
                  </div>
                </div>

                <div className="space-y-2">
                  <Label htmlFor="provider-share-parallel-limit">
                    {t("share.parallelLimit", { defaultValue: "并发限额" })}
                  </Label>
                  <Input
                    id="provider-share-parallel-limit"
                    type="number"
                    min={MIN_PARALLEL_LIMIT}
                    disabled={busy}
                    placeholder={t("share.unlimited", { defaultValue: "无上限" })}
                    value={parallelLimitInput}
                    onChange={(event) => {
                      parallelLimitTouchedRef.current = true;
                      setParallelLimitInput(event.target.value);
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
                      }}
                    >
                      {DEFAULT_PARALLEL_LIMIT}
                    </Button>
                  </div>
                </div>

                <div className="space-y-2 md:col-span-2">
                  <Label htmlFor="provider-share-expires">
                    {t("share.expiresIn", { defaultValue: "有效期（秒）" })}
                  </Label>
                  <Input
                    id="provider-share-expires"
                    type="number"
                    disabled={busy || isPermanent}
                    value={expiresInSecsInput}
                    onChange={(event) => {
                      expiresTouchedRef.current = true;
                      setExpiresInSecsInput(event.target.value);
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
                        const next = checked === true;
                        setIsPermanent(next);
                        if (next) {
                          setExpiresInSecsInput(String(permanentExpiresInSecs()));
                        } else {
                          setExpiresInSecsInput(String(24 * 3600));
                        }
                      }}
                    />
                    <Label
                      htmlFor="provider-share-expires-permanent"
                      className="cursor-pointer text-sm font-normal"
                    >
                      {t("share.expiry.permanent", { defaultValue: "永久有效" })}
                    </Label>
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

              <div className="flex flex-wrap gap-2">
                {!share ? (
                  <Button
                    type="button"
                    disabled={busy}
                    onClick={() => void handleCreate()}
                  >
                    <Play className="mr-2 h-4 w-4" />
                    {t("provider.share.createAndEnable", {
                      defaultValue: "创建并启用分享",
                    })}
                  </Button>
                ) : (
                  <>
                    <Button
                      type="button"
                      variant="outline"
                      disabled={busy}
                      onClick={() => void handleSave()}
                    >
                      {t("common.save", { defaultValue: "保存" })}
                    </Button>
                    {share.status === "active" ? (
                      <Button
                        type="button"
                        variant="outline"
                        disabled={busy}
                        onClick={() => void pauseMutation.mutateAsync(share.id)}
                      >
                        <Pause className="mr-2 h-4 w-4" />
                        {t("share.pause", { defaultValue: "暂停" })}
                      </Button>
                    ) : (
                      <Button
                        type="button"
                        variant="outline"
                        disabled={busy}
                        onClick={() => void resumeMutation.mutateAsync(share.id)}
                      >
                        <Play className="mr-2 h-4 w-4" />
                        {t("share.resume", { defaultValue: "恢复" })}
                      </Button>
                    )}
                    {share.tunnelUrl ? (
                      <Button
                        type="button"
                        variant="outline"
                        disabled={busy}
                        onClick={() => void disableMutation.mutateAsync(share.id)}
                      >
                        {t("share.disable", { defaultValue: "关闭隧道" })}
                      </Button>
                    ) : (
                      <Button
                        type="button"
                        variant="outline"
                        disabled={busy}
                        onClick={() => void enableMutation.mutateAsync(share.id)}
                      >
                        {t("share.enable", { defaultValue: "开启隧道" })}
                      </Button>
                    )}
                    <Button
                      type="button"
                      variant="ghost"
                      className="text-destructive hover:text-destructive"
                      disabled={busy}
                      onClick={() => void deleteMutation.mutateAsync(share.id)}
                    >
                      <Trash2 className="mr-2 h-4 w-4" />
                      {t("share.delete", { defaultValue: "删除分享" })}
                    </Button>
                  </>
                )}
              </div>
            </>
          )}
        </div>
      </div>

      <ConfirmDialog
        isOpen={confirmFreeOpen}
        title={t("share.forSaleOptions.free", { defaultValue: "免费" })}
        message={t("share.freeConfirm", {
          defaultValue: "确认将 For Sale 设为 Free？",
        })}
        confirmText={t("common.confirm", { defaultValue: "确认" })}
        cancelText={t("common.cancel", { defaultValue: "取消" })}
        variant="info"
        onConfirm={() => {
          setForSaleValue("Free");
          setConfirmFreeOpen(false);
        }}
        onCancel={() => setConfirmFreeOpen(false)}
      />
    </div>
  );
}

function MarketSelectorField({
  markets,
  marketAccessMode,
  selectedMarketEmails,
  marketSelectKey,
  disabled,
  marketsLoading,
  marketsError,
  onRetryMarkets,
  onMarketAccessModeChange,
  onSelectedMarketEmailsChange,
  onMarketSelectKeyChange,
}: {
  markets: PublicMarket[];
  marketAccessMode: "selected" | "all";
  selectedMarketEmails: string[];
  marketSelectKey: number;
  disabled: boolean;
  marketsLoading: boolean;
  marketsError?: string;
  onRetryMarkets: () => void;
  onMarketAccessModeChange: (mode: "selected" | "all") => void;
  onSelectedMarketEmailsChange: (emails: string[]) => void;
  onMarketSelectKeyChange: (updater: (current: number) => number) => void;
}) {
  const { t } = useTranslation();

  return (
    <div className="space-y-2 md:col-span-2">
      <Label>{t("share.market.title", { defaultValue: "Token Market" })}</Label>
      <div className="flex flex-wrap items-center gap-2">
        <Select
          key={marketSelectKey}
          value={marketAccessMode === "all" ? "__all__" : "__selected_market__"}
          disabled={disabled}
          onValueChange={(value) => {
            if (value === "__all__") {
              onMarketAccessModeChange("all");
              onSelectedMarketEmailsChange([]);
              onMarketSelectKeyChange((current) => current + 1);
              return;
            }
            if (value === "__selected_market__") {
              onMarketAccessModeChange("selected");
              return;
            }
            onMarketAccessModeChange("selected");
            onSelectedMarketEmailsChange(
              uniqueSortedEmails([...selectedMarketEmails, value.toLowerCase()]),
            );
            onMarketSelectKeyChange((current) => current + 1);
          }}
        >
          <SelectTrigger className="w-56">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="__all__">
              {t("share.market.all", { defaultValue: "全部" })}
            </SelectItem>
            <SelectItem value="__selected_market__" disabled>
              {t("share.market.select", { defaultValue: "选择 Market" })}
            </SelectItem>
            {markets.map((market) => (
              <SelectItem key={market.id} value={market.email}>
                {formatMarketSelectLabel(market)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {marketsLoading ? (
          <span className="text-xs text-muted-foreground">
            {t("common.loading", { defaultValue: "加载中" })}
          </span>
        ) : null}
        {marketsError ? (
          <button
            type="button"
            className="text-xs text-destructive underline"
            onClick={onRetryMarkets}
          >
            {marketsError}
          </button>
        ) : null}
      </div>
      {marketAccessMode === "all" ? (
        <Badge variant="secondary" className="w-fit text-xs">
          {t("share.market.allSelected", { defaultValue: "已选中所有 Market" })}
        </Badge>
      ) : (
        <MarketTags
          markets={markets}
          selectedMarketEmails={selectedMarketEmails}
          removable
          disabled={disabled}
          onRemove={(email) =>
            onSelectedMarketEmailsChange(
              selectedMarketEmails.filter((item) => item !== email),
            )
          }
        />
      )}
    </div>
  );
}

function ShareMarketSelectorField({
  markets,
  selectedShareMarketEmail,
  disabled,
  marketsLoading,
  marketsError,
  onRetryMarkets,
  onSelectedShareMarketEmailChange,
  invalid,
}: {
  markets: PublicMarket[];
  selectedShareMarketEmail: string;
  disabled: boolean;
  marketsLoading: boolean;
  marketsError?: string;
  onRetryMarkets: () => void;
  onSelectedShareMarketEmailChange: (email: string) => void;
  invalid: boolean;
}) {
  const { t } = useTranslation();

  return (
    <div className="space-y-2 md:col-span-2">
      <Label>
        {t("share.accountMarket.title", { defaultValue: "Share Market" })}
      </Label>
      <Select
        value={selectedShareMarketEmail || "__select_share_market__"}
        disabled={disabled || marketsLoading || markets.length === 0}
        onValueChange={(value) => {
          if (value !== "__select_share_market__") {
            onSelectedShareMarketEmailChange(value.toLowerCase());
          }
        }}
      >
        <SelectTrigger className="w-56">
          <SelectValue
            placeholder={t("share.accountMarket.select", {
              defaultValue: "选择 Share Market",
            })}
          />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="__select_share_market__" disabled>
            {t("share.accountMarket.select", {
              defaultValue: "选择 Share Market",
            })}
          </SelectItem>
          {markets.map((market) => (
            <SelectItem key={market.id} value={market.email}>
              {formatMarketSelectLabel(market)}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      {invalid ? (
        <p className="text-xs text-destructive">
          {t("share.accountMarket.required", {
            defaultValue: "请选择一个 Share Market",
          })}
        </p>
      ) : null}
      {marketsError ? (
        <button
          type="button"
          className="text-xs text-destructive underline"
          onClick={onRetryMarkets}
        >
          {marketsError}
        </button>
      ) : null}
    </div>
  );
}

function MarketTags({
  markets,
  selectedMarketEmails,
  removable = false,
  disabled = false,
  onRemove,
}: {
  markets: PublicMarket[];
  selectedMarketEmails: string[];
  removable?: boolean;
  disabled?: boolean;
  onRemove?: (email: string) => void;
}) {
  const marketByEmail = new Map(
    markets.map((market) => [market.email.toLowerCase(), market]),
  );

  if (selectedMarketEmails.length === 0) return null;

  return (
    <div className="flex flex-wrap gap-1.5">
      {selectedMarketEmails.map((email) => {
        const market = marketByEmail.get(email);
        return (
          <Badge
            key={email}
            variant="outline"
            className="flex items-center gap-1 text-xs"
          >
            <span>{market?.displayName ?? email}</span>
            {removable ? (
              <button
                type="button"
                className="rounded-full text-muted-foreground hover:text-foreground disabled:opacity-50"
                disabled={disabled}
                aria-label={`Remove ${market?.displayName ?? email}`}
                onClick={() => onRemove?.(email)}
              >
                <X className="h-3 w-3" />
              </button>
            ) : null}
          </Badge>
        );
      })}
    </div>
  );
}

export { getProviderShareState };
