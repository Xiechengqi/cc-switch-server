import { useEffect, useMemo, useRef, useState } from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { ChevronDown, ChevronRight, X } from "lucide-react";
import type {
  AppId,
  CreateShareParams,
  PublicMarket,
  ShareAccessByApp,
  ShareAppSettingsByApp,
  ShareBindings,
  TunnelConfig,
} from "@/lib/api";
import { SHARE_APP_TYPES } from "@/lib/api";
import {
  createShareSchema,
  type CreateShareFormInput,
  type CreateShareFormValues,
} from "@/lib/schemas/share";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogDescription,
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
import { Textarea } from "@/components/ui/textarea";
import { Checkbox } from "@/components/ui/checkbox";
import { EmailTagsInput } from "@/components/ui/tags-input";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import {
  DEFAULT_PARALLEL_LIMIT,
  MIN_PARALLEL_LIMIT,
  UNLIMITED_PARALLEL_LIMIT,
  UNLIMITED_TOKEN_LIMIT,
  isUnlimitedParallelLimit,
  isUnlimitedTokenLimit,
  permanentExpiresInSecs,
} from "@/utils/shareUtils";
import { normalizeShareRouterDomain } from "@/utils/shareRouter";
import { cn } from "@/lib/utils";
import {
  formatProviderOptionLabel,
  type ProviderOption,
} from "./providerOptions";
import { ShareRouterSelector } from "./ShareRouterSelector";

export type { ProviderOption } from "./providerOptions";

export interface CreateShareExtras {
  /**
   * 兼容老 API 字段：所有 per-app sharedWithEmails 取并集后回填这里。
   * 后端 UpdateShareAclParams 仍要求传入。新前端真正的数据源是 `accessByApp`。
   */
  sharedWithEmails: string[];
  marketAccessMode: "selected" | "all";
  saleMarketKind?: "token" | "share";
  /**
   * 按 app 区分的可访问邮箱。空对象 = 没有 per-app 自定义，仍走 `sharedWithEmails` 兼容路径。
   */
  accessByApp?: ShareAccessByApp;
  appSettings?: ShareAppSettingsByApp;
}

export interface BuildCreateShareAccessPayloadInput {
  forSale: "Yes" | "No" | "Free";
  saleMarketKind: "token" | "share";
  marketAccessMode: "selected" | "all";
  fixedBindings: Partial<Record<keyof ShareBindings, string>>;
  dynamicApps: Array<keyof ShareBindings>;
  shareToEmailsByApp: Record<keyof ShareBindings, string[]>;
  selectedTokenMarketEmails: string[];
  selectedShareMarketEmail: string;
  defaultShareApp: keyof ShareBindings;
  tokenLimit?: number;
  parallelLimit?: number;
  expiresInSecs?: number;
}

export function buildCreateShareAccessPayload({
  forSale,
  saleMarketKind,
  marketAccessMode,
  fixedBindings,
  dynamicApps,
  shareToEmailsByApp,
  selectedTokenMarketEmails,
  selectedShareMarketEmail,
  defaultShareApp,
  tokenLimit,
  parallelLimit,
  expiresInSecs,
}: BuildCreateShareAccessPayloadInput): CreateShareExtras {
  const usedApps = new Set<keyof ShareBindings>([
    ...(Object.keys(fixedBindings) as Array<keyof ShareBindings>),
    ...dynamicApps,
  ]);
  if (forSale === "Yes" && saleMarketKind === "share" && usedApps.size === 0) {
    usedApps.add(defaultShareApp);
  }

  const accessByApp: ShareAccessByApp = {};
  const appSettings: ShareAppSettingsByApp = {};
  for (const app of SHARE_APP_TYPES) {
    if (!usedApps.has(app)) continue;
    const marketEmails =
      forSale !== "Yes"
        ? []
        : saleMarketKind === "share"
          ? selectedShareMarketEmail
            ? [selectedShareMarketEmail]
            : []
          : marketAccessMode === "all"
            ? []
            : selectedTokenMarketEmails;
    const emails = uniqueSortedEmails([
      ...normalizeEmails(shareToEmailsByApp[app] ?? []),
      ...marketEmails,
    ]);
    accessByApp[app] = {
      sharedWithEmails: emails,
      marketAccessMode:
        forSale === "Yes" && saleMarketKind === "share"
          ? "selected"
          : marketAccessMode,
    };
    appSettings[app] = {
      forSale,
      saleMarketKind,
      marketAccessMode:
        forSale === "Yes" && saleMarketKind === "share"
          ? "selected"
          : marketAccessMode,
      sharedWithEmails: emails,
      tokenLimit: tokenLimit ?? UNLIMITED_TOKEN_LIMIT,
      parallelLimit: parallelLimit ?? UNLIMITED_PARALLEL_LIMIT,
      expiresAt:
        typeof expiresInSecs === "number"
          ? new Date(Date.now() + expiresInSecs * 1000).toISOString()
          : "",
    };
  }

  return {
    sharedWithEmails: uniqueSortedEmails(
      Object.values(accessByApp).flatMap(
        (entry) => entry?.sharedWithEmails ?? [],
      ),
    ),
    marketAccessMode:
      forSale === "Yes" && saleMarketKind === "share"
        ? "selected"
        : marketAccessMode,
    accessByApp: Object.keys(accessByApp).length > 0 ? accessByApp : undefined,
    appSettings: Object.keys(appSettings).length > 0 ? appSettings : undefined,
    saleMarketKind,
  };
}

/**
 * 表单里 Provider 选择器展示的最小 provider 形态。调用方按 `appType` 过滤后传入。
 *
 * Why: share ↔ provider 严格 1:1。一个 provider 同时只能被一个非 deleted share
 * 绑定，所以选择器要把已被其他 share 绑定的 provider 标灰禁选。
 */
interface CreateShareDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** 触发对话框时所在的 app tab。用于预填该 app slot 的默认 provider 选择。 */
  defaultApp?: AppId;
  ownerEmail?: string | null;
  tunnelConfig: TunnelConfig;
  tunnelConfigSaving: boolean;
  isSubmitting: boolean;
  submitLabel?: string;
  markets?: PublicMarket[];
  marketsLoading?: boolean;
  marketsError?: string | null;
  /** P8 多 app share：每个 app_type 各自一组候选（已过滤掉被别的 share 绑定的）。 */
  providersByApp: Record<keyof ShareBindings, ProviderOption[]>;
  onRetryMarkets?: () => void;
  onSaveTunnelConfig: (config: TunnelConfig) => Promise<void> | void;
  onSubmit: (
    params: CreateShareParams,
    extras: CreateShareExtras,
  ) => Promise<void> | void;
}

const EXPIRY_PRESETS = [
  { labelKey: "share.expiry.oneHour", value: 3600 },
  { labelKey: "share.expiry.sixHours", value: 6 * 3600 },
  { labelKey: "share.expiry.oneDay", value: 24 * 3600 },
  { labelKey: "share.expiry.sevenDays", value: 7 * 24 * 3600 },
  { labelKey: "share.expiry.thirtyDays", value: 30 * 24 * 3600 },
];

const TOKEN_PRESETS = [10000, 50000, 100000, 500000];
const DEFAULT_TOKEN_LIMIT_FALLBACK = 100000;
const SUBDOMAIN_PREFIX_LENGTH = 5;
const SUBDOMAIN_TIMESTAMP_LENGTH = 5;
const EMPTY_MARKETS: PublicMarket[] = [];
/**
 * P17 动态绑定的表单 sentinel：当用户在 Provider Select 里选了"动态绑定当前选中的
 * provider"，bindings.<app> 写入这个值；performSubmit 时把它折叠成后端的
 * `dynamicApps: [app]` 字段，bindings 字段里则不再带这个 app 的条目。
 *
 * 用 "__" 前缀是为了和真实 provider id（UUID / 用户自定义短名）保留出区分空间。
 */
export const DYNAMIC_BINDING_VALUE = "__dynamic__";

/**
 * 由 owner email 派生默认 subdomain。
 *
 * 形态：`{email-prefix}-{base36-timestamp-suffix}` 例如 `alice-2lr8q`。
 *
 * 单 share 模式时这个函数只取邮箱前缀（同一 owner 创建多个 share 时会撞），
 * 多 share 改造后追加毫秒时间戳的 base36 末 5 位作为去重后缀：
 *  - 同设备连续创建：Date.now() 必然递增，不会撞。
 *  - 跨设备同毫秒并发：前缀通常不同（不同 owner email）。
 *  - email 完全没有可用字母时（如 `123@x.com`）回退到 `s` 占位前缀。
 */
export function deriveSubdomainFromEmail(
  email: string | null | undefined,
): string {
  const local = (email ?? "").split("@")[0] ?? "";
  const filtered = local.toLowerCase().replace(/[^a-z]/g, "");
  const prefix =
    filtered.length === 0 ? "s" : filtered.slice(0, SUBDOMAIN_PREFIX_LENGTH);
  const suffix = Date.now().toString(36).slice(-SUBDOMAIN_TIMESTAMP_LENGTH);
  return `${prefix}-${suffix}`;
}

function buildDefaultValues(
  ownerEmail: string,
  _defaultApp: AppId | undefined,
  _providersByApp: Record<keyof ShareBindings, ProviderOption[]> | undefined,
): CreateShareFormInput {
  // P17：创建对话框默认不预填 provider；用户必须在高级设置里显式选择
  // 固定 provider 或"动态绑定当前选中的 provider"。所有 slot 初始为空。
  const initialBindings: { claude: string; codex: string; gemini: string } = {
    claude: "",
    codex: "",
    gemini: "",
  };
  return {
    bindings: initialBindings,
    description: "",
    forSale: "Yes",
    saleMarketKind: "token",
    tokenLimit: UNLIMITED_TOKEN_LIMIT,
    parallelLimit: UNLIMITED_PARALLEL_LIMIT,
    expiresInSecs: permanentExpiresInSecs(),
    subdomain: deriveSubdomainFromEmail(ownerEmail),
    marketAccessMode: "all",
  };
}

export function CreateShareDialog({
  open,
  onOpenChange,
  defaultApp,
  ownerEmail,
  tunnelConfig,
  tunnelConfigSaving,
  isSubmitting,
  submitLabel,
  markets = EMPTY_MARKETS,
  marketsLoading = false,
  marketsError = null,
  providersByApp,
  onRetryMarkets,
  onSaveTunnelConfig,
  onSubmit,
}: CreateShareDialogProps) {
  const { t } = useTranslation();
  const [confirmFreeOpen, setConfirmFreeOpen] = useState(false);
  const [defaultsConfirmOpen, setDefaultsConfirmOpen] = useState(false);
  const [isPermanent, setIsPermanent] = useState(true);
  const [ownerEmailInput, setOwnerEmailInput] = useState(ownerEmail ?? "");
  const [routerDomain, setRouterDomain] = useState(tunnelConfig.domain);
  const [routerDomainError, setRouterDomainError] = useState<string | null>(
    null,
  );
  const [advancedExpanded, setAdvancedExpanded] = useState(false);
  const [advancedOpened, setAdvancedOpened] = useState(false);
  const [lastFiniteTokenLimit, setLastFiniteTokenLimit] = useState(
    DEFAULT_TOKEN_LIMIT_FALLBACK,
  );
  const [lastFiniteParallelLimit, setLastFiniteParallelLimit] = useState(
    DEFAULT_PARALLEL_LIMIT,
  );
  const usageMarkets = useMemo(
    () =>
      markets.filter((market) => (market.marketKind ?? "usage") !== "share"),
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
  const [selectedMarketEmails, setSelectedMarketEmails] = useState<string[]>(
    [],
  );
  const [selectedShareMarketEmail, setSelectedShareMarketEmail] = useState("");
  const [marketSelectKey, setMarketSelectKey] = useState(0);
  const wasOpenRef = useRef(false);
  // 按 app 区分的「Share To」邮箱列表。与 EditShareDialog 行为对齐：用本地 state
  // 而不是 react-hook-form schema 字段，避免每个字段都要走 zod 校验。submit 时
  // 才汇总成 ShareAccessByApp。
  const emptyShareToByApp = (): Record<keyof ShareBindings, string[]> => ({
    claude: [],
    codex: [],
    gemini: [],
  });
  const [shareToEmailsByApp, setShareToEmailsByApp] =
    useState<Record<keyof ShareBindings, string[]>>(emptyShareToByApp);
  const [activeSettingsApp, setActiveSettingsApp] =
    useState<keyof ShareBindings>("claude");
  const subdomainManualRef = useRef(false);

  const form = useForm<CreateShareFormInput, unknown, CreateShareFormValues>({
    resolver: zodResolver(createShareSchema),
    defaultValues: buildDefaultValues(
      ownerEmail ?? "",
      defaultApp,
      providersByApp,
    ),
  });

  useEffect(() => {
    if (!open) {
      wasOpenRef.current = false;
      return;
    }
    if (wasOpenRef.current) return;
    wasOpenRef.current = true;
    setOwnerEmailInput(ownerEmail ?? "");
    setRouterDomain(tunnelConfig.domain);
    setRouterDomainError(null);
    form.reset(
      buildDefaultValues(ownerEmail ?? "", defaultApp, providersByApp),
    );
    setIsPermanent(true);
    setLastFiniteTokenLimit(DEFAULT_TOKEN_LIMIT_FALLBACK);
    setLastFiniteParallelLimit(DEFAULT_PARALLEL_LIMIT);
    setAdvancedExpanded(false);
    setAdvancedOpened(false);
    setDefaultsConfirmOpen(false);
    setShareToEmailsByApp(emptyShareToByApp());
    setSelectedMarketEmails([]);
    setSelectedShareMarketEmail("");
    setMarketSelectKey((current) => current + 1);
    subdomainManualRef.current = false;
  }, [form, open, ownerEmail, tunnelConfig.domain, defaultApp]);

  useEffect(() => {
    if (!open || subdomainManualRef.current) return;
    const derived = deriveSubdomainFromEmail(ownerEmailInput);
    form.setValue("subdomain", derived, { shouldValidate: false });
  }, [form, open, ownerEmailInput]);

  const tokenLimit = form.watch("tokenLimit") as number;
  const parallelLimit = form.watch("parallelLimit") as number;
  const marketAccessMode = form.watch("marketAccessMode") as "selected" | "all";
  const saleMarketKind = form.watch("saleMarketKind") as "token" | "share";
  const subdomainValue = form.watch("subdomain") as string;
  const forSaleValue = form.watch("forSale") as "Yes" | "No" | "Free";
  const descriptionValue = form.watch("description") as string;
  const expiresInSecsValue = form.watch("expiresInSecs") as number;
  const unlimitedTokenLimit = isUnlimitedTokenLimit(tokenLimit);
  const unlimitedParallelLimit = isUnlimitedParallelLimit(parallelLimit);
  const tokenLimitField = form.register("tokenLimit", { valueAsNumber: true });
  const parallelLimitField = form.register("parallelLimit", {
    valueAsNumber: true,
  });
  const subdomainField = form.register("subdomain");
  const normalizedOwnerEmail = ownerEmailInput.trim().toLowerCase();
  const ownerEmailInvalid =
    !normalizedOwnerEmail ||
    !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(normalizedOwnerEmail);

  // Share To 校验：任一 app 列表里若有不合法 email，整体置 invalid，阻断 submit。
  const shareToInvalid = useMemo(
    () =>
      Object.values(shareToEmailsByApp).some((emails) =>
        emails.some((email) => email && !isValidEmail(email)),
      ),
    [shareToEmailsByApp],
  );

  // 当前 advanced 里已经选了 provider 的 app 列表——只为这些 app 渲染 Share To 输入框，
  // 与 EditShareDialog 的 shareAccessApps 行为对齐：未绑定的 app 不参与共享 ACL。
  // 用 watch 让 bindings 变化时实时响应。
  const watchedBindings = form.watch("bindings") as
    | Record<keyof ShareBindings, string>
    | undefined;
  const selectedFixedProviderIds = useMemo(() => {
    const ids = new Set<string>();
    for (const app of SHARE_APP_TYPES) {
      const providerId = watchedBindings?.[app]?.trim() ?? "";
      if (providerId && providerId !== DYNAMIC_BINDING_VALUE) {
        ids.add(providerId);
      }
    }
    return ids;
  }, [watchedBindings]);
  useEffect(() => {
    if (!open || forSaleValue !== "Yes" || saleMarketKind !== "share") return;
    const currentEmail = selectedShareMarketEmail.trim().toLowerCase();
    if (currentEmail && shareMarketEmailSet.has(currentEmail)) return;
    const firstShareMarket = shareMarkets[0]?.email?.trim().toLowerCase();
    if (firstShareMarket) {
      setSelectedShareMarketEmail(firstShareMarket);
    }
  }, [
    forSaleValue,
    open,
    saleMarketKind,
    selectedShareMarketEmail,
    shareMarketEmailSet,
    shareMarkets,
  ]);

  const normalizedSelectedMarketEmails = useMemo(
    () =>
      uniqueSortedEmails(
        selectedMarketEmails
          .map((email) => email.trim().toLowerCase())
          .filter((email) => usageMarketEmailSet.has(email)),
      ),
    [selectedMarketEmails, usageMarketEmailSet],
  );
  const normalizedSelectedShareMarketEmail = shareMarketEmailSet.has(
    selectedShareMarketEmail.trim().toLowerCase(),
  )
    ? selectedShareMarketEmail.trim().toLowerCase()
    : forSaleValue === "Yes" && saleMarketKind === "share"
      ? (shareMarkets[0]?.email?.trim().toLowerCase() ?? "")
      : "";
  const marketDisabled = forSaleValue !== "Yes";
  const marketInvalid =
    forSaleValue === "Yes" &&
    saleMarketKind === "share" &&
    normalizedSelectedShareMarketEmail.length === 0;

  const defaultShareApp = toShareAppType(defaultApp);
  const defaultProviderId =
    (form.watch(`bindings.${defaultShareApp}` as const) as
      | string
      | undefined) ?? "";
  const defaultProvider = (providersByApp?.[defaultShareApp] ?? []).find(
    (provider) => provider.id === defaultProviderId,
  );
  const defaultProviderLabel =
    defaultProviderId === DYNAMIC_BINDING_VALUE
      ? t("share.providerBindingDynamic", {
          defaultValue: "动态绑定当前选中的 provider",
        })
      : defaultProvider?.name ||
        defaultProviderId ||
        t("share.unbound", { defaultValue: "未绑定" });

  const expandAdvanced = () => {
    setAdvancedExpanded(true);
    setAdvancedOpened(true);
  };

  const performSubmit = form.handleSubmit(async (values) => {
    if (ownerEmailInvalid) {
      return;
    }
    if (shareToInvalid) {
      return;
    }
    if (marketInvalid) {
      return;
    }
    let nextRouterDomain: string;
    try {
      nextRouterDomain = normalizeShareRouterDomain(routerDomain);
      setRouterDomainError(null);
    } catch (error) {
      const key =
        error instanceof Error
          ? error.message
          : "share.validation.invalidRouterDomain";
      setRouterDomainError(
        t(key, {
          defaultValue: "Router domain is invalid",
        }),
      );
      return;
    }
    if (nextRouterDomain && nextRouterDomain !== tunnelConfig.domain) {
      await onSaveTunnelConfig({ domain: nextRouterDomain });
    }
    // P17：拆分两类 slot：
    //   - DYNAMIC_BINDING_VALUE 折叠成 dynamicApps，后端解析当前激活 provider。
    //   - 非空 / 非 sentinel 走 bindings，作为固定绑定。
    //   - 空字符串保持原义：用户未选，不传后端。
    const allEntries = Object.entries(values.bindings ?? {}) as Array<
      [keyof ShareBindings, string]
    >;
    const fixedBindings = Object.fromEntries(
      allEntries.filter(
        ([, pid]) => pid && pid.length > 0 && pid !== DYNAMIC_BINDING_VALUE,
      ),
    );
    const dynamicApps = allEntries
      .filter(([, pid]) => pid === DYNAMIC_BINDING_VALUE)
      .map(([app]) => app as string);

    const accessPayload = buildCreateShareAccessPayload({
      forSale: forSaleValue,
      saleMarketKind,
      marketAccessMode: values.marketAccessMode,
      fixedBindings,
      dynamicApps: dynamicApps as Array<keyof ShareBindings>,
      shareToEmailsByApp,
      selectedTokenMarketEmails: normalizedSelectedMarketEmails,
      selectedShareMarketEmail: normalizedSelectedShareMarketEmail,
      defaultShareApp,
      tokenLimit: values.tokenLimit,
      parallelLimit: values.parallelLimit,
      expiresInSecs: values.expiresInSecs,
    });

    await onSubmit(
      {
        ownerEmail: normalizedOwnerEmail,
        bindings: fixedBindings,
        dynamicApps: dynamicApps.length > 0 ? dynamicApps : undefined,
        description: values.description || undefined,
        forSale: values.forSale,
        saleMarketKind: values.saleMarketKind,
        tokenLimit: values.tokenLimit,
        parallelLimit: values.parallelLimit,
        expiresInSecs: values.expiresInSecs,
        subdomain: values.subdomain || undefined,
      },
      accessPayload,
    );
  });

  const handleCreateClick = () => {
    if (ownerEmailInvalid || isSubmitting || tunnelConfigSaving) return;
    if (advancedOpened) {
      void performSubmit();
      return;
    }
    setDefaultsConfirmOpen(true);
  };

  const handleDefaultsConfirmAccept = () => {
    setDefaultsConfirmOpen(false);
    void performSubmit();
  };

  const summary = useMemo(
    () =>
      buildDefaultsSummary(t, {
        forSale: forSaleValue,
        saleMarketKind,
        marketAccessMode,
        expiresInSecs: expiresInSecsValue,
        isPermanent,
        tokenLimit,
        parallelLimit,
        subdomain: subdomainValue,
        providerBinding: `${defaultShareApp} · ${defaultProviderLabel}`,
      }),
    [
      t,
      forSaleValue,
      saleMarketKind,
      marketAccessMode,
      expiresInSecsValue,
      isPermanent,
      tokenLimit,
      parallelLimit,
      subdomainValue,
      defaultShareApp,
      defaultProviderLabel,
    ],
  );

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl overflow-hidden p-0">
        <DialogHeader className="px-5 pb-2 pt-5">
          <DialogTitle className="flex items-center gap-2">
            {t("share.create")}
            {/*
              多 app share 模式：badge 显示当前进入的 app tab 作为"默认聚焦的 slot"，
              用户可在表单里继续勾选其它 slot。
            */}
            <Badge variant="outline" className="capitalize">
              {toShareAppType(defaultApp)}
            </Badge>
          </DialogTitle>
          <DialogDescription className="text-xs">
            {t("share.createDescription")}
          </DialogDescription>
        </DialogHeader>

        <div className="flex-1 space-y-4 overflow-y-auto px-5 py-3">
          <div className="space-y-1.5">
            <Label htmlFor="share-create-router">
              {t("share.tunnel.region")}
            </Label>
            <ShareRouterSelector
              value={routerDomain}
              onChange={(value) => {
                setRouterDomain(value);
                setRouterDomainError(null);
              }}
              selectId="share-create-router"
              customInputId="share-create-router-custom"
              disabled={tunnelConfigSaving}
              error={routerDomainError}
            />
            <div className="text-xs text-muted-foreground">
              {t("share.createRouterHint", {
                defaultValue:
                  "创建前选择路由节点。创建完成后当前 share 会绑定到该节点。",
              })}
            </div>
          </div>

          {/* P17：删除"Provider 绑定"卡片——默认不预填，用户在高级设置里
              显式选择固定 provider 或"动态绑定当前选中的 provider"。表单级
              的 bindings 校验错误仍在这里渲染，避免高级设置未展开时静默失败。 */}
          {form.formState.errors.bindings ? (
            <div className="text-xs text-destructive">
              {(() => {
                const messageKey =
                  (form.formState.errors.bindings as { message?: string })
                    ?.message ?? "share.validation.providerRequired";
                return t(messageKey, {
                  defaultValue:
                    messageKey === "share.validation.providerDuplicate"
                      ? "同一个固定 Provider 只能绑定一个 share 分支"
                      : "至少为一个 app 选择 provider",
                });
              })()}
            </div>
          ) : null}

          <div className="grid gap-3 md:grid-cols-2">
            <div className="space-y-1.5">
              <Label htmlFor="share-owner-email">
                {t("share.ownerEmail", { defaultValue: "Owner Email" })}
              </Label>
              <Input
                id="share-owner-email"
                type="email"
                value={ownerEmailInput}
                onChange={(event) => setOwnerEmailInput(event.target.value)}
                placeholder="owner@example.com"
              />
              <div className="text-xs text-muted-foreground">
                {t("share.ownerEmailCreateHint", {
                  defaultValue:
                    "该邮箱会作为 share owner 上报到 router。router 页面使用相同邮箱登录后可查看 API Key 和编辑设置。",
                })}
              </div>
              <FieldError
                error={
                  ownerEmailInput.trim() && ownerEmailInvalid
                    ? t("share.validation.invalidEmail", {
                        defaultValue: "邮箱格式无效",
                      })
                    : undefined
                }
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="share-subdomain">{t("share.subdomain")}</Label>
              <Input
                id="share-subdomain"
                placeholder="my-share"
                {...subdomainField}
                onChange={(event) => {
                  subdomainField.onChange(event);
                  subdomainManualRef.current = true;
                }}
              />
              <div className="text-xs text-muted-foreground">
                {t("share.subdomainHint")}
              </div>
              <FieldError error={form.formState.errors.subdomain?.message} />
            </div>
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="share-description">{t("share.description")}</Label>
            <Textarea
              id="share-description"
              className="min-h-[72px]"
              maxLength={200}
              placeholder={t("share.descriptionPlaceholder")}
              {...form.register("description")}
            />
            <div className="text-xs text-muted-foreground">
              {t("share.descriptionHint")}
            </div>
            <FieldError error={form.formState.errors.description?.message} />
          </div>

          <div className="rounded-lg border border-border-default bg-muted/10">
            <button
              type="button"
              className="flex w-full items-center justify-between gap-2 px-3 py-2 text-sm font-medium"
              onClick={() =>
                advancedExpanded ? setAdvancedExpanded(false) : expandAdvanced()
              }
              aria-expanded={advancedExpanded}
              aria-controls="share-create-advanced"
            >
              <span className="flex items-center gap-2">
                {advancedExpanded ? (
                  <ChevronDown className="h-4 w-4" />
                ) : (
                  <ChevronRight className="h-4 w-4" />
                )}
                {t("share.createDialog.advancedToggle", {
                  defaultValue: "高级设置",
                })}
              </span>
              <span className="text-xs text-muted-foreground">
                {t("share.createDialog.advancedHint", {
                  defaultValue: "未展开则使用默认值，点击创建会弹出二次确认",
                })}
              </span>
            </button>
            {advancedExpanded ? (
              <div
                id="share-create-advanced"
                className="grid gap-3 border-t border-border-default px-3 py-3 md:grid-cols-2"
              >
                <div className="md:col-span-2">
                  <div className="inline-flex rounded-lg border bg-muted/40 p-0.5">
                    {SHARE_APP_TYPES.map((app) => (
                      <button
                        key={app}
                        type="button"
                        onClick={() => setActiveSettingsApp(app)}
                        className={cn(
                          "rounded-md px-3 py-1 text-xs font-medium transition-colors",
                          activeSettingsApp === app
                            ? "bg-background text-foreground shadow-sm"
                            : "text-muted-foreground hover:text-foreground",
                        )}
                      >
                        {shareAppDisplayLabel(app)}
                      </button>
                    ))}
                  </div>
                </div>

                <div className="space-y-2 md:col-span-2">
                  <div>
                    <Label>
                      {t("share.providerBindingsAdvanced", {
                        defaultValue: "按 App 独立绑定 Provider",
                      })}
                    </Label>
                    <div className="mt-1 text-xs text-muted-foreground">
                      {t("share.providerBindingsAdvancedHint", {
                        defaultValue:
                          "每个 App 最多绑定一个 Provider；留空表示该 App 在本 share 上不可用。",
                      })}
                    </div>
                  </div>
                  <div className="grid gap-2">
                    {[activeSettingsApp].map((app) => {
                      const candidates = providersByApp?.[app] ?? [];
                      const fieldKey = `bindings.${app}` as const;
                      const value =
                        (form.watch(fieldKey) as string | undefined) ?? "";
                      const isDynamic = value === DYNAMIC_BINDING_VALUE;
                      const selectedProvider = candidates.find(
                        (provider) => provider.id === value,
                      );
                      const selectedInOtherSlot = (providerId: string) =>
                        selectedFixedProviderIds.has(providerId) &&
                        value !== providerId;
                      return (
                        <div
                          key={app}
                          className="grid gap-1 rounded-md border border-default/50 p-2"
                        >
                          <div className="flex items-center justify-between text-xs font-medium uppercase text-muted-foreground">
                            <span>{app}</span>
                            {isDynamic ? (
                              <Badge variant="outline" className="text-[10px]">
                                {t("share.bindingDynamic", {
                                  defaultValue: "动态",
                                })}
                              </Badge>
                            ) : value ? (
                              <Badge variant="outline" className="text-[10px]">
                                {t("share.bound", { defaultValue: "已绑定" })}
                              </Badge>
                            ) : (
                              <Badge
                                variant="outline"
                                className="text-[10px] text-muted-foreground"
                              >
                                {t("share.unbound", { defaultValue: "未绑定" })}
                              </Badge>
                            )}
                          </div>
                          <div className="flex items-center gap-2">
                            <Select
                              value={value || ""}
                              onValueChange={(next) =>
                                form.setValue(fieldKey, next, {
                                  shouldValidate: true,
                                  shouldDirty: true,
                                })
                              }
                            >
                              <SelectTrigger
                                id={`share-create-provider-${app}`}
                                className="flex-1"
                              >
                                <SelectValue
                                  placeholder={t(
                                    "share.providerBindingPlaceholder",
                                    {
                                      defaultValue: `为 ${app} 选一个 provider`,
                                    },
                                  )}
                                >
                                  {isDynamic
                                    ? t("share.providerBindingDynamic", {
                                        defaultValue:
                                          "动态绑定当前选中的 provider",
                                      })
                                    : selectedProvider?.name}
                                </SelectValue>
                              </SelectTrigger>
                              <SelectContent>
                                <SelectItem value={DYNAMIC_BINDING_VALUE}>
                                  {t("share.providerBindingDynamic", {
                                    defaultValue: "动态绑定当前选中的 provider",
                                  })}
                                </SelectItem>
                                {candidates.length === 0 ? (
                                  <SelectItem value="__empty__" disabled>
                                    {t("share.providerBindingEmpty", {
                                      defaultValue: `{{app}} 下没有可绑定的 provider`,
                                      app,
                                    })}
                                  </SelectItem>
                                ) : (
                                  candidates.map((provider) => {
                                    const duplicateInForm = selectedInOtherSlot(
                                      provider.id,
                                    );
                                    const disabled =
                                      provider.disabled || duplicateInForm;
                                    return (
                                      <SelectItem
                                        key={provider.id}
                                        value={provider.id}
                                        disabled={disabled}
                                      >
                                        {formatProviderOptionLabel(
                                          { ...provider, disabled },
                                          duplicateInForm
                                            ? t(
                                                "share.providerBindingSelected",
                                                {
                                                  defaultValue:
                                                    "已在本 share 其他分支选择",
                                                },
                                              )
                                            : t("share.providerBindingTaken", {
                                                defaultValue:
                                                  "已被其他 share 绑定",
                                              }),
                                        )}
                                      </SelectItem>
                                    );
                                  })
                                )}
                              </SelectContent>
                            </Select>
                            {value ? (
                              <Button
                                type="button"
                                variant="ghost"
                                size="sm"
                                onClick={() =>
                                  form.setValue(fieldKey, "", {
                                    shouldValidate: true,
                                    shouldDirty: true,
                                  })
                                }
                                title={t("share.providerBindingClear", {
                                  defaultValue: "清空（解绑）",
                                })}
                              >
                                <X className="h-4 w-4" />
                              </Button>
                            ) : null}
                          </div>
                        </div>
                      );
                    })}
                  </div>
                </div>

                <div className="space-y-1.5">
                  <Label htmlFor="share-for-sale">{t("share.forSale")}</Label>
                  <Select
                    value={forSaleValue}
                    onValueChange={(value) => {
                      const next = value as "Yes" | "No" | "Free";
                      if (next === "Free") {
                        setConfirmFreeOpen(true);
                      } else {
                        form.setValue("forSale", next, {
                          shouldDirty: true,
                          shouldValidate: true,
                        });
                      }
                    }}
                  >
                    <SelectTrigger id="share-for-sale">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="No">
                        {t("share.forSaleOptions.no")}
                      </SelectItem>
                      <SelectItem value="Yes">
                        {t("share.forSaleOptions.yes")}
                      </SelectItem>
                      <SelectItem value="Free">
                        {t("share.forSaleOptions.free")}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                  <div className="text-xs text-muted-foreground">
                    {t("share.forSaleHint")}
                  </div>
                  <FieldError error={form.formState.errors.forSale?.message} />
                </div>

                <div className="space-y-1.5 md:col-span-2">
                  <Label>
                    {t("share.saleMarketKind.title", {
                      defaultValue: "Market Type",
                    })}
                  </Label>
                  <div className="text-xs text-muted-foreground">
                    {marketDisabled
                      ? t("share.market.forSaleRequired", {
                          defaultValue:
                            "Set ForSale to Yes before choosing a market.",
                        })
                      : t("share.saleMarketKind.description", {
                          defaultValue:
                            "Choose Token Market for token usage sale or Share Market for account rental.",
                        })}
                  </div>
                  <div className="flex flex-wrap items-center gap-5 pt-1">
                    {(["token", "share"] as const).map((value) => {
                      const id = `share-create-sale-market-kind-${value}`;
                      return (
                        <label
                          key={value}
                          htmlFor={id}
                          className={cn(
                            "flex cursor-pointer items-center gap-2 text-sm",
                            marketDisabled && "cursor-not-allowed opacity-60",
                          )}
                        >
                          <input
                            id={id}
                            type="radio"
                            name="share-create-sale-market-kind"
                            value={value}
                            checked={saleMarketKind === value}
                            disabled={marketDisabled}
                            onChange={() => {
                              form.setValue("saleMarketKind", value, {
                                shouldDirty: true,
                                shouldValidate: true,
                              });
                              if (value === "token") {
                                form.setValue("marketAccessMode", "all", {
                                  shouldDirty: true,
                                  shouldValidate: true,
                                });
                                setSelectedMarketEmails([]);
                                setSelectedShareMarketEmail("");
                              } else {
                                form.setValue("marketAccessMode", "selected", {
                                  shouldDirty: true,
                                  shouldValidate: true,
                                });
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
                      );
                    })}
                  </div>
                </div>

                {saleMarketKind === "token" ? (
                  <div className="space-y-1.5 md:col-span-2">
                    <Label>
                      {t("share.market.title", {
                        defaultValue: "Token Market",
                      })}
                    </Label>
                    <div className="text-xs text-muted-foreground">
                      {marketDisabled
                        ? t("share.market.forSaleRequired", {
                            defaultValue:
                              "Set ForSale to Yes before choosing a market.",
                          })
                        : t("share.market.description", {
                            defaultValue:
                              "Choose all or selected token markets.",
                          })}
                    </div>
                    <div className="flex flex-wrap items-center gap-2">
                      <Select
                        key={marketSelectKey}
                        value={
                          marketAccessMode === "all"
                            ? "__all__"
                            : "__selected_market__"
                        }
                        disabled={marketDisabled}
                        onValueChange={(value) => {
                          if (value === "__all__") {
                            form.setValue("marketAccessMode", "all", {
                              shouldDirty: true,
                              shouldValidate: true,
                            });
                            setSelectedMarketEmails([]);
                            setMarketSelectKey((current) => current + 1);
                            return;
                          }
                          form.setValue("marketAccessMode", "selected", {
                            shouldDirty: true,
                            shouldValidate: true,
                          });
                          setSelectedMarketEmails((current) =>
                            uniqueSortedEmails([
                              ...current,
                              value.toLowerCase(),
                            ]),
                          );
                          setMarketSelectKey((current) => current + 1);
                        }}
                      >
                        <SelectTrigger
                          className="w-56"
                          aria-label={t("share.market.select", {
                            defaultValue: "Select market",
                          })}
                        >
                          <SelectValue
                            placeholder={t("share.market.select", {
                              defaultValue: "Select market",
                            })}
                          />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="__all__">
                            {t("share.market.all", { defaultValue: "全部" })}
                          </SelectItem>
                          <SelectItem value="__selected_market__" disabled>
                            {t("share.market.select", {
                              defaultValue: "Select market",
                            })}
                          </SelectItem>
                          {usageMarkets.map((market) => (
                            <SelectItem key={market.id} value={market.email}>
                              {formatMarketSelectLabel(market)}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                      <Button
                        type="button"
                        variant="outline"
                        disabled={
                          marketDisabled ||
                          (marketAccessMode === "selected" &&
                            normalizedSelectedMarketEmails.length === 0)
                        }
                        onClick={() => {
                          form.setValue("marketAccessMode", "selected", {
                            shouldDirty: true,
                            shouldValidate: true,
                          });
                          setSelectedMarketEmails([]);
                        }}
                      >
                        {t("share.market.restore", {
                          defaultValue: "还原",
                        })}
                      </Button>
                    </div>
                    <MarketTags
                      markets={usageMarkets}
                      marketAccessMode={marketAccessMode}
                      selectedMarketEmails={normalizedSelectedMarketEmails}
                      removable
                      disabled={marketDisabled}
                      onRemove={(email) =>
                        setSelectedMarketEmails((current) =>
                          current.filter((item) => item !== email),
                        )
                      }
                    />
                    {marketAccessMode !== "all" &&
                    normalizedSelectedMarketEmails.length === 0 ? (
                      <div className="text-xs text-muted-foreground">
                        {t("share.market.default", {
                          defaultValue: "默认，不授权 Market",
                        })}
                      </div>
                    ) : null}
                    {marketsLoading && usageMarkets.length === 0 ? (
                      <div className="text-xs text-muted-foreground">
                        {t("common.loading", { defaultValue: "Loading" })}
                      </div>
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
                    {!marketsLoading &&
                    !marketsError &&
                    usageMarkets.length === 0 ? (
                      <div className="text-xs text-muted-foreground">
                        {t("share.market.empty", {
                          defaultValue: "暂无可用的 token market",
                        })}
                      </div>
                    ) : null}
                  </div>
                ) : null}

                {saleMarketKind === "share" ? (
                  <div className="space-y-1.5 md:col-span-2">
                    <Label>
                      {t("share.accountMarket.title", {
                        defaultValue: "Share Market",
                      })}
                    </Label>
                    <div className="text-xs text-muted-foreground">
                      {marketDisabled
                        ? t("share.accountMarket.forSaleRequired", {
                            defaultValue:
                              "Set ForSale to Yes before delegating an account market.",
                          })
                        : t("share.accountMarket.description", {
                            defaultValue:
                              "Choose one share market for account-hosted sale.",
                          })}
                    </div>
                    <Select
                      value={
                        selectedShareMarketEmail || "__select_share_market__"
                      }
                      disabled={
                        marketDisabled ||
                        marketsLoading ||
                        shareMarkets.length === 0
                      }
                      onValueChange={(value) =>
                        value === "__select_share_market__"
                          ? undefined
                          : setSelectedShareMarketEmail(value.toLowerCase())
                      }
                    >
                      <SelectTrigger
                        className="w-56"
                        aria-label={t("share.accountMarket.select", {
                          defaultValue: "Select share market",
                        })}
                      >
                        <SelectValue
                          placeholder={t("share.accountMarket.select", {
                            defaultValue: "Select share market",
                          })}
                        />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="__select_share_market__" disabled>
                          {t("share.accountMarket.select", {
                            defaultValue: "Select share market",
                          })}
                        </SelectItem>
                        {shareMarkets.map((market) => (
                          <SelectItem key={market.id} value={market.email}>
                            {formatMarketSelectLabel(market)}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    <MarketTags
                      markets={shareMarkets}
                      selectedMarketEmails={
                        normalizedSelectedShareMarketEmail
                          ? [normalizedSelectedShareMarketEmail]
                          : []
                      }
                    />
                    {marketInvalid ? (
                      <div className="text-xs text-destructive">
                        {t("share.accountMarket.required", {
                          defaultValue: "请选择一个 Share Market",
                        })}
                      </div>
                    ) : null}
                    {marketsLoading && shareMarkets.length === 0 ? (
                      <div className="text-xs text-muted-foreground">
                        {t("common.loading", { defaultValue: "Loading" })}
                      </div>
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
                    {!marketsLoading &&
                    !marketsError &&
                    shareMarkets.length === 0 ? (
                      <div className="text-xs text-muted-foreground">
                        {t("share.accountMarket.empty", {
                          defaultValue: "暂无可委托的 share market",
                        })}
                      </div>
                    ) : null}
                  </div>
                ) : null}

                {/* Share To：与 EditShareDialog 风格一致，按 app 分别输入；只为
                    已经绑了 provider 的 app 渲染，避免对未绑定 app 写入空 ACL。 */}
                <div className="space-y-1.5 md:col-span-2">
                  <Label>
                    {t("share.sharedWithEmails", { defaultValue: "Share To" })}
                  </Label>
                  <div className="text-xs text-muted-foreground">
                    {t("share.createDialog.shareToHint", {
                      defaultValue:
                        "每个 App 独立配置可访问邮箱；登录 cc-switch-router 后即可看到对应 share。留空 = 仅 owner 可见。",
                    })}
                  </div>
                  <div className="grid gap-2 pt-1">
                    {[activeSettingsApp].map((app) => (
                      <div key={app} className="space-y-1.5">
                        <Label className="text-xs uppercase text-muted-foreground">
                          {shareAppDisplayLabel(app)}
                        </Label>
                        <EmailTagsInput
                          value={shareToEmailsByApp[app] ?? []}
                          invalid={
                            shareToInvalid &&
                            (shareToEmailsByApp[app] ?? []).some(
                              (email) => email && !isValidEmail(email),
                            )
                          }
                          onChange={(emails) =>
                            setShareToEmailsByApp((current) => ({
                              ...current,
                              [app]: emails,
                            }))
                          }
                          placeholder={t("share.sharedWithEmailsPlaceholder", {
                            defaultValue:
                              "friend@example.com, teammate@example.com",
                          })}
                        />
                      </div>
                    ))}
                  </div>
                  {shareToInvalid ? (
                    <p className="text-xs text-destructive">
                      {t("share.validation.invalidEmail", {
                        defaultValue: "存在无效邮箱，请检查后再创建",
                      })}
                    </p>
                  ) : null}
                </div>

                <div className="space-y-1.5">
                  <Label htmlFor="share-expires">{t("share.expiresIn")}</Label>
                  <Input
                    id="share-expires"
                    type="number"
                    disabled={isPermanent}
                    {...form.register("expiresInSecs", { valueAsNumber: true })}
                  />
                  <div className="flex flex-wrap gap-1.5">
                    {EXPIRY_PRESETS.map((preset) => (
                      <Button
                        key={preset.value}
                        type="button"
                        variant="outline"
                        size="sm"
                        className="h-7 px-2 text-xs"
                        disabled={isPermanent}
                        onClick={() =>
                          form.setValue("expiresInSecs", preset.value)
                        }
                      >
                        {t(preset.labelKey)}
                      </Button>
                    ))}
                  </div>
                  <div className="flex items-center gap-2 pt-1">
                    <Checkbox
                      id="share-expires-permanent"
                      checked={isPermanent}
                      onCheckedChange={(checked) => {
                        const next = checked === true;
                        setIsPermanent(next);
                        if (next) {
                          form.setValue(
                            "expiresInSecs",
                            permanentExpiresInSecs(),
                            { shouldValidate: true },
                          );
                        } else {
                          form.setValue("expiresInSecs", 24 * 3600, {
                            shouldValidate: true,
                          });
                        }
                      }}
                    />
                    <Label
                      htmlFor="share-expires-permanent"
                      className="cursor-pointer text-sm font-normal"
                    >
                      {t("share.expiry.permanent")}
                    </Label>
                  </div>
                  <FieldError
                    error={form.formState.errors.expiresInSecs?.message}
                  />
                </div>

                <div className="space-y-1.5">
                  <div className="flex items-center justify-between gap-3">
                    <Label htmlFor="share-token-limit">
                      {t("share.tokenLimit")}
                    </Label>
                    <div className="flex items-center gap-2">
                      <Checkbox
                        id="share-token-limit-unlimited"
                        checked={unlimitedTokenLimit}
                        onCheckedChange={(checked) => {
                          const next = checked === true;
                          if (next) {
                            if (
                              typeof tokenLimit === "number" &&
                              tokenLimit > 0
                            ) {
                              setLastFiniteTokenLimit(tokenLimit);
                            }
                            form.setValue("tokenLimit", UNLIMITED_TOKEN_LIMIT, {
                              shouldDirty: true,
                              shouldValidate: true,
                            });
                            return;
                          }
                          form.setValue("tokenLimit", lastFiniteTokenLimit, {
                            shouldDirty: true,
                            shouldValidate: true,
                          });
                        }}
                      />
                      <Label
                        htmlFor="share-token-limit-unlimited"
                        className="cursor-pointer text-sm font-normal"
                      >
                        {t("share.unlimited")}
                      </Label>
                    </div>
                  </div>
                  <Input
                    id="share-token-limit"
                    type="number"
                    disabled={unlimitedTokenLimit}
                    {...tokenLimitField}
                    onChange={(event) => {
                      tokenLimitField.onChange(event);
                      const next = Number.parseInt(event.target.value, 10);
                      if (Number.isFinite(next) && next > 0) {
                        setLastFiniteTokenLimit(next);
                      }
                    }}
                  />
                  <div className="flex flex-wrap gap-2">
                    {TOKEN_PRESETS.map((preset) => (
                      <Button
                        key={preset}
                        type="button"
                        variant="outline"
                        size="sm"
                        className="h-7 px-2 text-xs"
                        disabled={unlimitedTokenLimit}
                        onClick={() => {
                          setLastFiniteTokenLimit(preset);
                          form.setValue("tokenLimit", preset, {
                            shouldDirty: true,
                            shouldValidate: true,
                          });
                        }}
                      >
                        {preset.toLocaleString()}
                      </Button>
                    ))}
                  </div>
                  <FieldError
                    error={form.formState.errors.tokenLimit?.message}
                  />
                </div>

                <div className="space-y-1.5">
                  <div className="flex items-center justify-between gap-3">
                    <Label htmlFor="share-parallel-limit">
                      {t("share.parallelLimit")}
                    </Label>
                    <div className="flex items-center gap-2">
                      <Checkbox
                        id="share-parallel-limit-unlimited"
                        checked={unlimitedParallelLimit}
                        onCheckedChange={(checked) => {
                          const next = checked === true;
                          if (next) {
                            if (
                              typeof parallelLimit === "number" &&
                              parallelLimit >= MIN_PARALLEL_LIMIT
                            ) {
                              setLastFiniteParallelLimit(parallelLimit);
                            }
                            form.setValue(
                              "parallelLimit",
                              UNLIMITED_PARALLEL_LIMIT,
                              { shouldDirty: true, shouldValidate: true },
                            );
                            return;
                          }
                          form.setValue(
                            "parallelLimit",
                            lastFiniteParallelLimit,
                            { shouldDirty: true, shouldValidate: true },
                          );
                        }}
                      />
                      <Label
                        htmlFor="share-parallel-limit-unlimited"
                        className="cursor-pointer text-sm font-normal"
                      >
                        {t("share.unlimited")}
                      </Label>
                    </div>
                  </div>
                  <Input
                    id="share-parallel-limit"
                    type="number"
                    min={MIN_PARALLEL_LIMIT}
                    disabled={unlimitedParallelLimit}
                    {...parallelLimitField}
                    onChange={(event) => {
                      parallelLimitField.onChange(event);
                      const next = Number.parseInt(event.target.value, 10);
                      if (Number.isFinite(next) && next >= MIN_PARALLEL_LIMIT) {
                        setLastFiniteParallelLimit(next);
                      }
                    }}
                  />
                  <div className="text-xs text-muted-foreground">
                    {t("share.parallelLimitHint")}
                  </div>
                  <FieldError
                    error={form.formState.errors.parallelLimit?.message}
                  />
                </div>
              </div>
            ) : null}
          </div>

          {!advancedExpanded ? (
            <div className="rounded-md border border-dashed border-border-default bg-muted/10 px-3 py-2 text-xs text-muted-foreground">
              <div className="font-medium">
                {t("share.createDialog.summaryHeading", {
                  defaultValue: "将以默认设置创建：",
                })}
              </div>
              <ul className="mt-1 grid gap-0.5 md:grid-cols-2">
                {summary.map((line) => (
                  <li key={line.key}>
                    <span className="text-muted-foreground">
                      {line.label}：
                    </span>
                    <span>{line.value}</span>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}

          {descriptionValue.length > 200 ? null : null}
        </div>

        <DialogFooter className="px-5 py-4">
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button
            onClick={handleCreateClick}
            disabled={
              isSubmitting ||
              tunnelConfigSaving ||
              ownerEmailInvalid ||
              (advancedOpened && marketInvalid)
            }
          >
            {submitLabel ?? t("share.create")}
          </Button>
        </DialogFooter>
      </DialogContent>

      <ConfirmDialog
        isOpen={confirmFreeOpen}
        title={t("share.forSaleFreeConfirmTitle")}
        message={t("share.forSaleFreeConfirmMessage")}
        variant="destructive"
        zIndex="top"
        onConfirm={() => {
          form.setValue("forSale", "Free", {
            shouldDirty: true,
            shouldValidate: true,
          });
          setConfirmFreeOpen(false);
        }}
        onCancel={() => setConfirmFreeOpen(false)}
      />

      <ConfirmDialog
        isOpen={defaultsConfirmOpen}
        title={t("share.createDialog.defaultsConfirm.title", {
          defaultValue: "确认使用默认设置创建？",
        })}
        message={[
          t("share.createDialog.defaultsConfirm.body", {
            defaultValue: '你未展开 "高级设置"，将按以下默认值创建：',
          }),
          "",
          ...summary.map((line) => `• ${line.label}：${line.value}`),
        ].join("\n")}
        confirmText={t("share.createDialog.defaultsConfirm.confirm", {
          defaultValue: "确认创建",
        })}
        cancelText={t("share.createDialog.defaultsConfirm.cancel", {
          defaultValue: "返回修改",
        })}
        variant="info"
        zIndex="top"
        onConfirm={handleDefaultsConfirmAccept}
        onCancel={() => setDefaultsConfirmOpen(false)}
      />
    </Dialog>
  );
}

function toShareAppType(app?: AppId): "claude" | "codex" | "gemini" {
  if (app === "codex" || app === "gemini") return app;
  return "claude";
}

const EMAIL_PATTERN = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

function isValidEmail(value: string): boolean {
  return EMAIL_PATTERN.test(value);
}

function normalizeEmails(emails: string[]): string[] {
  return uniqueSortedEmails(
    emails
      .map((email) => email.trim().toLowerCase())
      .filter((email) => email.length > 0 && isValidEmail(email)),
  );
}

function uniqueSortedEmails(emails: string[]): string[] {
  return Array.from(new Set(emails)).sort();
}

function shareAppDisplayLabel(app: keyof ShareBindings): string {
  if (app === "claude") return "Claude";
  if (app === "codex") return "Codex";
  return "Gemini";
}

function formatMarketSelectLabel(market: PublicMarket): string {
  return market.displayName.replace(/^https?:\/\//i, "");
}

function FieldError({ error }: { error?: string }) {
  if (!error) return null;
  return <p className={cn("text-sm text-destructive")}>{error}</p>;
}

function MarketTags({
  markets,
  marketAccessMode = "selected",
  selectedMarketEmails,
  removable = false,
  disabled = false,
  onRemove,
}: {
  markets: PublicMarket[];
  marketAccessMode?: "selected" | "all";
  selectedMarketEmails: string[];
  removable?: boolean;
  disabled?: boolean;
  onRemove?: (email: string) => void;
}) {
  const { t } = useTranslation();
  const marketByEmail = new Map(
    markets.map((market) => [market.email.toLowerCase(), market]),
  );

  if (marketAccessMode === "all") {
    return (
      <Badge variant="secondary" className="w-fit text-xs">
        {t("share.market.allSelected", {
          defaultValue: "已选中所有 Market",
        })}
      </Badge>
    );
  }

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

interface SummaryLine {
  key: string;
  label: string;
  value: string;
}

function buildDefaultsSummary(
  t: ReturnType<typeof useTranslation>["t"],
  values: {
    forSale: "Yes" | "No" | "Free";
    saleMarketKind: "token" | "share";
    marketAccessMode: "selected" | "all";
    expiresInSecs: number;
    isPermanent: boolean;
    tokenLimit: number;
    parallelLimit: number;
    subdomain: string;
    providerBinding: string;
  },
): SummaryLine[] {
  const lines: SummaryLine[] = [
    {
      key: "providerBinding",
      label: t("share.providerBindings", { defaultValue: "Provider 绑定" }),
      value: values.providerBinding,
    },
    {
      key: "forSale",
      label: t("share.forSale"),
      value: t(`share.forSaleOptions.${values.forSale.toLowerCase()}`),
    },
    {
      key: "market",
      label: t("share.market.title", { defaultValue: "Market" }),
      value:
        values.saleMarketKind === "share"
          ? t("share.saleMarketKind.share", { defaultValue: "Share Market" })
          : values.marketAccessMode === "all"
            ? t("share.market.allSelected", {
                defaultValue: "已选中所有 Token Market",
              })
            : t("share.market.default", {
                defaultValue: "默认，不授权 Market",
              }),
    },
    {
      key: "expiry",
      label: t("share.expiresAt"),
      value: values.isPermanent
        ? t("share.expiry.permanentLabel", { defaultValue: "永久" })
        : t("share.createDialog.summary.expiresInSecs", {
            defaultValue: "{{seconds}} 秒",
            seconds: values.expiresInSecs,
          }),
    },
    {
      key: "tokenLimit",
      label: t("share.tokenLimit"),
      value: isUnlimitedTokenLimit(values.tokenLimit)
        ? t("share.unlimited", { defaultValue: "无上限" })
        : values.tokenLimit.toLocaleString(),
    },
    {
      key: "parallelLimit",
      label: t("share.parallelLimit"),
      value: isUnlimitedParallelLimit(values.parallelLimit)
        ? t("share.unlimited", { defaultValue: "无上限" })
        : String(values.parallelLimit),
    },
    {
      key: "subdomain",
      label: t("share.subdomain"),
      value: values.subdomain.trim()
        ? values.subdomain.trim()
        : t("share.createDialog.subdomainAuto", {
            defaultValue: "(由后端生成)",
          }),
    },
  ];
  return lines;
}
