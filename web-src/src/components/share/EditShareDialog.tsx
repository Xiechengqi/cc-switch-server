import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { X } from "lucide-react";
import type {
  PublicMarket,
  ShareAccessByApp,
  ShareAppSettingsByApp,
  ShareBindings,
  ShareRecord,
} from "@/lib/api";
import { SHARE_APP_TYPES } from "@/lib/api";
import { DYNAMIC_BINDING_VALUE, type ProviderOption } from "./CreateShareDialog";
import { formatProviderOptionLabel } from "./providerOptions";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Checkbox } from "@/components/ui/checkbox";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
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
import { EmailTagsInput } from "@/components/ui/tags-input";
import { cn } from "@/lib/utils";
import type { ShareProviderSalePricing } from "./ShareCard";
import { useShareBindingHistoryQuery } from "@/lib/query/share";
import {
  DEFAULT_PARALLEL_LIMIT,
  isPermanentExpiry,
  isUnlimitedParallelLimit,
  isUnlimitedTokenLimit,
  MIN_PARALLEL_LIMIT,
  PERMANENT_EXPIRES_AT,
  UNLIMITED_PARALLEL_LIMIT,
  UNLIMITED_TOKEN_LIMIT,
} from "@/utils/shareUtils";

interface EditShareDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  share: ShareRecord;
  markets: PublicMarket[];
  providerSalePricing: ShareProviderSalePricing[];
  /**
   * P8 多 app share：每个 app_type 各自一组候选。`disabled` 表示该 provider 已被其他
   * active share 占用——除了本 share 自己的当前绑定（要让"保持原 provider"始终可选）。
   */
  providersByApp: Record<keyof ShareBindings, ProviderOption[]>;
  /**
   * P9-C：`${appType}:${providerId}` → provider 显示名。binding history 中如果命中此
   * 映射就显示名字，否则回退显示 provider id。
   */
  providerNameByKey?: Record<string, string>;
  marketsLoading: boolean;
  marketsError: string | null;
  readOnly?: boolean;
  subdomainReadOnly?: boolean;
  onRetryMarkets?: () => void;
  isBusy: boolean;
  onUpdateTokenLimit: (
    share: ShareRecord,
    tokenLimit: number,
  ) => Promise<void> | void;
  onUpdateParallelLimit: (
    share: ShareRecord,
    parallelLimit: number,
  ) => Promise<void> | void;
  onUpdateSubdomain: (
    share: ShareRecord,
    subdomain: string,
  ) => Promise<void> | void;
  onUpdateDescription: (
    share: ShareRecord,
    description: string,
  ) => Promise<void> | void;
  onUpdateForSale: (
    share: ShareRecord,
    forSale: "Yes" | "No" | "Free",
  ) => Promise<void> | void;
  onUpdateShareSalePricing: (
    share: ShareRecord,
    pricing: Record<string, number>,
  ) => Promise<void> | void;
  onUpdateExpiration: (
    share: ShareRecord,
    expiresAt: string,
  ) => Promise<void> | void;
  onUpdateOwnerEmail: (
    share: ShareRecord,
    ownerEmail: string,
  ) => Promise<void> | void;
  onTransferOwner: (
    share: ShareRecord,
    targetEmail: string,
  ) => Promise<void> | void;
  onUpdateAcl: (
    share: ShareRecord,
    sharedWithEmails: string[],
    marketAccessMode: "selected" | "all",
    accessByApp?: ShareAccessByApp,
    saleMarketKind?: "token" | "share",
    appSettings?: ShareAppSettingsByApp,
  ) => Promise<void> | void;
  /**
   * P8：改绑 / 解绑 share 的某个 app_type slot 的 provider。
   * `providerId = null` 表示清空该 slot（解绑）。后端约束：share 必须先 paused。
   */
  onUpdateProviderBinding: (
    share: ShareRecord,
    appType: keyof ShareBindings,
    providerId: string | null,
    options?: { dynamic?: boolean },
  ) => Promise<void> | void;
  /**
   * A-3：当 share 处于 active 状态时，点击"自动暂停并改绑"按钮触发
   * disable → rebind → enable 的链式操作。完成后 share 仍为 active。
   * `providerId = null` 表示在该 slot 上做"自动暂停并解绑"。
   */
  onRebindAtomic?: (
    share: ShareRecord,
    appType: keyof ShareBindings,
    newProviderId: string | null,
    options?: { dynamic?: boolean },
  ) => Promise<void> | void;
}

export function EditShareDialog({
  open,
  onOpenChange,
  share,
  markets,
  providerSalePricing,
  providersByApp,
  providerNameByKey,
  marketsLoading,
  marketsError,
  readOnly = false,
  subdomainReadOnly = false,
  onRetryMarkets,
  isBusy,
  onUpdateTokenLimit,
  onUpdateParallelLimit,
  onUpdateSubdomain,
  onUpdateDescription,
  onUpdateForSale,
  onUpdateShareSalePricing,
  onUpdateExpiration,
  onUpdateOwnerEmail,
  onTransferOwner,
  onUpdateAcl,
  onUpdateProviderBinding,
  onRebindAtomic,
}: EditShareDialogProps) {
  const { t } = useTranslation();
  const [saving, setSaving] = useState(false);
  const [confirmFreeOpen, setConfirmFreeOpen] = useState(false);
  const [transferTargetEmail, setTransferTargetEmail] = useState<string | null>(
    null,
  );

  const [tokenLimitInput, setTokenLimitInput] = useState("");
  const [tokenLimitUnlimited, setTokenLimitUnlimited] = useState(false);
  const [lastFiniteTokenLimit, setLastFiniteTokenLimit] = useState(100000);
  const [parallelLimitInput, setParallelLimitInput] = useState("");
  const [parallelLimitUnlimited, setParallelLimitUnlimited] = useState(false);
  const [lastFiniteParallelLimit, setLastFiniteParallelLimit] = useState(
    DEFAULT_PARALLEL_LIMIT,
  );
  const [subdomainInput, setSubdomainInput] = useState("");
  const [providerIdInputs, setProviderIdInputs] = useState<
    Record<string, string>
  >({});
  const [descriptionInput, setDescriptionInput] = useState("");
  const [ownerEmailInput, setOwnerEmailInput] = useState("");
  const [shareToEmailsByApp, setShareToEmailsByApp] = useState<
    Record<keyof ShareBindings, string[]>
  >({ claude: [], codex: [], gemini: [] });
  const [activeSettingsApp, setActiveSettingsApp] =
    useState<keyof ShareBindings>("claude");
  const [selectedMarketEmails, setSelectedMarketEmails] = useState<string[]>(
    [],
  );
  const [marketAccessModeInput, setMarketAccessModeInput] = useState<
    "selected" | "all"
  >("selected");
  const [saleMarketKindInput, setSaleMarketKindInput] = useState<
    "token" | "share"
  >("token");
  const [selectedShareMarketEmail, setSelectedShareMarketEmail] = useState("");
  const [marketSelectKey, setMarketSelectKey] = useState(0);
  const [forSaleInput, setForSaleInput] = useState<"Yes" | "No" | "Free">("No");
  const [salePricingInputs, setSalePricingInputs] = useState<
    Record<string, string>
  >({});
  const [expiryDateInput, setExpiryDateInput] = useState("");
  const [expiryHourInput, setExpiryHourInput] = useState("");
  const [expiryMinuteInput, setExpiryMinuteInput] = useState("");
  const [expiryPermanent, setExpiryPermanent] = useState(false);

  // P12：providerSalePricing 是页面级 (claude/codex/gemini) 全量列表；本 share 没有
  // 绑定某个 app 时，这个 app 不参与外部分享——既不展示定价 row，也不能 dirty/save
  // 该 slot 的价格。所有下游 dirty/validity/render 都走这个 filtered 列表。
  const effectiveProviderSalePricing = useMemo(
    () =>
      providerSalePricing.filter((item) =>
        Boolean(share.bindings?.[item.app as keyof typeof share.bindings]),
      ),
    [providerSalePricing, share.bindings],
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
  const publicMarketEmailSet = useMemo(
    () => new Set(markets.map((market) => market.email.toLowerCase())),
    [markets],
  );
  const usageMarketEmailSet = useMemo(
    () => new Set(usageMarkets.map((market) => market.email.toLowerCase())),
    [usageMarkets],
  );
  const supportedAccessApps = useMemo(
    () => shareAccessApps(share),
    [share.bindings],
  );
  const currentAccessByApp = useMemo(
    () => effectiveAccessByApp(share),
    [share],
  );
  const currentMarketEmails = uniqueSorted(
    Object.values(currentAccessByApp).flatMap((access) =>
      (access.sharedWithEmails ?? [])
        .map((email) => email.trim().toLowerCase())
        .filter((email) => usageMarketEmailSet.has(email)),
    ),
  );
  const shareMarketEmailSet = useMemo(
    () => new Set(shareMarkets.map((market) => market.email.toLowerCase())),
    [shareMarkets],
  );
  const currentShareMarketEmails = uniqueSorted(
    Object.values(currentAccessByApp).flatMap((access) =>
      (access.sharedWithEmails ?? [])
        .map((email) => email.trim().toLowerCase())
        .filter((email) => shareMarketEmailSet.has(email)),
    ),
  );
  const currentNonMarketEmailsByApp = useMemo(() => {
    const result: Record<keyof ShareBindings, string[]> = {
      claude: [],
      codex: [],
      gemini: [],
    };
    for (const app of SHARE_APP_TYPES) {
      result[app] = uniqueSorted(
        (currentAccessByApp[app]?.sharedWithEmails ?? [])
          .map((email) => email.trim().toLowerCase())
          .filter((email) => email && !publicMarketEmailSet.has(email)),
      );
    }
    return result;
  }, [currentAccessByApp, publicMarketEmailSet]);
  const currentMarketAccessMode = share.marketAccessMode ?? "selected";
  const currentSaleMarketKind =
    share.saleMarketKind === "share" || share.saleMarketKind === "token"
      ? share.saleMarketKind
      : "token";
  const currentShareSalePricing = share.forSaleOfficialPricePercentByApp ?? {};
  const wasOpenRef = useRef(false);
  useEffect(() => {
    if (!open) {
      wasOpenRef.current = false;
      return;
    }
    if (wasOpenRef.current) return;
    wasOpenRef.current = true;
    setSaving(false);
    setTokenLimitInput(String(share.tokenLimit));
    setTokenLimitUnlimited(isUnlimitedTokenLimit(share.tokenLimit));
    setLastFiniteTokenLimit(
      !isUnlimitedTokenLimit(share.tokenLimit) && share.tokenLimit > 0
        ? share.tokenLimit
        : 100000,
    );
    setParallelLimitInput(String(share.parallelLimit));
    setParallelLimitUnlimited(isUnlimitedParallelLimit(share.parallelLimit));
    setLastFiniteParallelLimit(
      !isUnlimitedParallelLimit(share.parallelLimit) &&
        share.parallelLimit >= MIN_PARALLEL_LIMIT
        ? share.parallelLimit
        : DEFAULT_PARALLEL_LIMIT,
    );
    setSubdomainInput(share.subdomain ?? "");
    // P8：每个 app_type slot 独立。"" 表示该 slot 未绑定。
    setProviderIdInputs({
      claude: share.dynamicApps?.includes("claude")
        ? DYNAMIC_BINDING_VALUE
        : (share.bindings.claude ?? ""),
      codex: share.dynamicApps?.includes("codex")
        ? DYNAMIC_BINDING_VALUE
        : (share.bindings.codex ?? ""),
      gemini: share.dynamicApps?.includes("gemini")
        ? DYNAMIC_BINDING_VALUE
        : (share.bindings.gemini ?? ""),
    });
    setDescriptionInput(share.description ?? "");
    setOwnerEmailInput(share.ownerEmail ?? "");
    setShareToEmailsByApp(currentNonMarketEmailsByApp);
    setSelectedMarketEmails(currentMarketEmails);
    setSelectedShareMarketEmail(currentShareMarketEmails[0] ?? "");
    setMarketAccessModeInput(currentMarketAccessMode);
    setSaleMarketKindInput(currentSaleMarketKind);
    setForSaleInput(share.forSale);
    setSalePricingInputs(
      salePricingInputValues(
        effectiveProviderSalePricing,
        currentShareSalePricing,
      ),
    );
    const permanent = isPermanentExpiry(share.expiresAt);
    setExpiryPermanent(permanent);
    const expires = new Date(share.expiresAt);
    if (!Number.isNaN(expires.getTime())) {
      setExpiryDateInput(
        `${expires.getFullYear()}-${String(expires.getMonth() + 1).padStart(
          2,
          "0",
        )}-${String(expires.getDate()).padStart(2, "0")}`,
      );
      setExpiryHourInput(String(expires.getHours()).padStart(2, "0"));
      setExpiryMinuteInput(String(expires.getMinutes()).padStart(2, "0"));
    } else {
      setExpiryDateInput("");
      setExpiryHourInput("");
      setExpiryMinuteInput("");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, share, effectiveProviderSalePricing]);

  const parsedTokenLimit = Number.parseInt(tokenLimitInput, 10);
  const tokenLimitDirty =
    Number.isFinite(parsedTokenLimit) && parsedTokenLimit !== share.tokenLimit;
  const tokenLimitInvalid =
    tokenLimitInput.trim().length === 0 ||
    !Number.isFinite(parsedTokenLimit) ||
    (parsedTokenLimit <= 0 && parsedTokenLimit !== UNLIMITED_TOKEN_LIMIT);
  const parsedParallelLimit = Number.parseInt(parallelLimitInput, 10);
  const parallelLimitDirty =
    Number.isFinite(parsedParallelLimit) &&
    parsedParallelLimit !== share.parallelLimit;
  const parallelLimitInvalid =
    parallelLimitInput.trim().length === 0 ||
    !Number.isFinite(parsedParallelLimit) ||
    (parsedParallelLimit !== UNLIMITED_PARALLEL_LIMIT &&
      parsedParallelLimit < MIN_PARALLEL_LIMIT);
  const subdomainDirty = subdomainInput.trim() !== (share.subdomain ?? "");
  const subdomainInvalid =
    subdomainInput.trim().length < 3 ||
    !/^[a-z0-9](?:[a-z0-9-]{1,61}[a-z0-9])?$/.test(subdomainInput.trim()) ||
    ["admin", "api", "www", "cdn-cgi"].includes(subdomainInput.trim());
  const sharePaused = share.status === "paused";
  const dynamicAppSet = useMemo(
    () => new Set(share.dynamicApps ?? []),
    [share.dynamicApps],
  );
  // P8：每个 slot 独立 dirty 检查。"" 视为 "未绑定"，与 share.bindings 缺键等价。
  const bindingChanges = useMemo(() => {
    return SHARE_APP_TYPES.map((app: keyof ShareBindings) => {
      const original = dynamicAppSet.has(app)
        ? DYNAMIC_BINDING_VALUE
        : (share.bindings[app] ?? "");
      const input = (providerIdInputs[app] ?? "").trim();
      return { app, original, input, dirty: input !== original };
    });
  }, [dynamicAppSet, providerIdInputs, share.bindings]);
  const nextAccessApps = useMemo<Array<keyof ShareBindings>>(() => {
    const bound = SHARE_APP_TYPES.filter((app) => {
      const providerId = (providerIdInputs[app] ?? "").trim();
      return providerId.length > 0;
    });
    return bound.length > 0 ? bound : supportedAccessApps;
  }, [providerIdInputs, supportedAccessApps]);
  const bindingsDirty = bindingChanges.some((entry) => entry.dirty);
  const duplicateFixedProviderIds = useMemo(() => {
    const counts = new Map<string, number>();
    for (const entry of bindingChanges) {
      if (!entry.input) continue;
      if (entry.input === DYNAMIC_BINDING_VALUE) continue;
      const remainsDynamic = dynamicAppSet.has(entry.app) && !entry.dirty;
      if (remainsDynamic) continue;
      counts.set(entry.input, (counts.get(entry.input) ?? 0) + 1);
    }
    return new Set(
      [...counts.entries()]
        .filter(([, count]) => count > 1)
        .map(([providerId]) => providerId),
    );
  }, [bindingChanges, dynamicAppSet]);
  const fixedBindingDuplicate = duplicateFixedProviderIds.size > 0;
  // P16：active 状态下也允许编辑 binding select。保存时若 share 还在 active，走
  // onRebindAtomic（disable → update → enable）兜底；后端 update_provider_binding
  // 仍然保留 paused-required 约束，所以"直接绕过"的请求会被拒，安全性由后端守门。
  // 此处只解开 UI 禁用，避免用户在弹窗里只能看不能改。
  const bindingsReadOnly = readOnly;
  const normalizedDescription = descriptionInput.trim();
  const descriptionDirty = normalizedDescription !== (share.description ?? "");
  const descriptionInvalid = normalizedDescription.length > 200;
  const normalizedOwnerEmail = ownerEmailInput.trim().toLowerCase();
  const ownerEmailDirty = normalizedOwnerEmail !== share.ownerEmail;
  const ownerEmailInvalid =
    !normalizedOwnerEmail ||
    !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(normalizedOwnerEmail);
  const normalizedShareToByApp = useMemo(() => {
    const result: Record<keyof ShareBindings, string[]> = {
      claude: [],
      codex: [],
      gemini: [],
    };
    for (const app of SHARE_APP_TYPES) {
      result[app] = uniqueSorted(
        (shareToEmailsByApp[app] ?? [])
          .map((value) => value.trim().toLowerCase())
          .filter((value) => value && !publicMarketEmailSet.has(value)),
      );
    }
    return result;
  }, [shareToEmailsByApp, publicMarketEmailSet]);
  const shareToDirty = nextAccessApps.some(
    (app) =>
      JSON.stringify(normalizedShareToByApp[app]) !==
      JSON.stringify(currentNonMarketEmailsByApp[app]),
  );
  const normalizedSelectedMarketEmails = uniqueSorted(
    marketAccessModeInput === "all"
      ? []
      : selectedMarketEmails.filter((email) => usageMarketEmailSet.has(email)),
  );
  const normalizedSelectedShareMarketEmail = shareMarketEmailSet.has(
    selectedShareMarketEmail.trim().toLowerCase(),
  )
    ? selectedShareMarketEmail.trim().toLowerCase()
    : forSaleInput === "Yes" && saleMarketKindInput === "share"
      ? (shareMarkets[0]?.email?.trim().toLowerCase() ?? "")
      : "";

  useEffect(() => {
    if (!open || forSaleInput !== "Yes" || saleMarketKindInput !== "share") {
      return;
    }
    const currentEmail = selectedShareMarketEmail.trim().toLowerCase();
    if (currentEmail && shareMarketEmailSet.has(currentEmail)) return;
    const firstShareMarket = shareMarkets[0]?.email?.trim().toLowerCase();
    if (firstShareMarket) {
      setSelectedShareMarketEmail(firstShareMarket);
    }
  }, [
    forSaleInput,
    open,
    saleMarketKindInput,
    selectedShareMarketEmail,
    shareMarketEmailSet,
    shareMarkets,
  ]);

  const marketDirty =
    saleMarketKindInput !== currentSaleMarketKind ||
    (saleMarketKindInput === "token" &&
      (marketAccessModeInput !== currentMarketAccessMode ||
        (marketAccessModeInput === "selected" &&
          JSON.stringify(normalizedSelectedMarketEmails) !==
            JSON.stringify(currentMarketEmails)))) ||
    (saleMarketKindInput === "share" &&
      normalizedSelectedShareMarketEmail !==
        (currentShareMarketEmails[0] ?? ""));
  const nextAccessByApp = useMemo(() => {
    const result: ShareAccessByApp = {};
    for (const app of nextAccessApps) {
      result[app] = {
        sharedWithEmails: uniqueSorted([
          ...(normalizedShareToByApp[app] ?? []),
          ...(saleMarketKindInput === "share"
            ? normalizedSelectedShareMarketEmail
              ? [normalizedSelectedShareMarketEmail]
              : []
            : marketAccessModeInput === "all"
              ? []
              : normalizedSelectedMarketEmails),
        ]),
        marketAccessMode:
          saleMarketKindInput === "share" ? "selected" : marketAccessModeInput,
      };
    }
    return result;
  }, [
    marketAccessModeInput,
    normalizedSelectedMarketEmails,
    normalizedSelectedShareMarketEmail,
    normalizedShareToByApp,
    nextAccessApps,
    saleMarketKindInput,
  ]);
  const nextAclEmails = uniqueSorted(
    Object.values(nextAccessByApp).flatMap(
      (access) => access?.sharedWithEmails ?? [],
    ),
  );
  const aclDirty = shareToDirty || marketDirty;
  const shareToInvalid = Object.values(normalizedShareToByApp).some((emails) =>
    emails.some((email) => !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)),
  );
  const forSaleDirty = forSaleInput !== share.forSale;
  const salePricingCurrentValues = salePricingInputValues(
    effectiveProviderSalePricing,
    currentShareSalePricing,
  );
  const salePricingDirty =
    saleMarketKindInput === "share"
      ? Object.keys(currentShareSalePricing).length > 0
      : effectiveProviderSalePricing.some(
          (item) =>
            (salePricingInputs[item.app] ?? "") !==
            (salePricingCurrentValues[item.app] ?? ""),
        );
  const salePricingInvalid =
    saleMarketKindInput === "token" &&
    effectiveProviderSalePricing.some((item) => {
      const value = (salePricingInputs[item.app] ?? "").trim();
      if (value === "") return false;
      if (!/^\d+$/.test(value)) return true;
      const parsed = Number.parseInt(value, 10);
      return parsed < 1 || parsed > 100;
    });
  const parsedExpiryHour = Number.parseInt(expiryHourInput, 10);
  const parsedExpiryMinute = Number.parseInt(expiryMinuteInput, 10);
  const computedExpiryIso =
    expiryDateInput &&
    Number.isFinite(parsedExpiryHour) &&
    Number.isFinite(parsedExpiryMinute) &&
    parsedExpiryHour >= 0 &&
    parsedExpiryHour <= 23 &&
    parsedExpiryMinute >= 0 &&
    parsedExpiryMinute <= 59
      ? new Date(
          Number.parseInt(expiryDateInput.slice(0, 4), 10),
          Number.parseInt(expiryDateInput.slice(5, 7), 10) - 1,
          Number.parseInt(expiryDateInput.slice(8, 10), 10),
          parsedExpiryHour,
          parsedExpiryMinute,
          0,
          0,
        ).toISOString()
      : "";
  const expiryIso = expiryPermanent ? PERMANENT_EXPIRES_AT : computedExpiryIso;
  const nextAppSettings = useMemo<ShareAppSettingsByApp>(() => {
    const result: ShareAppSettingsByApp = {};
    for (const app of nextAccessApps) {
      const access = nextAccessByApp[app];
      result[app] = {
        forSale: forSaleInput,
        saleMarketKind: saleMarketKindInput,
        marketAccessMode:
          saleMarketKindInput === "share" ? "selected" : marketAccessModeInput,
        sharedWithEmails: access?.sharedWithEmails ?? [],
        tokenLimit: parsedTokenLimit,
        parallelLimit: parsedParallelLimit,
        expiresAt: expiryIso || share.expiresAt,
      };
    }
    return result;
  }, [
    expiryIso,
    forSaleInput,
    marketAccessModeInput,
    nextAccessApps,
    nextAccessByApp,
    parsedParallelLimit,
    parsedTokenLimit,
    saleMarketKindInput,
    share.expiresAt,
  ]);
  const currentExpiryMs = new Date(share.expiresAt).getTime();
  const nextExpiryMs = expiryIso ? new Date(expiryIso).getTime() : NaN;
  const expiryDirty = expiryPermanent
    ? !isPermanentExpiry(share.expiresAt)
    : Boolean(
        expiryIso &&
          Number.isFinite(currentExpiryMs) &&
          Number.isFinite(nextExpiryMs) &&
          currentExpiryMs !== nextExpiryMs,
      );
  const expiryInvalid = expiryPermanent
    ? false
    : !expiryDateInput ||
      !Number.isFinite(parsedExpiryHour) ||
      !Number.isFinite(parsedExpiryMinute) ||
      parsedExpiryHour < 0 ||
      parsedExpiryHour > 23 ||
      parsedExpiryMinute < 0 ||
      parsedExpiryMinute > 59 ||
      Number.isNaN(new Date(expiryIso).getTime()) ||
      new Date(expiryIso).getTime() <= Date.now();
  const marketInvalid =
    forSaleInput === "Yes" &&
    saleMarketKindInput === "share" &&
    normalizedSelectedShareMarketEmail.length === 0;
  const appSettingsDirty =
    aclDirty || forSaleDirty || tokenLimitDirty || parallelLimitDirty || expiryDirty;
  const hasChanges =
    aclDirty ||
    forSaleDirty ||
    salePricingDirty ||
    ownerEmailDirty ||
    descriptionDirty ||
    expiryDirty ||
    subdomainDirty ||
    bindingsDirty ||
    tokenLimitDirty ||
    parallelLimitDirty;
  const hasInvalidChanges =
    (aclDirty && shareToInvalid) ||
    marketInvalid ||
    salePricingInvalid ||
    (ownerEmailDirty && ownerEmailInvalid) ||
    (descriptionDirty && descriptionInvalid) ||
    (expiryDirty && expiryInvalid) ||
    (subdomainDirty && subdomainInvalid) ||
    (bindingsDirty && fixedBindingDuplicate) ||
    (tokenLimitDirty && tokenLimitInvalid) ||
    (parallelLimitDirty && parallelLimitInvalid);

  const busy = isBusy || saving || readOnly;
  const marketDisabled = forSaleInput !== "Yes";
  const pricingDisabled =
    forSaleInput !== "Yes" || saleMarketKindInput !== "token";

  const handleSave = async () => {
    if (!hasChanges || hasInvalidChanges || busy) return;
    setSaving(true);
    try {
      if (appSettingsDirty)
        await onUpdateAcl(
          share,
          nextAclEmails,
          saleMarketKindInput === "share" ? "selected" : marketAccessModeInput,
          nextAccessByApp,
          saleMarketKindInput,
          nextAppSettings,
        );
      if (forSaleDirty) await onUpdateForSale(share, forSaleInput);
      if (salePricingDirty) {
        await onUpdateShareSalePricing(
          share,
          saleMarketKindInput === "share"
            ? {}
            : parseSalePricingInputs(salePricingInputs),
        );
      }
      if (ownerEmailDirty)
        await onUpdateOwnerEmail(share, normalizedOwnerEmail);
      if (descriptionDirty)
        await onUpdateDescription(share, normalizedDescription);
      if (expiryDirty) await onUpdateExpiration(share, expiryIso);
      if (subdomainDirty) await onUpdateSubdomain(share, subdomainInput.trim());
      // P8：逐 slot 写改动。`input === ""` 表示解绑（传 null 给后端）。
      // P16：share 还在 active 时走 onRebindAtomic（disable→update→enable）；
      // 后端 update_provider_binding 仍然只接受 paused share，这是为了避免请求
      // 落在改绑中间态。这里多绑几个时会触发多次 disable/enable —— 对正常 1-2
      // 个 slot 的改动可以接受；想完全单次 disable/enable 包住改动，手动 pause
      // 一下再 Save 就行。
      for (const entry of bindingChanges) {
        if (!entry.dirty) continue;
        const nextDynamic = entry.input === DYNAMIC_BINDING_VALUE;
        const nextProviderId =
          entry.input.length > 0 && !nextDynamic ? entry.input : null;
        if (!sharePaused && onRebindAtomic) {
          await onRebindAtomic(share, entry.app, nextProviderId, {
            dynamic: nextDynamic,
          });
        } else {
          await onUpdateProviderBinding(share, entry.app, nextProviderId, {
            dynamic: nextDynamic,
          });
        }
      }
      if (tokenLimitDirty) await onUpdateTokenLimit(share, parsedTokenLimit);
      if (parallelLimitDirty)
        await onUpdateParallelLimit(share, parsedParallelLimit);
      onOpenChange(false);
    } finally {
      setSaving(false);
    }
  };

  const handleTransferOwner = async () => {
    if (!transferTargetEmail || busy) return;
    setSaving(true);
    try {
      await onTransferOwner(share, transferTargetEmail);
      setTransferTargetEmail(null);
      onOpenChange(false);
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      <Dialog
        open={open}
        onOpenChange={(next) => {
          if (busy && !next) return;
          onOpenChange(next);
        }}
      >
        <DialogContent className="flex max-h-[90vh] w-full max-w-3xl flex-col gap-0 p-0">
          <DialogHeader>
            <div className="flex flex-col gap-1">
              <DialogTitle>
                {t("share.editDialog.title", { defaultValue: "设置选项" })}
                <span className="ml-2 text-sm font-normal text-muted-foreground">
                  {t("share.settings", { defaultValue: "Settings" })}
                </span>
              </DialogTitle>
            </div>
          </DialogHeader>

          <div className="flex-1 space-y-4 overflow-y-auto px-6 py-5">
            <div className="grid gap-4 md:grid-cols-2">
              <DialogSection
                title={t("share.ownerEmail", { defaultValue: "Owner Email" })}
                hint={t("share.ownerEmailCreateHint", {
                  defaultValue:
                    "该邮箱会作为 share owner 上报到 router。router 页面使用相同邮箱登录后可查看 API Key 和编辑设置。",
                })}
                invalid={ownerEmailDirty && ownerEmailInvalid}
              >
                <Input
                  type="email"
                  value={ownerEmailInput}
                  disabled={busy}
                  onChange={(event) => setOwnerEmailInput(event.target.value)}
                  placeholder="owner@example.com"
                />
              </DialogSection>

              <DialogSection
                title={t("share.subdomain", { defaultValue: "Subdomain" })}
                invalid={subdomainDirty && subdomainInvalid}
              >
                <Input
                  value={subdomainInput}
                  disabled={busy || subdomainReadOnly}
                  onChange={(event) =>
                    setSubdomainInput(event.target.value.toLowerCase())
                  }
                />
              </DialogSection>
            </div>

            <DialogSection
              title={t("share.description")}
              hint={
                <>
                  <span>{t("share.descriptionHint")}</span>
                  <span>{normalizedDescription.length}/200</span>
                </>
              }
              invalid={descriptionDirty && descriptionInvalid}
            >
              <Textarea
                value={descriptionInput}
                maxLength={200}
                disabled={busy}
                onChange={(event) => setDescriptionInput(event.target.value)}
                placeholder={t("share.descriptionPlaceholder", {
                  defaultValue: "可选，信息将显示在 cc-switch-router 侧边栏",
                })}
              />
            </DialogSection>

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
                  {shareAppLabel(app)}
                </button>
              ))}
            </div>

            <DialogSection
              title={t("share.providerBindings", {
                defaultValue: "Provider 绑定",
              })}
              invalid={bindingsDirty && fixedBindingDuplicate}
              hint={
                sharePaused
                  ? t("share.providerBindingsEditHint", {
                      defaultValue:
                        "为每个 app 独立挑一个 provider，留空 = 该 app 在本 share 上不可用。",
                    })
                  : t("share.providerBindingsEditHintActive", {
                      defaultValue:
                        "为每个 app 独立挑一个 provider，留空 = 该 app 在本 share 上不可用。保存时若 share 还在分享中，会先暂停 → 改绑 → 自动恢复。",
                    })
              }
            >
              <div className="grid gap-3">
                {[activeSettingsApp].map((app) => {
                  const candidates = providersByApp[app] ?? [];
                  const value = providerIdInputs[app] ?? "";
                  const isDynamic = value === DYNAMIC_BINDING_VALUE;
                  const selectedProvider = candidates.find(
                    (provider) => provider.id === value,
                  );
                  const selectedInOtherFixedSlot = (providerId: string) =>
                    bindingChanges.some((entry) => {
                      if (entry.app === app || entry.input !== providerId) {
                        return false;
                      }
                      return !(dynamicAppSet.has(entry.app) && !entry.dirty);
                    });
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
                          value={value}
                          disabled={busy || bindingsReadOnly}
                          onValueChange={(next) =>
                            setProviderIdInputs((prev) => ({
                              ...prev,
                              [app]: next,
                            }))
                          }
                        >
                          <SelectTrigger className="flex-1">
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
                                  defaultValue: "暂无可绑定 provider",
                                })}
                              </SelectItem>
                            ) : (
                              candidates.map((provider) => {
                                const duplicateInForm =
                                  selectedInOtherFixedSlot(provider.id);
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
                                        ? t("share.providerBindingSelected", {
                                            defaultValue:
                                              "已在本 share 其他分支选择",
                                          })
                                        : t("share.providerBindingTaken", {
                                            defaultValue: "已被其他 share 绑定",
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
                            disabled={busy || bindingsReadOnly}
                            onClick={() =>
                              setProviderIdInputs((prev) => ({
                                ...prev,
                                [app]: "",
                              }))
                            }
                            title={t("share.providerBindingClear", {
                              defaultValue: "清空（解绑）",
                            })}
                          >
                            <X className="h-4 w-4" />
                          </Button>
                        ) : null}
                      </div>
                      {/* P16：原本有一个 "自动暂停并改绑 → 恢复" 行内按钮。
                          现在 Save 已经按 share.status 自动选 update vs.
                          rebindAtomic，行内按钮变成多余的入口反而引人疑惑，
                          统一收回到 Save。 */}
                    </div>
                  );
                })}
                {bindingsDirty && fixedBindingDuplicate ? (
                  <div className="text-xs text-destructive">
                    {t("share.validation.providerDuplicate", {
                      defaultValue:
                        "同一个固定 Provider 只能绑定一个 share 分支",
                    })}
                  </div>
                ) : null}
              </div>
            </DialogSection>

            {/* P9-C：binding 改动审计历史。默认折叠，展开后才拉数据，避免每次开 Dialog 都查 DB。 */}
            <BindingHistorySection
              shareId={share.id}
              providerNameByKey={providerNameByKey}
            />

            <DialogSection
              title={t("share.sharedWithEmails", { defaultValue: "Share To" })}
              hint={t("share.sharedWithEmailsHint", {
                defaultValue:
                  "每个分支可单独配置可访问邮箱；这些邮箱登录 cc-switch-router dashboard 后可查看对应 share。",
              })}
              invalid={shareToDirty && shareToInvalid}
            >
              <div className="space-y-3">
                {[activeSettingsApp].map((app) => (
                  <div key={app} className="space-y-1.5">
                    <Label className="text-xs text-muted-foreground">
                      {shareAppLabel(app)}
                    </Label>
                    <EmailTagsInput
                      value={shareToEmailsByApp[app] ?? []}
                      disabled={busy}
                      invalid={shareToDirty && shareToInvalid}
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
                      onPromote={(email) => setTransferTargetEmail(email)}
                      promotableEmails={uniqueSorted(
                        Object.values(currentNonMarketEmailsByApp).flat(),
                      )}
                      promoteLabel={t("share.transferOwner.action", {
                        defaultValue: "设为 Owner",
                      })}
                    />
                  </div>
                ))}
              </div>
            </DialogSection>

            <DialogSection
              title={t("share.forSale", { defaultValue: "For Sale" })}
              hint={t("share.editDialog.forSaleDisableHint", {
                defaultValue:
                  "选择 Free 或 No 时，目标市场和模型定价将不可编辑。",
              })}
            >
              <div className="flex flex-wrap items-center gap-5">
                {(["Yes", "No", "Free"] as const).map((value) => {
                  const id = `edit-share-for-sale-${share.id}-${value}`;
                  return (
                    <label
                      key={value}
                      htmlFor={id}
                      className="flex cursor-pointer items-center gap-2 text-sm"
                    >
                      <input
                        id={id}
                        type="radio"
                        name={`edit-share-for-sale-${share.id}`}
                        value={value}
                        checked={forSaleInput === value}
                        disabled={busy}
                        onChange={() => {
                          if (value === "Free" && share.forSale !== "Free") {
                            setConfirmFreeOpen(true);
                            return;
                          }
                          setForSaleInput(value);
                        }}
                        className="h-4 w-4 accent-primary"
                      />
                      <span>
                        {t(`share.forSaleOptions.${value.toLowerCase()}`)}
                      </span>
                    </label>
                  );
                })}
              </div>
            </DialogSection>

            <DialogSection
              title={t("share.saleMarketKind.title", {
                defaultValue: "Market Type",
              })}
              hint={
                marketDisabled
                  ? t("share.market.forSaleRequired", {
                      defaultValue:
                        "Set ForSale to Yes before choosing a market.",
                    })
                  : t("share.saleMarketKind.description", {
                      defaultValue:
                        "Choose Token Market for token usage sale or Share Market for account rental.",
                    })
              }
            >
              <div className="flex flex-wrap items-center gap-5">
                {(["token", "share"] as const).map((value) => {
                  const id = `edit-share-sale-market-kind-${share.id}-${value}`;
                  return (
                    <label
                      key={value}
                      htmlFor={id}
                      className="flex cursor-pointer items-center gap-2 text-sm"
                    >
                      <input
                        id={id}
                        type="radio"
                        name={`edit-share-sale-market-kind-${share.id}`}
                        value={value}
                        checked={saleMarketKindInput === value}
                        disabled={busy || marketDisabled}
                        onChange={() => {
                          setSaleMarketKindInput(value);
                          if (value === "token") {
                            setMarketAccessModeInput("all");
                            setSelectedMarketEmails([]);
                          } else {
                            setMarketAccessModeInput("selected");
                          }
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
            </DialogSection>

            {saleMarketKindInput === "token" ? (
              <DialogSection
                title={t("share.market.title", {
                  defaultValue: "Token Market",
                })}
                hint={
                  marketDisabled
                    ? t("share.market.forSaleRequired", {
                        defaultValue:
                          "Set ForSale to Yes before choosing a market.",
                      })
                    : t("share.market.description", {
                        defaultValue: "Choose all or selected token markets.",
                      })
                }
              >
                <div className="space-y-3">
                  <div className="flex gap-2">
                    <Select
                      key={marketSelectKey}
                      onValueChange={(value) => {
                        if (value === "__all__") {
                          setMarketAccessModeInput("all");
                          setSelectedMarketEmails([]);
                          setMarketSelectKey((current) => current + 1);
                          return;
                        }
                        setMarketAccessModeInput("selected");
                        setSelectedMarketEmails((current) =>
                          uniqueSorted([...current, value.toLowerCase()]),
                        );
                        setMarketSelectKey((current) => current + 1);
                      }}
                      disabled={busy || marketDisabled || marketsLoading}
                    >
                      <SelectTrigger
                        aria-label={t("share.market.select", {
                          defaultValue: "Select market",
                        })}
                      >
                        <SelectValue
                          placeholder={
                            marketsLoading
                              ? t("common.loading", { defaultValue: "Loading" })
                              : t("share.market.select", {
                                  defaultValue: "Select market",
                                })
                          }
                        />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="__all__">
                          {t("share.market.all", { defaultValue: "All" })}
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
                        busy ||
                        marketDisabled ||
                        (marketAccessModeInput === "selected" &&
                          selectedMarketEmails.length === 0)
                      }
                      onClick={() => {
                        setMarketAccessModeInput("selected");
                        setSelectedMarketEmails([]);
                      }}
                    >
                      {t("share.market.restore", { defaultValue: "还原" })}
                    </Button>
                  </div>
                  <MarketTags
                    markets={usageMarkets}
                    marketAccessMode={marketAccessModeInput}
                    selectedMarketEmails={normalizedSelectedMarketEmails}
                    removable
                    disabled={busy || marketDisabled}
                    onRemove={(email) =>
                      setSelectedMarketEmails((current) =>
                        current.filter((item) => item !== email),
                      )
                    }
                  />
                  {marketAccessModeInput !== "all" &&
                  normalizedSelectedMarketEmails.length === 0 ? (
                    <div className="text-sm text-muted-foreground">
                      {t("share.market.default", {
                        defaultValue: "默认，不授权 Market",
                      })}
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
                </div>
              </DialogSection>
            ) : null}

            {saleMarketKindInput === "share" ? (
              <DialogSection
                title={t("share.accountMarket.title", {
                  defaultValue: "Share Market",
                })}
                hint={
                  marketDisabled
                    ? t("share.accountMarket.forSaleRequired", {
                        defaultValue:
                          "Set ForSale to Yes before delegating an account market.",
                      })
                    : t("share.accountMarket.description", {
                        defaultValue:
                          "Choose one share market for account-hosted sale.",
                      })
                }
              >
                <div className="space-y-3">
                  <Select
                    key={`share-market-${selectedShareMarketEmail || "none"}`}
                    value={selectedShareMarketEmail || undefined}
                    onValueChange={(value) => {
                      setSelectedShareMarketEmail(value.toLowerCase());
                    }}
                    disabled={
                      busy ||
                      marketDisabled ||
                      marketsLoading ||
                      shareMarkets.length === 0
                    }
                  >
                    <SelectTrigger
                      aria-label={t("share.accountMarket.select", {
                        defaultValue: "Select share market",
                      })}
                    >
                      <SelectValue
                        placeholder={
                          marketsLoading
                            ? t("common.loading", { defaultValue: "Loading" })
                            : t("share.accountMarket.select", {
                                defaultValue: "Select share market",
                              })
                        }
                      />
                    </SelectTrigger>
                    <SelectContent>
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
                    <div className="text-sm text-destructive">
                      {t("share.accountMarket.required", {
                        defaultValue: "请选择一个 Share Market",
                      })}
                    </div>
                  ) : null}
                  {shareMarkets.length === 0 && !marketsLoading ? (
                    <div className="text-sm text-muted-foreground">
                      {t("share.accountMarket.empty", {
                        defaultValue: "暂无可委托的 share market",
                      })}
                    </div>
                  ) : null}
                </div>
              </DialogSection>
            ) : null}

            {saleMarketKindInput === "token" &&
            effectiveProviderSalePricing.length > 0 ? (
              <DialogSection
                title={t("share.modelPricingPercentTitle", {
                  defaultValue: "模型定价（Share 默认；供应商非空时优先）",
                })}
                invalid={salePricingInvalid}
              >
                <div className="grid gap-3 md:grid-cols-3">
                  {effectiveProviderSalePricing.map((item) => (
                    <div key={item.app} className="space-y-1">
                      <Label className="text-xs text-muted-foreground">
                        {item.label}
                      </Label>
                      <Input
                        type="number"
                        min="1"
                        max="100"
                        step="1"
                        inputMode="numeric"
                        value={salePricingInputs[item.app] ?? ""}
                        disabled={busy || pricingDisabled}
                        onChange={(event) =>
                          setSalePricingInputs((current) => ({
                            ...current,
                            [item.app]: event.target.value,
                          }))
                        }
                        placeholder={
                          item.providerName
                            ? t("share.forSaleOfficialPricePercentEmpty", {
                                defaultValue: "未设置",
                              })
                            : t("share.forSaleOfficialPricePercentNoProvider", {
                                defaultValue: "无当前节点",
                              })
                        }
                      />
                      <div className="truncate text-xs text-muted-foreground">
                        {item.percent == null
                          ? (item.providerName ?? "-")
                          : t("share.providerPricingOverrideHint", {
                              defaultValue:
                                "{{provider}} provider override: {{percent}}%",
                              provider: item.providerName ?? item.label,
                              percent: item.percent,
                            })}
                      </div>
                    </div>
                  ))}
                </div>
              </DialogSection>
            ) : null}

            <div className="grid gap-4 md:grid-cols-3">
              <DialogSection
                title={t("share.expiresAt")}
                hint={t("share.expirationEditHint")}
                invalid={expiryDirty && expiryInvalid}
              >
                <div className="space-y-2">
                  <div className="flex items-center gap-2">
                    <input
                      id={`edit-share-expiry-permanent-${share.id}`}
                      type="radio"
                      name={`edit-share-expiry-mode-${share.id}`}
                      checked={expiryPermanent}
                      disabled={busy}
                      onChange={() => setExpiryPermanent(true)}
                      className="h-4 w-4 accent-primary"
                    />
                    <Label
                      htmlFor={`edit-share-expiry-permanent-${share.id}`}
                      className="cursor-pointer text-sm font-normal"
                    >
                      {t("share.expiry.permanent", {
                        defaultValue: "永久有效",
                      })}
                    </Label>
                  </div>
                  <div className="flex items-center gap-2">
                    <input
                      id={`edit-share-expiry-pick-${share.id}`}
                      type="radio"
                      name={`edit-share-expiry-mode-${share.id}`}
                      checked={!expiryPermanent}
                      disabled={busy}
                      onChange={() => setExpiryPermanent(false)}
                      className="h-4 w-4 accent-primary"
                    />
                    <Label
                      htmlFor={`edit-share-expiry-pick-${share.id}`}
                      className="cursor-pointer text-sm font-normal"
                    >
                      {t("share.expiry.pickDate", { defaultValue: "选择日期" })}
                    </Label>
                  </div>
                  <Input
                    type="date"
                    value={expiryPermanent ? "2099-12-31" : expiryDateInput}
                    disabled={busy || expiryPermanent}
                    onChange={(event) => setExpiryDateInput(event.target.value)}
                  />
                  <div className="grid grid-cols-2 gap-2">
                    <Input
                      type="number"
                      min={0}
                      max={23}
                      value={expiryPermanent ? "23" : expiryHourInput}
                      disabled={busy || expiryPermanent}
                      onChange={(event) =>
                        setExpiryHourInput(event.target.value)
                      }
                    />
                    <Input
                      type="number"
                      min={0}
                      max={59}
                      value={expiryPermanent ? "59" : expiryMinuteInput}
                      disabled={busy || expiryPermanent}
                      onChange={(event) =>
                        setExpiryMinuteInput(event.target.value)
                      }
                    />
                  </div>
                </div>
              </DialogSection>

              <DialogSection
                title={t("share.tokenLimit")}
                invalid={tokenLimitDirty && tokenLimitInvalid}
              >
                <div className="flex items-center gap-2">
                  <Input
                    type="number"
                    min={1}
                    step={1}
                    value={tokenLimitInput}
                    disabled={busy || tokenLimitUnlimited}
                    onChange={(event) => {
                      setTokenLimitInput(event.target.value);
                      const next = Number.parseInt(event.target.value, 10);
                      if (Number.isFinite(next) && next > 0) {
                        setLastFiniteTokenLimit(next);
                      }
                    }}
                  />
                  <label className="flex items-center gap-2 whitespace-nowrap text-sm">
                    <Checkbox
                      id={`edit-share-token-limit-unlimited-${share.id}`}
                      checked={tokenLimitUnlimited}
                      disabled={busy}
                      onCheckedChange={(checked) => {
                        const next = checked === true;
                        setTokenLimitUnlimited(next);
                        if (next) {
                          if (
                            Number.isFinite(parsedTokenLimit) &&
                            parsedTokenLimit > 0
                          ) {
                            setLastFiniteTokenLimit(parsedTokenLimit);
                          }
                          setTokenLimitInput(String(UNLIMITED_TOKEN_LIMIT));
                          return;
                        }
                        setTokenLimitInput(String(lastFiniteTokenLimit));
                      }}
                    />
                    <span className="cursor-pointer">
                      {t("share.unlimited", { defaultValue: "无上限" })}
                    </span>
                  </label>
                </div>
              </DialogSection>

              <DialogSection
                title={t("share.parallelLimit", {
                  defaultValue: "最大并发数",
                })}
                hint={t("share.parallelLimitHint")}
                invalid={parallelLimitDirty && parallelLimitInvalid}
              >
                <div className="flex items-center gap-2">
                  <Input
                    type="number"
                    min={MIN_PARALLEL_LIMIT}
                    step={1}
                    value={parallelLimitInput}
                    disabled={busy || parallelLimitUnlimited}
                    onChange={(event) => {
                      setParallelLimitInput(event.target.value);
                      const next = Number.parseInt(event.target.value, 10);
                      if (Number.isFinite(next) && next >= MIN_PARALLEL_LIMIT) {
                        setLastFiniteParallelLimit(next);
                      }
                    }}
                  />
                  <label className="flex items-center gap-2 whitespace-nowrap text-sm">
                    <Checkbox
                      id={`edit-share-parallel-limit-unlimited-${share.id}`}
                      checked={parallelLimitUnlimited}
                      disabled={busy}
                      onCheckedChange={(checked) => {
                        const next = checked === true;
                        setParallelLimitUnlimited(next);
                        if (next) {
                          if (
                            Number.isFinite(parsedParallelLimit) &&
                            parsedParallelLimit >= MIN_PARALLEL_LIMIT
                          ) {
                            setLastFiniteParallelLimit(parsedParallelLimit);
                          }
                          setParallelLimitInput(
                            String(UNLIMITED_PARALLEL_LIMIT),
                          );
                          return;
                        }
                        setParallelLimitInput(String(lastFiniteParallelLimit));
                      }}
                    />
                    <span className="cursor-pointer">
                      {t("share.unlimited", { defaultValue: "无上限" })}
                    </span>
                  </label>
                </div>
              </DialogSection>
            </div>
          </div>

          <DialogFooter>
            <Button
              variant="outline"
              disabled={isBusy || saving}
              onClick={() => onOpenChange(false)}
            >
              {t("common.cancel", { defaultValue: "取消" })}
            </Button>
            {!readOnly ? (
              <Button
                disabled={!hasChanges || hasInvalidChanges || busy}
                onClick={() => void handleSave()}
              >
                {t("share.editDialog.save", { defaultValue: "保存设置" })}
              </Button>
            ) : null}
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ConfirmDialog
        isOpen={confirmFreeOpen}
        title={t("share.forSaleFreeConfirmTitle")}
        message={t("share.forSaleFreeConfirmMessage")}
        variant="destructive"
        onConfirm={() => {
          setForSaleInput("Free");
          setConfirmFreeOpen(false);
        }}
        onCancel={() => setConfirmFreeOpen(false)}
      />
      <ConfirmDialog
        isOpen={Boolean(transferTargetEmail)}
        title={t("share.transferOwner.confirmTitle", {
          defaultValue: "转移 Owner?",
        })}
        message={t("share.transferOwner.confirmMessage", {
          defaultValue:
            "将 {{target}} 升级为 owner，并把当前 owner {{owner}} 降级为 shareto。此操作会同步到 router。",
          target: transferTargetEmail ?? "",
          owner: share.ownerEmail,
        })}
        onConfirm={handleTransferOwner}
        onCancel={() => setTransferTargetEmail(null)}
      />
    </>
  );
}

function uniqueSorted(values: string[]) {
  return Array.from(
    new Set(values.map((value) => value.trim().toLowerCase()).filter(Boolean)),
  ).sort();
}

function salePricingInputValues(
  providerSalePricing: ShareProviderSalePricing[],
  sharePricing: Record<string, number>,
) {
  return Object.fromEntries(
    providerSalePricing.map((item) => {
      const percent = sharePricing[item.app];
      return [item.app, percent == null ? "" : String(percent)];
    }),
  );
}

function parseSalePricingInputs(inputs: Record<string, string>) {
  return Object.fromEntries(
    Object.entries(inputs)
      .map(([app, value]) => [app, value.trim()] as const)
      .filter(([, value]) => value !== "")
      .map(([app, value]) => [app, Number.parseInt(value, 10)]),
  );
}

function DialogSection({
  title,
  hint,
  invalid = false,
  children,
}: {
  title: string;
  hint?: ReactNode;
  invalid?: boolean;
  children: ReactNode;
}) {
  return (
    <section
      className={cn(
        "rounded-lg border border-border-default bg-background/60 px-4 py-3",
        invalid && "border-destructive/60",
      )}
    >
      <div className="mb-2 text-sm font-semibold">{title}</div>
      {children}
      {hint ? (
        <div className="mt-2 flex items-center justify-between gap-2 text-xs text-muted-foreground">
          {hint}
        </div>
      ) : null}
    </section>
  );
}

function shareAccessApps(share: ShareRecord): Array<keyof ShareBindings> {
  const bound = SHARE_APP_TYPES.filter((app) => Boolean(share.bindings?.[app]));
  return bound.length > 0 ? bound : [...SHARE_APP_TYPES];
}

function effectiveAccessByApp(share: ShareRecord): ShareAccessByApp {
  if (share.accessByApp && Object.keys(share.accessByApp).length > 0) {
    return share.accessByApp;
  }
  const result: ShareAccessByApp = {};
  for (const app of shareAccessApps(share)) {
    result[app] = {
      sharedWithEmails: share.sharedWithEmails ?? [],
      marketAccessMode: share.marketAccessMode ?? "selected",
    };
  }
  return result;
}

function shareAppLabel(app: keyof ShareBindings) {
  if (app === "claude") return "Claude";
  if (app === "codex") return "Codex";
  return "Gemini";
}

function formatMarketSelectLabel(market: PublicMarket): string {
  return market.displayName.replace(/^https?:\/\//i, "");
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
      <div className="text-sm text-muted-foreground">
        {t("share.market.allSelected", {
          defaultValue: "已选中所有 Market",
        })}
      </div>
    );
  }

  if (selectedMarketEmails.length === 0) return null;

  return (
    <div className="flex flex-wrap gap-2">
      {selectedMarketEmails.map((email) => {
        const market = marketByEmail.get(email);
        return (
          <Badge
            key={email}
            variant="secondary"
            className="max-w-full gap-1 rounded-md px-2 py-1 text-xs"
          >
            <span className="min-w-0 truncate">
              {market?.displayName ?? email}
            </span>
            {removable ? (
              <button
                type="button"
                className="rounded-sm p-0.5 hover:bg-background/70 disabled:opacity-50"
                disabled={disabled}
                onClick={() => onRemove?.(email)}
                aria-label={`Remove ${market?.displayName ?? email}`}
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

/**
 * P9-C：binding 改动审计历史的展示。默认折叠不拉数据，用户展开后才发起查询，避免
 * 每次开 EditDialog 就触发 list_share_binding_history。
 *
 * 渲染时优先用 providerNameByKey (`${appType}:${pid}`) 显示 provider 名，命中不到
 * 时回退到 provider id；"解绑"事件用 italic "—" 表示。
 */
function BindingHistorySection({
  shareId,
  providerNameByKey,
}: {
  shareId: string;
  providerNameByKey?: Record<string, string>;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const { data, isLoading, error } = useShareBindingHistoryQuery(
    shareId,
    open,
    50,
  );

  const renderProvider = (
    appType: string,
    providerId: string | null | undefined,
  ): ReactNode => {
    if (!providerId) {
      return (
        <span className="italic text-muted-foreground">
          {t("share.bindingHistory.unbound", { defaultValue: "—（解绑）" })}
        </span>
      );
    }
    const name = providerNameByKey?.[`${appType}:${providerId}`];
    return name ? (
      <span>
        {name}
        <span className="ml-1 text-[10px] text-muted-foreground">
          ({providerId})
        </span>
      </span>
    ) : (
      <span className="font-mono">{providerId}</span>
    );
  };

  return (
    <DialogSection
      title={t("share.bindingHistory.title", {
        defaultValue: "绑定历史",
      })}
      hint={t("share.bindingHistory.hint", {
        defaultValue: "最近 50 条改绑 / 解绑事件。按时间倒序。",
      })}
    >
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => setOpen((prev) => !prev)}
      >
        {open
          ? t("share.bindingHistory.hide", { defaultValue: "收起" })
          : t("share.bindingHistory.show", { defaultValue: "查看历史" })}
      </Button>
      {open ? (
        <div className="mt-2 grid gap-1 text-xs">
          {isLoading ? (
            <span className="text-muted-foreground">
              {t("share.bindingHistory.loading", {
                defaultValue: "加载中…",
              })}
            </span>
          ) : error ? (
            <span className="text-destructive">
              {t("share.bindingHistory.error", {
                defaultValue: "拉取历史失败",
              })}
            </span>
          ) : !data || data.length === 0 ? (
            <span className="text-muted-foreground">
              {t("share.bindingHistory.empty", {
                defaultValue: "暂无改绑历史。",
              })}
            </span>
          ) : (
            <ul className="grid gap-1">
              {data.map((entry) => (
                <li
                  key={entry.id}
                  className="rounded border border-default/40 p-2"
                >
                  <div className="flex items-center justify-between gap-2 text-[10px] uppercase text-muted-foreground">
                    <span className="font-mono">{entry.appType}</span>
                    <span>{entry.changedAt}</span>
                  </div>
                  <div className="mt-1 flex flex-wrap items-center gap-1 text-foreground">
                    {renderProvider(entry.appType, entry.oldProviderId)}
                    <span className="text-muted-foreground">→</span>
                    {renderProvider(entry.appType, entry.newProviderId)}
                  </div>
                </li>
              ))}
            </ul>
          )}
        </div>
      ) : null}
    </DialogSection>
  );
}
