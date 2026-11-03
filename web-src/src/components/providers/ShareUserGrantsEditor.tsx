import { useEffect, useMemo, useRef, useState } from "react";
import { Pencil, Plus, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import type {
  ShareTokenPeriod,
  ShareUserGrant,
  ShareUserGrantMap,
  ShareUserPolicy,
  ShareUserUsageEdit,
  ShareUserUsageEditMap,
  ShareUserValueUsageInitializationMap,
  AccountResetPreview,
} from "@/lib/api/share";
import { shareApi } from "@/lib/api/share";
import { isValidShareEmail, stageGrantPolicy } from "@/utils/shareFormUtils";
import { applyShareUserPolicyBatch, patchedSharePolicy, type ShareUserPolicyBatchPatch } from "./share-user-policy-batch";
import { ShareValueUsageCorrection } from "./ShareValueUsageCorrection";
import {
  ShareValueLimitField, ShareValuePreviewLine, ShareValueReadiness, ShareValueScheduledLine, ShareValueUnitToggle, ShareValueUsageCell,
} from "./ShareValueReadiness";
import { useShareValuePreview, useShareValueReadiness, useShareValueStatus } from "./shareValueHooks";
import {
  matchingValueStatus, parseValueLimitInput, valueLimitErrorKey, valueLimitInput, valueLimitLabel, valuePreviewBlocker,
  valuePreviewNeedsConfirmation, valueUsageInput, valueUsageLabel, valueStateKey, parseValueUsage, parseInitialValueUsage, valueRowKind, type ValuePreview,
} from "@/lib/share-value-quota";

type PolicyDraft = {
  quotaUnit: "tokens" | "usdEquivalent";
  email: string;
  parallelLimit: string;
  tokenLimit: string;
  tokenPeriod: ShareTokenPeriod;
  tokenPeriodAnchor: string;
  expiresAt: string;
  consumedTokens: string;
  consumedValue: string;
  valueReason: string;
  unitDrafts?: Partial<Record<"tokens" | "usdEquivalent", {tokenLimit: string; consumedTokens: string; usageAction: "unchanged" | "set" | "clear"}>>;
  usageAction: "unchanged" | "set" | "clear";
};

type BatchPolicyDraft = Omit<PolicyDraft, "email" | "consumedValue" | "valueReason" | "unitDrafts"> & {
  applyParallelLimit: boolean;
  applyTokenLimit: boolean;
  applyConsumedTokens: boolean;
  applyExpiresAt: boolean;
};

const ANCHORED_PERIODS: ReadonlySet<ShareTokenPeriod> = new Set(["sevenDays", "thirtyDays"]);
const DAY_MS = 24 * 60 * 60 * 1000;

function fixedPeriodDurationMs(period: ShareTokenPeriod): number | undefined {
  if (period === "sevenDays") return 7 * DAY_MS;
  if (period === "thirtyDays") return 30 * DAY_MS;
  return undefined;
}

/**
 * `onChange` and `onUsageEditsChange` fire together in one event handler, so a
 * consumer that keeps both halves inside a single state object must apply each
 * update functionally; spreading a captured draft in both handlers drops the
 * grant change.
 */
type ShareUserGrantsEditorProps = {
  shareId?: string;
  configRevision?: number;
  value: ShareUserGrantMap;
  ownerEmail: string;
  defaultPolicy: ShareUserPolicy;
  protectedEmails?: ReadonlySet<string>;
  usageEdits?: ShareUserUsageEditMap;
  onUsageEditsChange?: (value: ShareUserUsageEditMap) => void;
  valueUsageInitializations?: ShareUserValueUsageInitializationMap;
  onValueUsageInitializationsChange?: (value: ShareUserValueUsageInitializationMap) => void;
  savedGrants?: ShareUserGrantMap;
  disabled?: boolean;
  onChange: (value: ShareUserGrantMap) => void;
};

function toLocalDateTime(value: number | undefined) {
  if (!value) return "";
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "";
  const pad = (part: number) => String(part).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function toUtcDateTime(value?: number) {
  const date = new Date(value ?? Math.floor(Date.now() / 60_000) * 60_000);
  if (!Number.isFinite(date.getTime())) return "";
  const pad = (part: number) => String(part).padStart(2, "0");
  return `${date.getUTCFullYear()}-${pad(date.getUTCMonth() + 1)}-${pad(date.getUTCDate())}T${pad(date.getUTCHours())}:${pad(date.getUTCMinutes())}`;
}

function parseUtcDateTime(value: string) {
  return value ? new Date(`${value}:00Z`).getTime() : undefined;
}

function fixedPeriodWindow(
  period: ShareTokenPeriod,
  anchorAtMs: number | undefined,
  nowMs = Date.now(),
): { start: number; end: number } | undefined {
  const duration = fixedPeriodDurationMs(period);
  if (duration == null || anchorAtMs == null || !Number.isFinite(anchorAtMs)) {
    return undefined;
  }
  const index = Math.floor((nowMs - anchorAtMs) / duration);
  const start = anchorAtMs + index * duration;
  return { start, end: start + duration };
}

function formatUtcWindow(value: number): string {
  return new Date(value).toISOString().replace("T", " ").slice(0, 16) + " UTC";
}

function policyDraft(email: string, policy: ShareUserPolicy): PolicyDraft {
  return {
    email,
    quotaUnit: policy.valueQuota ? "usdEquivalent" : "tokens",
    parallelLimit: policy.parallelLimit == null ? "" : String(policy.parallelLimit),
    tokenLimit: policy.valueQuota ? valueLimitInput(policy.valueQuota.limitUsdMicros) : policy.tokenLimit == null ? "" : String(policy.tokenLimit),
    tokenPeriod: policy.tokenPeriod ?? "lifetime",
    tokenPeriodAnchor: toUtcDateTime(policy.tokenPeriodAnchorAtMs),
    expiresAt: toLocalDateTime(policy.expiresAt),
    consumedTokens: "",
    consumedValue: "",
    valueReason: "",
    usageAction: "unchanged",
  };
}

function currentGrantTokens(
  grant: ShareUserGrant,
  usageEdits: ShareUserUsageEditMap = {},
): number {
  const edit = usageEdits[grant.email.trim().toLowerCase()];
  if (edit?.action === "set" && edit.targetTokens != null) {
    return edit.targetTokens;
  }
  if (edit?.action === "clear") {
    return grant.usageQuota?.observedTokensUsed ?? grantTokensFromUsage(grant);
  }
  // The Server-derived view is authoritative when present.  The bucket read
  // below only covers grants persisted before the view existed.
  if (grant.usageQuota) return grant.usageQuota.effectiveTokensUsed;
  return grantTokensFromUsage(grant);
}

function grantTokensFromUsage(grant: ShareUserGrant): number {
  const usage = grant.usage;
  if (!usage) return 0;
  switch (grant.policy.tokenPeriod) {
    case "day":
      return usage.day?.tokensUsed ?? 0;
    case "week":
      return usage.week?.tokensUsed ?? 0;
    case "calendarMonth":
      return usage.calendarMonth?.tokensUsed ?? 0;
    case "sevenDays":
    case "thirtyDays":
      return usage.anchored?.period === grant.policy.tokenPeriod
        ? usage.anchored.tokensUsed
        : 0;
    case "lifetime":
    default:
      return usage.lifetime?.tokensUsed ?? 0;
  }
}

/**
 * What the Usage history alone reports, which is the floor a new baseline may
 * not go below.  Read straight from the Server view rather than re-derived:
 * inverting the rebase formula here would disagree with the Server the moment
 * that formula changes, and the disagreement would only surface as a rejected
 * save.
 */
function observedGrantTokens(grant: ShareUserGrant): number {
  return grant.usageQuota?.observedTokensUsed ?? grantTokensFromUsage(grant);
}

function usageEditForGrant(
  grant: ShareUserGrant,
  usageEdits: ShareUserUsageEditMap,
): Pick<PolicyDraft, "consumedTokens" | "usageAction"> {
  const edit = usageEdits[grant.email.trim().toLowerCase()];
  if (edit?.action === "clear") {
    return { consumedTokens: "", usageAction: "clear" };
  }
  if (edit?.action === "set") {
    return {
      consumedTokens:
        edit.targetTokens == null ? "" : String(edit.targetTokens),
      usageAction: "set",
    };
  }
  return {
    consumedTokens: String(currentGrantTokens(grant)),
    usageAction: "unchanged",
  };
}

function batchPolicyDraft(grant: ShareUserGrant): BatchPolicyDraft {
  const draft = policyDraft(grant.email, grant.policy);
  return {
    quotaUnit: draft.quotaUnit,
    parallelLimit: draft.parallelLimit,
    tokenLimit: draft.tokenLimit,
    tokenPeriod: draft.tokenPeriod,
    tokenPeriodAnchor: draft.tokenPeriodAnchor,
    expiresAt: draft.expiresAt,
    consumedTokens: "",
    usageAction: "set",
    applyParallelLimit: false,
    applyTokenLimit: false,
    applyConsumedTokens: false,
    applyExpiresAt: false,
  };
}

function displayLimit(value: number | undefined, unlimited: string) {
  return value == null ? unlimited : value.toLocaleString();
}

function displayExpiry(value: number | undefined, permanent: string) {
  return value == null ? permanent : new Date(value).toLocaleString();
}

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function anchorInvalid(period: ShareTokenPeriod, anchorAtMs: number | undefined) {
  return ANCHORED_PERIODS.has(period) && (anchorAtMs == null || !Number.isFinite(anchorAtMs)
    || anchorAtMs > Math.floor(Date.now() / 60_000) * 60_000);
}

/** The policy a valid USD draft saves; preview and save share it so intents match exactly. */
function usdPolicyFor(draft: Pick<PolicyDraft, "parallelLimit" | "tokenLimit" | "tokenPeriod" | "tokenPeriodAnchor" | "expiresAt">): ShareUserPolicy | undefined {
  const amount = parseValueLimitInput(draft.tokenLimit);
  if (!amount.ok) return undefined;
  const parallelLimit = draft.parallelLimit.trim() ? Number(draft.parallelLimit) : undefined;
  const expiresAt = draft.expiresAt ? new Date(draft.expiresAt).getTime() : undefined;
  const tokenPeriodAnchorAtMs = ANCHORED_PERIODS.has(draft.tokenPeriod) ? parseUtcDateTime(draft.tokenPeriodAnchor) : undefined;
  if ((parallelLimit != null && (!Number.isInteger(parallelLimit) || parallelLimit < 1))
    || (expiresAt != null && !Number.isFinite(expiresAt)) || anchorInvalid(draft.tokenPeriod, tokenPeriodAnchorAtMs)) return undefined;
  return {
    parallelLimit,
    tokenLimit: undefined,
    valueQuota: { limitUsdMicros: amount.micros, tariffPolicyId: "api-equivalent-v1" },
    tokenPeriod: draft.tokenPeriod,
    tokenPeriodAnchorAtMs,
    expiresAt,
  };
}

function batchPatchFor(draft: BatchPolicyDraft): ShareUserPolicyBatchPatch {
  const parallelLimit = draft.parallelLimit.trim() ? Number(draft.parallelLimit) : undefined;
  const amount = draft.quotaUnit === "usdEquivalent" ? parseValueLimitInput(draft.tokenLimit) : undefined;
  const tokenLimit = draft.quotaUnit === "tokens" && draft.tokenLimit.trim() ? Number(draft.tokenLimit) : undefined;
  const expiresAt = draft.expiresAt ? new Date(draft.expiresAt).getTime() : undefined;
  const periodAnchorAtMs = ANCHORED_PERIODS.has(draft.tokenPeriod) ? parseUtcDateTime(draft.tokenPeriodAnchor) : undefined;
  return {
    ...(draft.applyParallelLimit ? { parallelLimit: { value: parallelLimit } } : {}),
    ...(draft.applyTokenLimit ? { tokenLimit: {
      value: tokenLimit,
      ...(amount?.ok ? { valueQuota: { limitUsdMicros: amount.micros, tariffPolicyId: "api-equivalent-v1" as const } } : {}),
      period: draft.tokenPeriod,
      periodAnchorAtMs,
    } } : {}),
    ...(draft.applyExpiresAt ? { expiresAt: { value: expiresAt } } : {}),
  };
}

export function ShareUserGrantsEditor({
  shareId,
  configRevision,
  value,
  ownerEmail,
  defaultPolicy,
  protectedEmails,
  usageEdits = {},
  onUsageEditsChange,
  valueUsageInitializations = {},
  onValueUsageInitializationsChange,
  savedGrants = value,
  disabled,
  onChange,
}: ShareUserGrantsEditorProps) {
  const { t } = useTranslation();
  const [accountPreview, setAccountPreview] = useState<AccountResetPreview>();
  const [previewIntent, setPreviewIntent] = useState("");
  const [previewBusy, setPreviewBusy] = useState(false);
  const [draftCycle, setDraftCycle] = useState<ShareUserGrant["accountQuotaCycle"]>();
  const [batchCycle, setBatchCycle] = useState<ShareUserGrant["accountQuotaCycle"]>();
  useEffect(() => {
    let cancelled = false;
    if (shareId && configRevision != null) {
      void shareApi.previewAccountReset(shareId, configRevision).then((result) => {
        if (!cancelled) setAccountPreview(result);
      }).catch(() => { if (!cancelled) setAccountPreview(undefined); });
    }
    return () => { cancelled = true; };
  }, [shareId, configRevision]);
  const accountReady = accountPreview?.accountResetCapability.syncStatus === "ready";
  const confirmAccountPreview = async (policies: Record<string, ShareUserPolicy>, intent: string) => {
    if (!shareId || configRevision == null) throw new Error(t("share.userLimit.accountReset.saveFirst"));
    if (previewIntent === intent && accountReady) return true;
    setPreviewBusy(true);
    try {
      const result = await shareApi.previewAccountReset(shareId, configRevision, policies);
      setAccountPreview(result);
      if (result.rows.some((row) => row.reason) || result.accountResetCapability.syncStatus !== "ready") {
        throw new Error(result.accountResetCapability.reason || t("share.userLimit.accountReset.notReady"));
      }
      setPreviewIntent(intent);
      return false;
    } finally { setPreviewBusy(false); }
  };
  const normalizedOwner = ownerEmail.trim().toLowerCase();
  const [editingEmail, setEditingEmail] = useState<string | null>(null);
  const [draft, setDraft] = useState<PolicyDraft | null>(null);
  const currentDraft = useRef(draft);
  currentDraft.current = draft;
  const [selecting, setSelecting] = useState(false);
  const [selectedEmails, setSelectedEmails] = useState<Set<string>>(new Set());
  const [batchDraft, setBatchDraft] = useState<BatchPolicyDraft | null>(null);
  const [batchError, setBatchError] = useState("");
  const [draftError, setDraftError] = useState("");

  const grants = useMemo(
    () =>
      // Each row shows the grant's own (enforced or staged) policy; a waiting
      // USD policy is rendered beside it, never in its place.
      Object.values(value)
        .filter((grant) => grant.active !== false)
        .sort((left, right) => {
          if (left.role === "owner") return -1;
          if (right.role === "owner") return 1;
          return left.email.localeCompare(right.email);
        }),
    [value],
  );
  const selectableEmails = useMemo(
    () => grants
      .filter((grant) => !protectedEmails?.has(grant.email) && !grant.valueQuotaMalformed)
      .map((grant) => grant.email),
    [grants, protectedEmails],
  );
  const selectableEmailKey = selectableEmails.join("\0");
  const selectedEditableEmails = new Set(
    selectableEmails.filter((email) => selectedEmails.has(email)),
  );
  const allSelected = selectableEmails.length > 0 &&
    selectableEmails.every((email) => selectedEditableEmails.has(email));
  const someSelected = selectedEditableEmails.size > 0;

  const singleUsdPolicies = (() => {
    if (draft?.quotaUnit !== "usdEquivalent") return undefined;
    const email = draft.email.trim().toLowerCase();
    const policy = isValidShareEmail(email) ? usdPolicyFor(draft) : undefined;
    return policy ? { [email]: policy } : undefined;
  })();
  const batchUsdPolicies = (() => {
    if (!batchDraft?.applyTokenLimit || batchDraft.quotaUnit !== "usdEquivalent" || !selectedEditableEmails.size) return undefined;
    const patch = batchPatchFor(batchDraft);
    const parallel = patch.parallelLimit?.value;
    if (!patch.tokenLimit?.valueQuota || anchorInvalid(batchDraft.tokenPeriod, patch.tokenLimit.periodAnchorAtMs)
      || (parallel != null && (!Number.isInteger(parallel) || parallel < 1))
      || (patch.expiresAt?.value != null && !Number.isFinite(patch.expiresAt.value))) return undefined;
    // Preview the policy being asked for, not what staging keeps enforced.
    return Object.fromEntries([...selectedEditableEmails].sort().map((email) => [email, patchedSharePolicy(value[email], patch)]));
  })();
  const dialogOpen = draft != null || batchDraft != null;
  const hasUsdGrant = grants.some((grant) => !grant.valueQuotaMalformed && (grant.policy.valueQuota || grant.pendingValueQuota));
  const usdSelected = draft?.quotaUnit === "usdEquivalent" || (!!batchDraft?.applyTokenLimit && batchDraft.quotaUnit === "usdEquivalent");
  // Check once per open dialog to gate `$`; poll only while USD is selected or in use.
  const readiness = useShareValueReadiness(shareId, configRevision, { enabled: dialogOpen, poll: dialogOpen && (usdSelected || hasUsdGrant) });
  const singlePreview = useShareValuePreview(shareId, configRevision, readiness.key, singleUsdPolicies);
  const batchPreview = useShareValuePreview(shareId, configRevision, readiness.key, batchUsdPolicies);
  // USD usage rows only matter once some user has (or is being given) a USD quota.
  const valueStatus = useShareValueStatus(shareId, configRevision != null && (hasUsdGrant || usdSelected));
  const [applying, setApplying] = useState(false);
  const [amountAttempted, setAmountAttempted] = useState(false);
  const [correctionEmail, setCorrectionEmail] = useState<string | null>(null);
  const needsConfirmation = (state: { preview?: ValuePreview }) =>
    !!state.preview && !valuePreviewBlocker(state.preview) && valuePreviewNeedsConfirmation(state.preview);
  /** True when it may apply now; an unseen schedule stops so the button reads "Confirm & apply". */
  const resolveValuePreview = async (state: typeof singlePreview) => {
    const alreadyShown = !!state.preview;
    setApplying(true);
    let preview: ValuePreview | undefined;
    try { preview = await state.fetchNow(); } finally { setApplying(false); }
    if (!preview || valuePreviewBlocker(preview)) return undefined;
    return alreadyShown || !valuePreviewNeedsConfirmation(preview) ? preview : undefined;
  };
  const limitError = (unit: PolicyDraft["quotaUnit"], raw: string) => {
    if (unit !== "usdEquivalent") return undefined;
    const parsed = parseValueLimitInput(raw);
    if (parsed.ok || (parsed.error === "empty" && !amountAttempted)) return undefined;
    return t(valueLimitErrorKey(parsed.error));
  };

  useEffect(() => {
    const selectable = new Set(selectableEmailKey ? selectableEmailKey.split("\0") : []);
    setSelectedEmails((current) => {
      const next = new Set(Array.from(current).filter((email) => selectable.has(email)));
      if (next.size === current.size && Array.from(next).every((email) => current.has(email))) {
        return current;
      }
      return next;
    });
  }, [selectableEmailKey]);

  useEffect(() => {
    if (!normalizedOwner || value[normalizedOwner]) return;
    onChange({
      ...value,
      [normalizedOwner]: {
        email: normalizedOwner,
        role: "owner",
        active: true,
        policy: { ...defaultPolicy },
      },
    });
  }, [defaultPolicy, normalizedOwner, onChange, value]);

  const editedEmail = draft?.email.trim().toLowerCase() ?? "";
  const savedGrant = savedGrants[editedEmail];
  const editingValueStatus = savedGrant ? matchingValueStatus(valueStatus.items, savedGrant) : undefined;
  const initialPreviewRow = singlePreview.preview?.rows.find(row => row.email === editedEmail);
  const canInitializeValue = !disabled && !!onValueUsageInitializationsChange && !savedGrant
    && singlePreview.preview?.ready === true
    && singlePreview.preview?.checks?.some(check => check.code === "value_usage_initialization" && check.ready)
    && initialPreviewRow?.canInitializeUsage === true && !!initialPreviewRow.windowId
    && valueRowKind(initialPreviewRow) !== "scheduled" && valueRowKind(initialPreviewRow) !== "blocked";

  const openAdd = () => {
    setCorrectionEmail(null);
    setPreviewIntent("");
    setAmountAttempted(false);
    setDraftCycle(accountPreview?.accountQuotaCycle);
    setEditingEmail(null);
    setDraftError("");
    setDraft({
      ...policyDraft("", { ...defaultPolicy, tokenPeriod: accountReady ? "accountReset" : defaultPolicy.tokenPeriod }),
      consumedTokens: "",
      usageAction: "unchanged",
    });
  };

  const openEdit = (grant: ShareUserGrant) => {
    if (protectedEmails?.has(grant.email) || grant.valueQuotaMalformed) return;
    setCorrectionEmail(null);
    setPreviewIntent("");
    setAmountAttempted(false);
    setDraftCycle(grant.accountQuotaCycle);
    setEditingEmail(grant.email);
    setDraftError("");
    // A waiting USD policy is what the owner last asked for, so edit that.
    setDraft({
      ...policyDraft(grant.email, grant.pendingValueQuota?.policy ?? grant.policy),
      ...usageEditForGrant(grant, usageEdits),
      consumedValue: valueUsageInput(valueUsageInitializations[grant.email]?.targetPicos),
      valueReason: valueUsageInitializations[grant.email]?.reason ?? "",
    });
  };

  const exitSelecting = () => {
    setSelecting(false);
    setSelectedEmails(new Set());
  };

  const openBatchEdit = () => {
    setPreviewIntent("");
    if (!selecting) {
      setSelecting(true);
      return;
    }
    const firstSelected = grants.find((grant) => selectedEditableEmails.has(grant.email));
    if (!firstSelected) return;
    setBatchCycle(firstSelected.accountQuotaCycle);
    setBatchError("");
    setAmountAttempted(false);
    setBatchDraft(batchPolicyDraft(firstSelected));
  };

  const saveDraft = async () => {
    if (!draft) return;
    if (draft.tokenPeriod === "accountReset" && draft.quotaUnit === "tokens") {
      try {
        if (!await confirmAccountPreview({ [draft.email]: { tokenPeriod: "accountReset",
          tokenLimit: draft.tokenLimit.trim() ? Number(draft.tokenLimit) : undefined } }, JSON.stringify(draft))) return;
      } catch (error) { setDraftError(errorText(error)); return; }
    }
    const email = draft.email.trim().toLowerCase();
    const parallelLimit = draft.parallelLimit.trim()
      ? Number(draft.parallelLimit)
      : undefined;
    if (draft.quotaUnit === "usdEquivalent" && !parseValueLimitInput(draft.tokenLimit).ok) {
      setAmountAttempted(true); setDraftError(""); return;
    }
    const tokenLimit = draft.quotaUnit==="tokens" && draft.tokenLimit.trim()
      ? Number(draft.tokenLimit)
      : undefined;
    const expiresAt = draft.expiresAt
      ? new Date(draft.expiresAt).getTime()
      : undefined;
    const anchored = ANCHORED_PERIODS.has(draft.tokenPeriod);
    const tokenPeriodAnchorAtMs = anchored
      ? parseUtcDateTime(draft.tokenPeriodAnchor)
      : undefined;
    const consumedTokens = draft.consumedTokens.trim()
      ? Number(draft.consumedTokens)
      : undefined;
    const previous = value[editingEmail ?? email];
    const reuseMarketTombstone =
      previous?.manager === "routerShareMarket" && previous.active === false;
    const observedTokens =
      previous && !reuseMarketTombstone ? observedGrantTokens(previous) : 0;
    const usageInvalid =
      draft.quotaUnit==="tokens" &&
      draft.usageAction === "set" &&
      !!draft.consumedTokens.trim() &&
      (consumedTokens == null ||
        !Number.isSafeInteger(consumedTokens) ||
        consumedTokens < 0 ||
        (draft.tokenPeriod !== "accountReset" && consumedTokens < observedTokens));
    if (
      !isValidShareEmail(email) ||
      (editingEmail == null && (Boolean(value[email]?.active) || protectedEmails?.has(email))) ||
      parallelLimit === 0 ||
      tokenLimit === 0 ||
      (parallelLimit != null && (!Number.isInteger(parallelLimit) || parallelLimit < 1)) ||
      (tokenLimit != null && (!Number.isInteger(tokenLimit) || tokenLimit < 1)) ||
      (expiresAt != null && !Number.isFinite(expiresAt)) ||
      (anchored && (
        tokenPeriodAnchorAtMs == null ||
          !Number.isFinite(tokenPeriodAnchorAtMs) ||
        tokenPeriodAnchorAtMs > Math.floor(Date.now() / 60_000) * 60_000
      )) ||
      usageInvalid
    ) {
      if (usageInvalid) {
        setDraftError(
          consumedTokens != null && consumedTokens < observedTokens
            ? t("share.userLimit.consumedBelowObserved", {
                defaultValue:
                  "已消耗 Token 不能低于当前观测值（{{observed}}）。",
                observed: observedTokens.toLocaleString(),
              })
            : t("share.userLimit.invalidUsage", {
                defaultValue: "已消耗 Token 必须是大于等于 0 的整数。",
              }),
        );
      } else {
        setDraftError(t("share.userLimit.invalidPolicy"));
      }
      return;
    }
    const usdPolicy = draft.quotaUnit === "usdEquivalent" ? usdPolicyFor(draft) : undefined;
    if (draft.quotaUnit === "usdEquivalent" && !usdPolicy) {
      // `$` is selected: never save a Token policy instead.
      setDraftError(t("share.userLimit.invalidPolicy"));
      return;
    }
    if (previous?.valueQuotaMalformed && !reuseMarketTombstone) {
      setDraftError(t("share.userLimit.valueQuota.malformed"));
      return;
    }
    setDraftError("");
    const requested: ShareUserPolicy = usdPolicy ?? {
      parallelLimit,
      tokenLimit,
      tokenPeriod: draft.tokenPeriod,
      tokenPeriodAnchorAtMs,
      expiresAt,
    };
    const base: ShareUserGrant = {
      ...(reuseMarketTombstone ? undefined : previous),
      email,
      role: email === normalizedOwner ? "owner" : "shareto",
      active: true,
      manager: email === normalizedOwner ? "owner" : "manual",
      entitlementId: undefined,
      revokedAtMs: undefined,
      accountQuotaCycle: draft.tokenPeriod === "accountReset"
        ? previous?.policy.tokenPeriod === "accountReset" ? draftCycle : accountPreview?.accountQuotaCycle
        : previous?.accountQuotaCycle,
      accountResetCapability: draft.tokenPeriod === "accountReset" ? accountPreview?.accountResetCapability : previous?.accountResetCapability,
      policy: previous && !reuseMarketTombstone ? previous.policy : requested,
    };
    const next = stageGrantPolicy(base, requested);
    let initialization: ShareUserValueUsageInitializationMap[string] | undefined;
    if (requested.valueQuota) {
      const preview = await resolveValuePreview(singlePreview);
      if (!preview) return;
      if (currentDraft.current !== draft) return;
      const parsed = parseInitialValueUsage(preview, email, draft.consumedValue, draft.valueReason, !!savedGrants[email]);
      if (!parsed.ok || (parsed.initialization && !onValueUsageInitializationsChange)) {
        setDraftError(t("share.userLimit.valueQuota.initialInvalid")); return;
      }
      initialization = parsed.initialization;
    }
    if (draft.quotaUnit==="tokens" && draft.usageAction === "set" && consumedTokens != null) {
      const previousQuota = previous?.usageQuota;
      const observed = previous ? observedGrantTokens(previous) : 0;
      next.usageQuota = {
        period: draft.tokenPeriod,
        anchorAtMs: tokenPeriodAnchorAtMs,
        windowStartsAtMs: previousQuota?.windowStartsAtMs,
        windowEndsAtMs: previousQuota?.windowEndsAtMs,
        effectiveTokensUsed: consumedTokens,
        observedTokensUsed: observed,
        manualOffsetTokens: consumedTokens - observed,
        observedRequestsCount: previousQuota?.observedRequestsCount ?? 0,
        rebaseApplies: true,
      };
    } else if (draft.quotaUnit==="tokens" && draft.usageAction === "clear") {
      const previousQuota = previous?.usageQuota;
      const observed = previous ? observedGrantTokens(previous) : 0;
      next.usageRebase = undefined;
      next.usageQuota = previousQuota
        ? {
            ...previousQuota,
            effectiveTokensUsed: observed,
            observedTokensUsed: observed,
            manualOffsetTokens: 0,
            rebaseApplies: false,
          }
        : undefined;
    }
    const updated = { ...value };
    if (editingEmail && editingEmail !== email) delete updated[editingEmail];
    updated[email] = next;
    onChange(updated);
    if (onValueUsageInitializationsChange) {
      const next = {...valueUsageInitializations};
      if (editingEmail && editingEmail !== email) delete next[editingEmail];
      delete next[email]; if (initialization) next[email] = initialization;
      onValueUsageInitializationsChange(next);
    }
    if (onUsageEditsChange) {
      const nextEdits: ShareUserUsageEditMap = { ...usageEdits };
      if (editingEmail && editingEmail !== email) delete nextEdits[editingEmail];
      if (draft.quotaUnit === "usdEquivalent") delete nextEdits[email];
      if (draft.quotaUnit==="tokens" && draft.usageAction === "set" && consumedTokens != null) {
        nextEdits[email] = {
          action: "set",
          targetTokens: consumedTokens,
          expectedGrantRevision: previous?.revision,
          expectedAccountCycle: draft.tokenPeriod === "accountReset" ? next.accountQuotaCycle : undefined,
          period: draft.tokenPeriod,
          anchorAtMs: tokenPeriodAnchorAtMs,
          source: usageEdits[editingEmail ?? email]?.source ?? "manual",
        };
      } else if (draft.quotaUnit==="tokens" && draft.usageAction === "clear") {
        nextEdits[email] = {
          action: "clear",
          expectedGrantRevision: previous?.revision,
          expectedAccountCycle: draft.tokenPeriod === "accountReset" ? next.accountQuotaCycle : undefined,
          period: draft.tokenPeriod,
          anchorAtMs: tokenPeriodAnchorAtMs,
        };
      } else if (draft.usageAction === "unchanged") {
        delete nextEdits[email];
      }
      onUsageEditsChange(nextEdits);
    }
    setDraft(null);
  };

  const saveBatchDraft = async () => {
    if (!batchDraft || selectedEditableEmails.size === 0) return;
    const previewGrants = grants.filter((grant) => selectedEditableEmails.has(grant.email));
    if (batchDraft.quotaUnit==="tokens" && (batchDraft.applyTokenLimit || batchDraft.applyConsumedTokens) && previewGrants.some((grant) =>
      (batchDraft.applyTokenLimit ? batchDraft.tokenPeriod : grant.policy.tokenPeriod) === "accountReset")) {
      try {
        const policies = Object.fromEntries(previewGrants.filter((grant) =>
          (batchDraft.applyTokenLimit ? batchDraft.tokenPeriod : grant.policy.tokenPeriod) === "accountReset")
          .map((grant) => [grant.email, { ...grant.policy, tokenPeriod: "accountReset" as const,
            tokenLimit: batchDraft.applyTokenLimit ? batchDraft.tokenLimit.trim() ? Number(batchDraft.tokenLimit) : undefined : grant.policy.tokenLimit }]));
        if (!await confirmAccountPreview(policies, JSON.stringify([batchDraft, [...selectedEditableEmails].sort()]))) return;
      } catch (error) { setBatchError(errorText(error)); return; }
    }
    const parallelLimit = batchDraft.parallelLimit.trim()
      ? Number(batchDraft.parallelLimit)
      : undefined;
    const amount = batchDraft.quotaUnit === "usdEquivalent" ? parseValueLimitInput(batchDraft.tokenLimit) : undefined;
    const valueQuota = amount?.ok ? amount.micros : undefined;
    if (batchDraft.applyTokenLimit && amount && !amount.ok) { setAmountAttempted(true); setBatchError(""); return; }
    if (batchDraft.applyConsumedTokens && (batchDraft.quotaUnit === "usdEquivalent" || previewGrants.some((g) => g.policy.valueQuota))) {
      setBatchError(t("share.userLimit.valueQuota.usageEditUnavailable")); return;
    }
    const tokenLimit = batchDraft.quotaUnit==="tokens" && batchDraft.tokenLimit.trim()
      ? Number(batchDraft.tokenLimit)
      : undefined;
    const expiresAt = batchDraft.expiresAt
      ? new Date(batchDraft.expiresAt).getTime()
      : undefined;
    const anchored = ANCHORED_PERIODS.has(batchDraft.tokenPeriod);
    const tokenPeriodAnchorAtMs = anchored
      ? parseUtcDateTime(batchDraft.tokenPeriodAnchor) : undefined;
    const consumedTokens = batchDraft.consumedTokens.trim()
      ? Number(batchDraft.consumedTokens)
      : undefined;
    if (
      !batchDraft.applyParallelLimit &&
      !batchDraft.applyTokenLimit &&
      !batchDraft.applyConsumedTokens &&
      !batchDraft.applyExpiresAt
    ) {
      return;
    }
    const selectedGrants = grants.filter((grant) => selectedEditableEmails.has(grant.email));
    const usageFloor = selectedGrants.reduce(
      (highest, grant) => Math.max(highest, observedGrantTokens(grant)),
      0,
    );
    const usageInvalid =
      batchDraft.applyConsumedTokens &&
      batchDraft.usageAction === "set" &&
      consumedTokens != null &&
      (!Number.isSafeInteger(consumedTokens) ||
        consumedTokens < 0 ||
        (!(batchDraft.applyTokenLimit ? batchDraft.tokenPeriod === "accountReset" : selectedGrants.every((grant) => grant.policy.tokenPeriod === "accountReset")) && consumedTokens < usageFloor));
    if (
      (batchDraft.applyParallelLimit && parallelLimit != null &&
        (!Number.isInteger(parallelLimit) || parallelLimit < 1)) ||
      (batchDraft.applyTokenLimit && tokenLimit != null &&
        (!Number.isInteger(tokenLimit) || tokenLimit < 1)) ||
      (batchDraft.applyExpiresAt && expiresAt != null && !Number.isFinite(expiresAt)) || (batchDraft.applyTokenLimit && anchored && (
        tokenPeriodAnchorAtMs == null ||
        !Number.isFinite(tokenPeriodAnchorAtMs) ||
        tokenPeriodAnchorAtMs > Math.floor(Date.now() / 60_000) * 60_000
      )) ||
      usageInvalid
    ) {
      setBatchError(
        usageInvalid
          ? consumedTokens != null && consumedTokens < usageFloor
            ? t("share.userLimit.consumedBelowObserved", {
                defaultValue:
                  "已消耗 Token 不能低于当前观测值（{{observed}}）。",
                observed: usageFloor.toLocaleString(),
              })
            : t("share.userLimit.invalidUsage", {
                defaultValue: "已消耗 Token 必须是大于等于 0 的整数。",
              })
          : t("share.userLimit.invalidPolicy", {
              defaultValue: "限制必须为正整数，且时间必须有效。",
            }),
      );
      return;
    }

    const nextGrants = applyShareUserPolicyBatch(value, selectedEditableEmails, batchPatchFor(batchDraft));
    if (batchDraft.applyTokenLimit && valueQuota && !await resolveValuePreview(batchPreview)) return;
    if (batchDraft.applyConsumedTokens && !valueQuota) {
      for (const grant of selectedGrants) {
        const current = nextGrants[grant.email] ?? grant;
        const observed = observedGrantTokens(grant);
        const period = batchDraft.applyTokenLimit
          ? batchDraft.tokenPeriod
          : current.policy.tokenPeriod;
        const anchorAtMs = batchDraft.applyTokenLimit
          ? tokenPeriodAnchorAtMs : current.policy.tokenPeriodAnchorAtMs;
        if (batchDraft.usageAction === "set" && consumedTokens != null) {
          const previousQuota = current.usageQuota;
          nextGrants[grant.email] = {
            ...current,
            usageQuota: {
              period,
              anchorAtMs,
              windowStartsAtMs: previousQuota?.windowStartsAtMs,
              windowEndsAtMs: previousQuota?.windowEndsAtMs,
              effectiveTokensUsed: consumedTokens,
              observedTokensUsed: observed,
              manualOffsetTokens: consumedTokens - observed,
              observedRequestsCount: previousQuota?.observedRequestsCount ?? 0,
              rebaseApplies: true,
            },
          };
        } else if (batchDraft.usageAction === "clear") {
          const previousQuota = current.usageQuota;
          nextGrants[grant.email] = {
            ...current,
            usageRebase: undefined,
            usageQuota: previousQuota
              ? {
                  ...previousQuota,
                  effectiveTokensUsed: observed,
                  observedTokensUsed: observed,
                  manualOffsetTokens: 0,
                  rebaseApplies: false,
                }
              : undefined,
          };
        }
      }
    }
    if (batchDraft.applyTokenLimit && batchDraft.tokenPeriod === "accountReset") {
      for (const email of selectedEditableEmails) {
        const grant = nextGrants[email];
        if (grant) nextGrants[email] = { ...grant, accountQuotaCycle: batchCycle ?? accountPreview?.accountQuotaCycle,
          accountResetCapability: accountPreview?.accountResetCapability };
      }
    }
    onChange(nextGrants);
    if (onUsageEditsChange && (batchDraft.applyTokenLimit || batchDraft.applyConsumedTokens)) {
      const nextEdits: ShareUserUsageEditMap = { ...usageEdits };
      for (const grant of selectedGrants) {
        const email = grant.email.trim().toLowerCase();
        if (nextGrants[email]?.policy.valueQuota || valueQuota) { delete nextEdits[email]; continue; }
        const period = batchDraft.applyTokenLimit ? batchDraft.tokenPeriod : grant.policy.tokenPeriod;
        if (batchDraft.applyConsumedTokens && batchDraft.usageAction === "set" && consumedTokens != null) {
          nextEdits[email] = {
            action: "set",
            targetTokens: consumedTokens,
            expectedGrantRevision: grant.revision,
            expectedAccountCycle: period === "accountReset" ? batchCycle ?? accountPreview?.accountQuotaCycle : undefined,
            period: batchDraft.applyTokenLimit ? batchDraft.tokenPeriod : grant.policy.tokenPeriod,
            anchorAtMs: batchDraft.applyTokenLimit
              ? tokenPeriodAnchorAtMs
              : grant.policy.tokenPeriodAnchorAtMs,
            source: usageEdits[email]?.source ?? "manual",
          };
        } else if (batchDraft.applyConsumedTokens && batchDraft.usageAction === "clear") {
          nextEdits[email] = {
            action: "clear",
            expectedGrantRevision: grant.revision,
            expectedAccountCycle: period === "accountReset" ? batchCycle ?? accountPreview?.accountQuotaCycle : undefined,
            period: batchDraft.applyTokenLimit ? batchDraft.tokenPeriod : grant.policy.tokenPeriod,
            anchorAtMs: batchDraft.applyTokenLimit
              ? tokenPeriodAnchorAtMs
              : grant.policy.tokenPeriodAnchorAtMs,
          };
        } else if (batchDraft.applyTokenLimit && nextEdits[email]) {
          // Preserve an explicit calibration already staged in this draft.
          nextEdits[email] = { ...nextEdits[email], period: batchDraft.tokenPeriod, anchorAtMs: tokenPeriodAnchorAtMs };
        }
      }
      onUsageEditsChange(nextEdits);
    }
    setSelectedEmails(new Set());
    setBatchDraft(null);
    setBatchError("");
    setSelecting(false);
  };

  const unlimited = t("share.unlimited", { defaultValue: "无限" });
  const permanent = t("share.permanent", { defaultValue: "永久" });
  const periodLabels: Record<ShareTokenPeriod, string> = {
    accountReset: t("share.userLimit.accountReset.label"),
    lifetime: t("share.userLimit.periodLifetime", { defaultValue: "累计" }),
    day: t("share.userLimit.periodDay", { defaultValue: "每天" }),
    week: t("share.userLimit.periodWeek", { defaultValue: "自然周" }),
    sevenDays: t("share.userLimit.periodSevenDays", { defaultValue: "每 7 天" }),
    calendarMonth: t("share.userLimit.periodMonth", { defaultValue: "每月" }),
    thirtyDays: t("share.userLimit.periodThirtyDays", { defaultValue: "每 30 天" }),
  };
  const draftAnchorAtMs = draft
    ? parseUtcDateTime(draft.tokenPeriodAnchor)
    : undefined;
  const draftWindow = draft
    ? fixedPeriodWindow(draft.tokenPeriod, draftAnchorAtMs)
    : undefined;
  const batchAnchorAtMs = batchDraft
    ? parseUtcDateTime(batchDraft.tokenPeriodAnchor)
    : undefined;
  const batchWindow = batchDraft
    ? fixedPeriodWindow(batchDraft.tokenPeriod, batchAnchorAtMs)
    : undefined;
  // Server-derived window and standing correction for the grant being edited.
  const editingQuota =
    editingEmail && value[editingEmail]?.usageQuota?.rebaseApplies
      ? value[editingEmail].usageQuota
      : undefined;

  return (
    <div className="space-y-2 md:col-span-2">
      {valueStatus.diagnostics ? <details className="rounded-md border p-2 text-xs text-muted-foreground"><summary>{t("share.userLimit.valueQuota.diagnostics")}</summary>
        <p className="mt-2">{t("share.userLimit.valueQuota.diagnosticsSummary",{attempts:valueStatus.diagnostics.attempts,pending:valueStatus.diagnostics.pendingAttempts,
          unavailable:valueStatus.diagnostics.unavailableAttempts,gaps:valueStatus.diagnostics.coverageGaps,checkpoints:valueStatus.diagnostics.dailyCheckpoints})}</p>
        {valueStatus.diagnostics.capacityWarning ? <p className="mt-1 text-destructive" role="status">{t("share.userLimit.valueQuota.capacityWarning")}</p>:null}
      </details>:null}
      <div className="text-xs text-muted-foreground" role="status">
        {accountReady ? t("share.userLimit.accountReset.ready") : t("share.userLimit.accountReset.unavailable", { reason: accountPreview?.accountResetCapability.reason || t("share.userLimit.accountReset.pending") })}
        {accountPreview?.accountQuotaCycle ? <div>
          {accountPreview.accountQuotaCycle.bucketId} · {formatUtcWindow(accountPreview.accountQuotaCycle.startsAtMs)} → {accountPreview.accountQuotaCycle.endsAtMs ? formatUtcWindow(accountPreview.accountQuotaCycle.endsAtMs) : t("share.userLimit.accountReset.noEnd")}
          {" · "}{accountPreview.accountQuotaCycle.basis} · {t("share.userLimit.accountReset.observed")} {formatUtcWindow(accountPreview.accountQuotaCycle.observedAtMs)}
        </div> : null}
        <div>{t("share.userLimit.accountReset.caveat")}</div>
        {accountPreview?.ledgerHealth?.capacityWarning ? <div role="status">{t("share.userLimit.accountReset.unavailable", {
          reason: `quota_ledger_capacity_warning (${accountPreview.ledgerHealth.revisionCount}/${accountPreview.ledgerHealth.admissionLimit})`,
        })}</div> : null}
      </div>
      <div className="flex items-center justify-between gap-3">
        <Label>{t("share.userLimit.title", { defaultValue: "授权用户与配额" })}</Label>
        <div className="flex flex-wrap items-center justify-end gap-2">
          {selecting ? (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={disabled}
              onClick={exitSelecting}
            >
              {t("common.cancel", { defaultValue: "取消" })}
            </Button>
          ) : null}
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={
              disabled ||
              selectableEmails.length === 0 ||
              (selecting && selectedEditableEmails.size === 0)
            }
            onClick={openBatchEdit}
          >
            <Pencil className="mr-1.5 h-4 w-4" />
            {selecting
              ? t("share.userLimit.batchEditSelected", {
                  defaultValue: "编辑已选（{{count}}）",
                  count: selectedEditableEmails.size,
                })
              : t("share.userLimit.batchEdit", {
                  defaultValue: "批量编辑",
                })}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={disabled}
            onClick={openAdd}
          >
            <Plus className="mr-1.5 h-4 w-4" />
            {t("share.userLimit.add", { defaultValue: "添加用户" })}
          </Button>
        </div>
      </div>

      <div className="w-full overflow-hidden rounded-md border border-border-default [&_.relative.w-full.overflow-auto]:overflow-hidden">
        <Table className="w-full table-fixed">
          <TableHeader>
            <TableRow>
              {selecting ? (
                <TableHead className="h-9 w-10 px-3">
                  <Checkbox
                    checked={allSelected ? true : someSelected ? "indeterminate" : false}
                    disabled={disabled || selectableEmails.length === 0}
                    aria-label={t("share.userLimit.selectAll", {
                      defaultValue: "选择全部可编辑用户",
                    })}
                    onCheckedChange={(checked) =>
                      setSelectedEmails(new Set(checked === true ? selectableEmails : []))
                    }
                  />
                </TableHead>
              ) : null}
              <TableHead className="h-9 w-[28%] px-3">Email</TableHead>
              <TableHead className="h-9 w-[12%] px-3">{t("share.parallelLimit", { defaultValue: "并发" })}</TableHead>
              <TableHead className="h-9 w-[18%] px-3">Token</TableHead>
              <TableHead className="h-9 w-[22%] px-3">
                {t("share.userLimit.consumedTokens", {
                  defaultValue: "已消耗 Token（当前周期）",
                })}
              </TableHead>
              <TableHead className="h-9 w-[12%] px-3">{t("share.expiration", { defaultValue: "到期" })}</TableHead>
              <TableHead className="h-9 w-20 px-3" />
            </TableRow>
          </TableHeader>
          <TableBody>
            {grants.map((grant) => {
              const malformed = !!grant.valueQuotaMalformed;
              const stored = grant;
              const enforcedPolicy = grant.policy;
              // A waiting USD policy is shown under the enforced one, which keeps
              // its own usage — USD → USD changes included.
              const scheduled = !malformed && grant.pendingValueQuota?.policy?.valueQuota ? grant.pendingValueQuota : undefined;
              const usdLimit = malformed ? undefined : grant.policy.valueQuota?.limitUsdMicros;
              const rowStatus = usdLimit ? matchingValueStatus(valueStatus.items, grant) : undefined;
              const emailKey = grant.email.trim().toLowerCase();
              // Corrections need a live USD window; only a Token-enforced user waiting for USD has none.
              const correctable = !!rowStatus && (!!usdLimit || rowStatus.meteringStatus !== "scheduled") && !!shareId && configRevision != null
                && !disabled && grant.manager !== "routerShareMarket" && !protectedEmails?.has(grant.email);
              return (
              <TableRow key={grant.email}>
                {selecting ? (
                  <TableCell className="w-10 px-3 py-2">
                    {malformed ? null : <Checkbox
                      checked={selectedEmails.has(grant.email)}
                      disabled={disabled || protectedEmails?.has(grant.email)}
                      aria-label={t("share.userLimit.selectUser", {
                        defaultValue: "选择 {{email}}",
                        email: grant.email,
                      })}
                      onCheckedChange={(checked) => {
                        setSelectedEmails((current) => {
                          const next = new Set(current);
                          if (checked === true) next.add(grant.email);
                          else next.delete(grant.email);
                          return next;
                        });
                      }}
                    />}
                  </TableCell>
                ) : null}
                <TableCell className="px-3 py-2 align-top">
                  <div className="flex min-w-0 flex-wrap items-start gap-2">
                    <span className="min-w-0 whitespace-normal break-all">{grant.email}</span>
                    {grant.role === "owner" ? <Badge variant="secondary">Owner</Badge> : null}
                    {grant.manager === "routerShareMarket" && grant.active !== false ? (
                      <Badge variant="secondary">Share Market</Badge>
                    ) : null}
                  </div>
                </TableCell>
                <TableCell className="px-3 py-2">{displayLimit(grant.policy.parallelLimit, unlimited)}</TableCell>
                <TableCell className="px-3 py-2 align-top">
                  {malformed ? "—" : <>{usdLimit ? valueLimitLabel(usdLimit) : displayLimit(enforcedPolicy.tokenLimit, unlimited)} · {periodLabels[enforcedPolicy.tokenPeriod]}</>}
                  {scheduled ? <ShareValueScheduledLine limitMicros={scheduled.policy.valueQuota!.limitUsdMicros} effectiveFromMs={scheduled.effectiveFromMs} /> : null}
                </TableCell>
                <TableCell className="px-3 py-2 align-top">
                  {malformed ? <p className="whitespace-normal text-[11px] text-destructive" role="status">{t("share.userLimit.valueQuota.malformed")}</p> : usdLimit ? <>
                    <ShareValueUsageCell status={rowStatus} limitMicros={usdLimit} stale={valueStatus.stale}
                      onReconcile={correctable ? () => setCorrectionEmail(emailKey) : undefined} />
                    {correctable && rowStatus ? <ShareValueUsageCorrection key={grant.email} shareId={shareId!} revision={configRevision!} grant={grant} status={rowStatus}
                      onSaved={valueStatus.merge} open={!draft && correctionEmail === emailKey} onOpenChange={(open) => setCorrectionEmail(open ? emailKey : null)} /> : null}
                  </> : <div className="whitespace-normal break-all font-mono text-xs">
                    {enforcedPolicy.tokenPeriod === "accountReset" && !stored.usageQuota ? t("share.userLimit.accountReset.pending") : currentGrantTokens(stored, usageEdits).toLocaleString()}
                  </div>}
                  {grant.usageRebase && !usdLimit && !malformed ? (
                    <div className="text-[11px] text-muted-foreground">
                      {t("share.userLimit.rebaseTarget", {
                        defaultValue: "基线 {{value}}",
                        value: grant.usageRebase.targetTokens.toLocaleString(),
                      })}
                    </div>
                  ) : null}
                  {grant.manager === "routerShareMarket" && grant.active !== false ? (
                    <div className="text-[11px] text-muted-foreground">
                      {t("share.userLimit.readOnly", {
                        defaultValue: "Share Market 管理，只读",
                      })}
                    </div>
                  ) : null}
                </TableCell>
                <TableCell className="px-3 py-2">{displayExpiry(grant.policy.expiresAt, permanent)}</TableCell>
                <TableCell className="px-3 py-2">
                  <div className="flex justify-end gap-1">
                    {!protectedEmails?.has(grant.email) && !malformed ? (
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon"
                        disabled={disabled}
                        onClick={() => openEdit(grant)}
                        title={t("common.edit", { defaultValue: "编辑" })}
                        aria-label={t("common.edit", { defaultValue: "编辑" })}
                      >
                        <Pencil className="h-4 w-4" />
                      </Button>
                    ) : null}
                    {grant.role !== "owner" && !protectedEmails?.has(grant.email) ? (
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon"
                        disabled={disabled}
                        onClick={() => {
                          const updated = { ...value };
                          delete updated[grant.email];
                          onChange(updated);
                          if (onValueUsageInitializationsChange) {
                            const next = {...valueUsageInitializations}; delete next[grant.email]; onValueUsageInitializationsChange(next);
                          }
                          if (onUsageEditsChange) {
                            const nextEdits = { ...usageEdits };
                            delete nextEdits[grant.email];
                            onUsageEditsChange(nextEdits);
                          }
                        }}
                        title={t("common.delete", { defaultValue: "删除" })}
                        aria-label={t("common.delete", { defaultValue: "删除" })}
                      >
                        <Trash2 className="h-4 w-4 text-destructive" />
                      </Button>
                    ) : null}
                  </div>
                </TableCell>
              </TableRow>
            );})}
          </TableBody>
        </Table>
      </div>

      <Dialog open={draft != null} onOpenChange={(open) => !open && setDraft(null)}>
        {/* Provider editing uses a z-[60] FullScreenPanel. */}
        <DialogContent className="max-w-xl" zIndex="top">
          <DialogHeader>
            <DialogTitle>
              {editingEmail
                ? t("share.userLimit.edit", { defaultValue: "编辑用户限制" })
                : t("share.userLimit.add", { defaultValue: "添加用户" })}
            </DialogTitle>
          </DialogHeader>
          {draft ? (
            <div className="grid gap-4 overflow-y-auto px-6 py-5 sm:grid-cols-2">
              <div className="space-y-2 sm:col-span-2">
                <Label htmlFor="share-user-email">Email</Label>
                <Input id="share-user-email" type="email" disabled={editingEmail != null} value={draft.email} onChange={(event) => setDraft({ ...draft, email: event.target.value })} />
              </div>
              <div className="space-y-2">
                <Label htmlFor="share-user-parallel">{t("share.parallelLimit", { defaultValue: "并发限额" })}</Label>
                <Input id="share-user-parallel" type="number" min={1} placeholder={unlimited} value={draft.parallelLimit} onChange={(event) => setDraft({ ...draft, parallelLimit: event.target.value })} />
              </div>
              <div className="space-y-2">
                <Label id="share-user-unit-label">{t("share.userLimit.valueQuota.unit")}</Label>
                <ShareValueUnitToggle labelledBy="share-user-unit-label" readinessId="share-user-usd-readiness" value={draft.quotaUnit} usdDisabled={readiness.unavailable}
                  onChange={(unit) => {
                    if (unit === draft.quotaUnit) return;
                    const unitDrafts = {...draft.unitDrafts, [draft.quotaUnit]: {
                      tokenLimit: draft.tokenLimit, consumedTokens: draft.consumedTokens, usageAction: draft.usageAction,
                    }};
                    setAmountAttempted(false); setDraft({...draft, quotaUnit: unit, unitDrafts,
                      ...(unitDrafts[unit] ?? {tokenLimit: "", usageAction: "unchanged", consumedTokens: ""})});
                  }} />
                <ShareValueReadiness state={readiness} id="share-user-usd-readiness" />
              </div>
              <div className="space-y-2 sm:col-span-2">
                <ShareValueLimitField id="share-user-token" unit={draft.quotaUnit} value={draft.tokenLimit}
                  label={draft.quotaUnit === "usdEquivalent" ? t("share.userLimit.valueQuota.limit") : t("share.tokenLimit", { defaultValue: "Token 限额" })}
                  error={limitError(draft.quotaUnit, draft.tokenLimit)} onChange={(tokenLimit) => setDraft({ ...draft, tokenLimit })} />
                {draft.quotaUnit === "usdEquivalent" ? <ShareValuePreviewLine state={singlePreview} /> : null}
              </div>
              <div className={draft.quotaUnit==="usdEquivalent"?"hidden":"space-y-2 sm:col-span-2"}>
                <Label htmlFor="share-user-consumed-tokens">
                  {t("share.userLimit.consumedTokens", {
                    defaultValue: "已消耗 Token（当前周期）",
                  })}
                </Label>
                <div className="flex items-center gap-2">
                  <Input
                    id="share-user-consumed-tokens"
                    type="number"
                    min={0}
                    step={1}
                    value={draft.consumedTokens}
                    placeholder="0"
                    onChange={(event) =>
                      setDraft({
                        ...draft,
                        consumedTokens: event.target.value,
                        usageAction: "set",
                      })
                    }
                  />
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    disabled={
                      !editingEmail ||
                      (!value[editingEmail]?.usageRebase &&
                        usageEdits[editingEmail]?.action !== "set")
                    }
                    onClick={() =>
                      setDraft({
                        ...draft,
                        consumedTokens: "",
                        usageAction: "clear",
                      })
                    }
                  >
                    {t("share.userLimit.clearRebase", {
                      defaultValue: "清除重基线",
                    })}
                  </Button>
                </div>
                {editingEmail && value[editingEmail] ? (
                  <>
                    <p className="text-xs text-muted-foreground">
                      {t("share.userLimit.consumedHint", {
                        defaultValue:
                          "当前有效 {{effective}}；保存后以该值为基线，并继续累加新请求。当前观测值 {{observed}}。",
                        effective: currentGrantTokens(value[editingEmail]).toLocaleString(),
                        observed: observedGrantTokens(value[editingEmail]).toLocaleString(),
                      })}
                    </p>
                    {editingQuota ? (
                      <p className="text-xs text-muted-foreground">
                        {t("share.userLimit.savedQuotaHint", {
                          defaultValue:
                            "服务端统计周期：{{start}} 至 {{end}}；手工修正 {{offset}}。",
                          start: editingQuota.windowStartsAtMs
                            ? formatUtcWindow(editingQuota.windowStartsAtMs)
                            : t("share.userLimit.periodLifetime", {
                                defaultValue: "累计",
                              }),
                          end: editingQuota.windowEndsAtMs
                            ? formatUtcWindow(editingQuota.windowEndsAtMs)
                            : "—",
                          offset: editingQuota.manualOffsetTokens.toLocaleString(),
                        })}
                      </p>
                    ) : null}
                  </>
                ) : (
                  <p className="text-xs text-muted-foreground">
                    {t("share.userLimit.newConsumedHint", {
                      defaultValue: "可填写 0；留空表示不创建手工重基线。",
                    })}
                  </p>
                )}
              </div>
              {draft.quotaUnit === "usdEquivalent" ? <div className="space-y-2 sm:col-span-2">
                <span id="share-user-value-label" className="text-sm font-medium">{t("share.userLimit.valueQuota.consumed")}</span>
                {!savedGrant ? <>
                  <Input id="share-user-initial-value" inputMode="decimal" autoComplete="off" placeholder="0"
                    aria-labelledby="share-user-value-label"
                    disabled={!canInitializeValue} value={draft.consumedValue} onChange={event => setDraft({...draft, consumedValue: event.target.value})} />
                  <p className="text-xs text-muted-foreground">{t(canInitializeValue ? "share.userLimit.valueQuota.initialHint" : "share.userLimit.valueQuota.initialUnavailable")}</p>
                  {draft.consumedValue.trim() && parseValueUsage(draft.consumedValue) !== "0" ?
                    <label className="grid gap-1 text-xs text-muted-foreground">{t("share.userLimit.valueQuota.reason")}
                      <Input maxLength={1000} value={draft.valueReason} onChange={event => setDraft({...draft, valueReason: event.target.value})} />
                    </label> : null}
                </> : <>
                  <p id="share-user-initial-value" className="font-mono text-sm">{valueUsageLabel(editingValueStatus?.usedPicos)}</p>
                  <p className="text-xs text-muted-foreground">{t(!editingValueStatus && (savedGrant.pendingValueQuota || (initialPreviewRow && valueRowKind(initialPreviewRow) === "scheduled"))
                    ? "share.userLimit.valueQuota.scheduledUsage" : valueStatus.stale ? "share.userLimit.valueQuota.stale" : valueStateKey(editingValueStatus))}</p>
                  {editingValueStatus && editingValueStatus.meteringStatus !== "scheduled" && shareId && configRevision != null && !disabled && !protectedEmails?.has(editedEmail) ?
                    <ShareValueUsageCorrection key={`${editedEmail}:${configRevision}`} shareId={shareId} revision={configRevision}
                      grant={savedGrant} status={editingValueStatus} onSaved={valueStatus.merge}
                      open={correctionEmail === editedEmail} onOpenChange={open => setCorrectionEmail(open ? editedEmail : null)} /> : null}
                </>}
              </div> : null}
              <div className="space-y-2">
                <Label id="share-user-period-label">{t("share.userLimit.period", { defaultValue: "Token 周期" })}</Label>
                <Select value={draft.tokenPeriod} onValueChange={(tokenPeriod: ShareTokenPeriod) => setDraft({
                  ...draft,
                  tokenPeriod,
                  tokenPeriodAnchor: ANCHORED_PERIODS.has(tokenPeriod)
                    ? (draft.tokenPeriodAnchor || toUtcDateTime())
                    : "",
                })}>
                  <SelectTrigger><SelectValue /></SelectTrigger>
                  <SelectContent className="z-[130]">
                    {(Object.keys(periodLabels) as ShareTokenPeriod[]).map((period) => <SelectItem key={period} value={period} disabled={period === "accountReset" && !accountReady}>{periodLabels[period]}</SelectItem>)}
                  </SelectContent>
                </Select>
              </div>
              {ANCHORED_PERIODS.has(draft.tokenPeriod) ? (
                <div className="space-y-2 sm:col-span-2">
                  <Label htmlFor="share-user-period-anchor">
                    {t("share.userLimit.anchor", { defaultValue: "周期开始时间（UTC）" })}
                  </Label>
                  <Input
                    id="share-user-period-anchor"
                    type="datetime-local"
                    step={60}
                    min={toUtcDateTime(
                      Date.now() -
                        (fixedPeriodDurationMs(draft.tokenPeriod) ?? 0),
                    )}
                    max={toUtcDateTime()}
                    value={draft.tokenPeriodAnchor}
                    onChange={(event) => setDraft({ ...draft, tokenPeriodAnchor: event.target.value })}
                  />
                  <p className="text-xs text-muted-foreground">
                    {t("share.userLimit.anchorHint", { defaultValue: "从该 UTC 时间起每隔固定天数重置，不可晚于当前时间。" })}
                  </p>
                  {draftWindow ? (
                    <p className="text-xs text-muted-foreground">
                      {t("share.userLimit.currentWindow", {
                        defaultValue: "当前周期：{{start}} 至 {{end}}",
                        start: formatUtcWindow(draftWindow.start),
                        end: formatUtcWindow(draftWindow.end),
                      })}
                    </p>
                  ) : null}
                </div>
              ) : null}
              <div className="space-y-2">
                <Label htmlFor="share-user-expiry">{t("share.expiration", { defaultValue: "到期时间" })}</Label>
                <Input id="share-user-expiry" type="datetime-local" value={draft.expiresAt} onChange={(event) => setDraft({ ...draft, expiresAt: event.target.value })} />
              </div>
              {draftError ? <p className="text-sm text-destructive sm:col-span-2" role="alert">{draftError}</p> : null}
            </div>
          ) : null}
          {draft?.tokenPeriod === "accountReset" && previewIntent && accountPreview ? <div className="text-xs" role="status">
            {t("share.userLimit.accountReset.preview")}
            {accountPreview.rows.map((row) => <div key={row.email}>{row.email}: {row.currentTokens ?? t("share.userLimit.accountReset.pending")} → {row.expectedTokens ?? t("share.userLimit.accountReset.notReady")}{row.wouldExceed ? ` · ${t("share.userLimit.accountReset.exceeded")}` : ""}</div>)}
          </div> : null}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => setDraft(null)}>{t("common.cancel", { defaultValue: "取消" })}</Button>
            <Button type="button" disabled={previewBusy || applying} aria-busy={applying || undefined} onClick={() => void saveDraft()}>
              {draft?.quotaUnit === "usdEquivalent" && needsConfirmation(singlePreview) ? t("share.userLimit.valueQuota.confirmApply") : t("common.apply", { defaultValue: "应用" })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={batchDraft != null} onOpenChange={(open) => {
        if (!open) {
          setBatchDraft(null);
          setBatchError("");
        }
      }}>
        <DialogContent className="max-w-xl" zIndex="top">
          <DialogHeader>
            <DialogTitle>
              {t("share.userLimit.batchTitle", { defaultValue: "批量编辑用户限制" })}
            </DialogTitle>
          </DialogHeader>
          {batchDraft ? (
            <div className="grid gap-4 overflow-y-auto px-6 py-5 sm:grid-cols-2">
              <p className="text-sm text-muted-foreground sm:col-span-2">
                {t("share.userLimit.batchHint", {
                  defaultValue: "已选择 {{count}} 个用户。仅覆盖勾选的参数，其他参数保持不变；空限额表示无限，空到期时间表示永久。",
                  count: selectedEditableEmails.size,
                })}
              </p>
              <div className="space-y-2">
                <div className="flex items-center gap-2">
                  <Checkbox
                    id="share-user-batch-parallel-enabled"
                    checked={batchDraft.applyParallelLimit}
                    onCheckedChange={(checked) => setBatchDraft({
                      ...batchDraft,
                      applyParallelLimit: checked === true,
                    })}
                  />
                  <Label htmlFor="share-user-batch-parallel-enabled">
                    {t("share.parallelLimit", { defaultValue: "并发限额" })}
                  </Label>
                </div>
                <Input
                  type="number"
                  min={1}
                  disabled={!batchDraft.applyParallelLimit}
                  placeholder={unlimited}
                  value={batchDraft.parallelLimit}
                  onChange={(event) => setBatchDraft({
                    ...batchDraft,
                    parallelLimit: event.target.value,
                  })}
                />
              </div>
              <div className="space-y-2">
                <div className="flex items-center gap-2">
                  <Checkbox
                    id="share-user-batch-token-enabled"
                    checked={batchDraft.applyTokenLimit}
                    onCheckedChange={(checked) => setBatchDraft({
                      ...batchDraft,
                      applyTokenLimit: checked === true,
                    })}
                  />
                  <Label htmlFor="share-user-batch-token-enabled">
                    {batchDraft.quotaUnit === "usdEquivalent" ? t("share.userLimit.valueQuota.limit") : t("share.tokenLimit", { defaultValue: "Token 限额" })}
                  </Label>
                </div>
                <ShareValueUnitToggle disabled={!batchDraft.applyTokenLimit} readinessId="share-batch-usd-readiness" value={batchDraft.quotaUnit} usdDisabled={readiness.unavailable}
                  onChange={(unit) => { if (unit === batchDraft.quotaUnit) return; setAmountAttempted(false); setBatchDraft({ ...batchDraft, quotaUnit: unit, tokenLimit: "", applyConsumedTokens: false }); }} />
                {batchDraft.applyTokenLimit ? <ShareValueReadiness state={readiness} id="share-batch-usd-readiness" /> : null}
                <ShareValueLimitField id="share-user-batch-token" labelHidden unit={batchDraft.quotaUnit} value={batchDraft.tokenLimit} disabled={!batchDraft.applyTokenLimit}
                  label={batchDraft.quotaUnit === "usdEquivalent" ? t("share.userLimit.valueQuota.limit") : t("share.tokenLimit", { defaultValue: "Token 限额" })}
                  error={batchDraft.applyTokenLimit ? limitError(batchDraft.quotaUnit, batchDraft.tokenLimit) : undefined}
                  onChange={(tokenLimit) => setBatchDraft({ ...batchDraft, tokenLimit })} />
                {batchDraft.applyTokenLimit && batchDraft.quotaUnit === "usdEquivalent" ? <ShareValuePreviewLine state={batchPreview} /> : null}
              </div>
              <div className="space-y-2">
                <Label>{t("share.userLimit.period", { defaultValue: "Token 周期" })}</Label>
                <Select
                  value={batchDraft.tokenPeriod}
                  onValueChange={(tokenPeriod: ShareTokenPeriod) => setBatchDraft({
                    ...batchDraft,
                    applyTokenLimit: true,
                    tokenPeriod,
                    tokenPeriodAnchor: ANCHORED_PERIODS.has(tokenPeriod)
                      ? (batchDraft.tokenPeriodAnchor || toUtcDateTime())
                      : "",
                  })}
                >
                  <SelectTrigger><SelectValue /></SelectTrigger>
                  <SelectContent className="z-[130]">
                    {(Object.keys(periodLabels) as ShareTokenPeriod[]).map((period) => (
                      <SelectItem key={period} value={period} disabled={period === "accountReset" && !accountReady}>{periodLabels[period]}</SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="space-y-2 sm:col-span-2">
                <div className="flex items-center gap-2">
                  <Checkbox
                    id="share-user-batch-consumed-enabled"
                    checked={batchDraft.applyConsumedTokens}
                    onCheckedChange={(checked) => setBatchDraft({
                      ...batchDraft,
                      applyConsumedTokens: checked === true,
                    })}
                  />
                  <Label htmlFor="share-user-batch-consumed-enabled">
                    {t("share.userLimit.consumedTokens", {
                      defaultValue: "已消耗 Token（当前周期）",
                    })}
                  </Label>
                </div>
                <div className="flex items-center gap-2">
                  <Input
                    type="number"
                    min={0}
                    step={1}
                    disabled={!batchDraft.applyConsumedTokens}
                    placeholder="0"
                    value={batchDraft.consumedTokens}
                    onChange={(event) => setBatchDraft({
                      ...batchDraft,
                      consumedTokens: event.target.value,
                      usageAction: "set",
                    })}
                  />
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    disabled={!batchDraft.applyConsumedTokens}
                    onClick={() => setBatchDraft({
                      ...batchDraft,
                      consumedTokens: "",
                      usageAction: "clear",
                    })}
                  >
                    {t("share.userLimit.clearRebase", {
                      defaultValue: "清除重基线",
                    })}
                  </Button>
                </div>
                <p className="text-xs text-muted-foreground">
                  {t("share.userLimit.batchConsumedHint", {
                    defaultValue: "勾选后会把该值应用到所有已选用户。留空并保存表示不创建手工重基线。",
                  })}
                </p>
              </div>
              {batchDraft.applyTokenLimit && ANCHORED_PERIODS.has(batchDraft.tokenPeriod) ? (
                <div className="space-y-2 sm:col-span-2">
                  <Label htmlFor="share-user-batch-period-anchor">
                    {t("share.userLimit.anchor", { defaultValue: "周期开始时间（UTC）" })}
                  </Label>
                  <Input
                    id="share-user-batch-period-anchor"
                    type="datetime-local"
                    step={60}
                    min={toUtcDateTime(
                      Date.now() -
                        (fixedPeriodDurationMs(batchDraft.tokenPeriod) ?? 0),
                    )}
                    max={toUtcDateTime()}
                    value={batchDraft.tokenPeriodAnchor}
                    onChange={(event) => setBatchDraft({
                      ...batchDraft,
                      tokenPeriodAnchor: event.target.value,
                    })}
                  />
                  <p className="text-xs text-muted-foreground">
                    {t("share.userLimit.anchorHint", {
                      defaultValue: "从该 UTC 时间起每隔固定天数重置，不可晚于当前时间。",
                    })}
                  </p>
                  {batchWindow ? (
                    <p className="text-xs text-muted-foreground">
                      {t("share.userLimit.currentWindow", {
                        defaultValue: "当前周期：{{start}} 至 {{end}}",
                        start: formatUtcWindow(batchWindow.start),
                        end: formatUtcWindow(batchWindow.end),
                      })}
                    </p>
                  ) : null}
                </div>
              ) : null}
              <div className="space-y-2 sm:col-span-2">
                <div className="flex items-center gap-2">
                  <Checkbox
                    id="share-user-batch-expiry-enabled"
                    checked={batchDraft.applyExpiresAt}
                    onCheckedChange={(checked) => setBatchDraft({
                      ...batchDraft,
                      applyExpiresAt: checked === true,
                    })}
                  />
                  <Label htmlFor="share-user-batch-expiry-enabled">
                    {t("share.expiration", { defaultValue: "到期时间" })}
                  </Label>
                </div>
                <Input
                  type="datetime-local"
                  disabled={!batchDraft.applyExpiresAt}
                  value={batchDraft.expiresAt}
                  onChange={(event) => setBatchDraft({
                    ...batchDraft,
                    expiresAt: event.target.value,
                  })}
                />
              </div>
              {batchError ? (
                <p className="text-sm text-destructive sm:col-span-2">{batchError}</p>
              ) : null}
            </div>
          ) : null}
          {batchDraft && previewIntent && accountPreview ? <div className="text-xs" role="status">
            {t("share.userLimit.accountReset.preview")}
            {accountPreview.rows.map((row) => <div key={row.email}>{row.email}: {row.expectedTokens ?? t("share.userLimit.accountReset.notReady")}{row.wouldExceed ? ` · ${t("share.userLimit.accountReset.exceeded")}` : ""}</div>)}
          </div> : null}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => {
              setBatchDraft(null);
              setBatchError("");
            }}>
              {t("common.cancel", { defaultValue: "取消" })}
            </Button>
            <Button
              type="button"
              disabled={previewBusy || applying || (batchDraft != null &&
                !batchDraft.applyParallelLimit &&
                !batchDraft.applyTokenLimit &&
                !batchDraft.applyConsumedTokens &&
                !batchDraft.applyExpiresAt)}
              onClick={saveBatchDraft}
            >
              {batchDraft?.applyTokenLimit && batchDraft.quotaUnit === "usdEquivalent" && needsConfirmation(batchPreview)
                ? t("share.userLimit.valueQuota.confirmApply")
                : t("share.userLimit.batchApply", {
                  defaultValue: "应用到 {{count}} 个用户",
                  count: selectedEditableEmails.size,
                })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
