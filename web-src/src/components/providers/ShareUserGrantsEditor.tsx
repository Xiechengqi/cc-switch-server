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
import { isValidShareEmail, stageGrantPolicy, sharePolicyKey } from "@/utils/shareFormUtils";
import { applyShareUserPolicyBatch, patchedSharePolicy, type ShareUserPolicyBatchPatch } from "./share-user-policy-batch";
import { ShareValueUsageCorrection } from "./ShareValueUsageCorrection";
import {
  ShareValueResourceToggle, ShareValueLimitField, ShareValuePreviewLine, ShareValueReadiness, ShareValueScheduledLine, ShareValueUnitToggle, ShareValueUsageCell,
} from "./ShareValueReadiness";
import { useShareValuePreview, useShareValueReadiness, useShareValueStatus } from "./shareValueHooks";
import { ShareAccountResetStatus } from "./ShareAccountResetStatus";
import { ShareSubheading } from "./ShareSubheading";
import {
  accountResetCodeInText, accountResetInUse, accountResetReasonKey, accountResetSelectable, effectiveAccountResetCapability,
} from "@/lib/share-account-reset";
import { formatShareTokens } from "@/utils/shareUtils";
import {
  quotaConversionNeeded, quotaConversionInput, matchingValueStatus, parseValueLimitInput, valueLimitErrorKey, valueLimitInput, valueLimitLabel, valuePreviewBlocker,
  valuePreviewNeedsConfirmation, valueUsageInput, valueUsageLabel, valueStateKey, parseValueUsage, parseInitialValueUsage, valueRowKind, type ValuePreview,
} from "@/lib/share-value-quota";

type PolicyDraft = {
  quotaUnit: "tokens" | "usdEquivalent";
  resourceMetering: boolean;
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

type BatchPolicyDraft = Omit<PolicyDraft, "email" | "consumedValue" | "valueReason"> & {
  calibrations?: Record<string, {amount: string; reason: string}>;
  calibrationDrafts?: Partial<Record<"tokens" | "usdEquivalent", Record<string,{amount:string;reason:string}>>>;
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
  /** Public Share: rows are personal limits, not who may call it. */
  freeAccess?: boolean;
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
    resourceMetering: policy.valueQuota?.tariffPolicyId === "api-equivalent-v2",
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
    resourceMetering: draft.resourceMetering,
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

function displayExpiry(value: number | undefined, permanent: string, language?: string) {
  return value == null ? permanent : new Date(value).toLocaleString(language || undefined);
}

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function anchorInvalid(period: ShareTokenPeriod, anchorAtMs: number | undefined) {
  return ANCHORED_PERIODS.has(period) && (anchorAtMs == null || !Number.isFinite(anchorAtMs)
    || anchorAtMs > Math.floor(Date.now() / 60_000) * 60_000);
}

/** The policy a valid USD draft saves; preview and save share it so intents match exactly. */
function usdPolicyFor(draft: Pick<PolicyDraft, "resourceMetering" | "parallelLimit" | "tokenLimit" | "tokenPeriod" | "tokenPeriodAnchor" | "expiresAt">): ShareUserPolicy | undefined {
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
    valueQuota: { limitUsdMicros: amount.micros, tariffPolicyId: draft.resourceMetering ? "api-equivalent-v2" : "api-equivalent-v1" },
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
      ...(amount?.ok ? { valueQuota: { limitUsdMicros: amount.micros, tariffPolicyId: draft.resourceMetering ? "api-equivalent-v2" as const : "api-equivalent-v1" as const } } : {}),
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
  freeAccess = false,
  onChange,
}: ShareUserGrantsEditorProps) {
  const { t, i18n } = useTranslation();
  const tokens = (value: number | null | undefined) => formatShareTokens(value, i18n.language);
  const [accountPreview, setAccountPreview] = useState<AccountResetPreview>();
  const [previewIntent, setPreviewIntent] = useState("");
  const [previewBusy, setPreviewBusy] = useState(false);
  const [draftCycle, setDraftCycle] = useState<ShareUserGrant["accountQuotaCycle"]>();
  const [batchCycle, setBatchCycle] = useState<ShareUserGrant["accountQuotaCycle"]>();
  // Asked on mount and again whenever a quota dialog opens, so the period
  // choice never rests on an answer from before the dialog was opened.
  const [accountAttempt, setAccountAttempt] = useState(0);
  const [accountLoadFailed, setAccountLoadFailed] = useState(false);
  const shareSaved = !!shareId && configRevision != null;
  // Background reads and policy previews share one sequence: only the latest
  // request may write the preview, and a background read never replaces the
  // policy preview an "Apply" is waiting to confirm.
  const accountRequestSeq = useRef(0);
  const previewIntentRef = useRef(previewIntent);
  previewIntentRef.current = previewIntent;
  useEffect(() => {
    let cancelled = false;
    if (shareId && configRevision != null) {
      const seq = ++accountRequestSeq.current;
      const current = () => !cancelled && seq === accountRequestSeq.current && !previewIntentRef.current;
      void shareApi.previewAccountReset(shareId, configRevision).then((result) => {
        if (current()) { setAccountPreview(result); setAccountLoadFailed(false); }
      }).catch(() => {
        // A failed read keeps the last preview; it only says the read failed.
        if (current()) setAccountLoadFailed(true);
      });
    }
    return () => { cancelled = true; };
  }, [shareId, configRevision, accountAttempt]);
  const refreshAccountReset = () => { if (shareSaved) setAccountAttempt((n) => n + 1); };
  const accountState = effectiveAccountResetCapability(accountPreview?.accountResetCapability, accountPreview?.accountQuotaCycle);
  const accountReady = accountState?.syncStatus === "ready";
  const accountSelectable = accountResetSelectable(accountState);
  /** A preview or save refusal in words; the code itself is diagnostics only. */
  const accountResetError = (code?: string | null, email?: string) => {
    const reason = t(accountResetReasonKey(code) ?? "share.userLimit.accountReset.reason.notReady");
    return email ? t("share.userLimit.accountReset.rowReason", { email, reason }) : reason;
  };
  const describeError = (error: unknown) => {
    const text = errorText(error);
    const code = accountResetCodeInText(text);
    return code ? accountResetError(code) : text;
  };
  const confirmAccountPreview = async (policies: Record<string, ShareUserPolicy>, intent: string) => {
    if (!shareId || configRevision == null) throw new Error(t("share.userLimit.accountReset.saveFirst"));
    if (previewIntent === intent && accountReady) return true;
    setPreviewBusy(true);
    const seq = ++accountRequestSeq.current;
    try {
      const result = await shareApi.previewAccountReset(shareId, configRevision, policies);
      // A newer request owns the preview now; this answer must not confirm anything.
      if (seq !== accountRequestSeq.current) return false;
      setAccountPreview(result);
      setAccountLoadFailed(false);
      const refused = result.rows.find((row) => row.reason);
      const state = effectiveAccountResetCapability(result.accountResetCapability, result.accountQuotaCycle);
      if (refused) throw new Error(accountResetError(refused.reason, refused.email));
      if (state?.syncStatus !== "ready") throw new Error(accountResetError(state?.reason ?? "ACCOUNT_RESET_NOT_READY"));
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
    if (!draft) return undefined;
    const email = draft.email.trim().toLowerCase();
    if (!isValidShareEmail(email)) return undefined;
    const policy: ShareUserPolicy | undefined = draft.quotaUnit === "usdEquivalent" ? usdPolicyFor(draft) : {
      parallelLimit: draft.parallelLimit.trim() ? Number(draft.parallelLimit) : undefined,
      tokenLimit: draft.tokenLimit.trim() ? Number(draft.tokenLimit) : undefined,
      tokenPeriod: draft.tokenPeriod,
      tokenPeriodAnchorAtMs: ANCHORED_PERIODS.has(draft.tokenPeriod) ? parseUtcDateTime(draft.tokenPeriodAnchor) : undefined,
      expiresAt: draft.expiresAt ? new Date(draft.expiresAt).getTime() : undefined,
    };
    return policy && (policy.valueQuota || quotaConversionNeeded(savedGrants[email], policy)) ? {[email]: policy} : undefined;
  })();
  const batchUsdPolicies = (() => {
    if (!batchDraft?.applyTokenLimit || !selectedEditableEmails.size) return undefined;
    const patch = batchPatchFor(batchDraft);
    const parallel = patch.parallelLimit?.value;
    if (!patch.tokenLimit || (batchDraft.quotaUnit === "usdEquivalent" && !patch.tokenLimit.valueQuota) || anchorInvalid(batchDraft.tokenPeriod, patch.tokenLimit.periodAnchorAtMs)
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
  const valueStatus = useShareValueStatus(shareId, configRevision != null && (hasUsdGrant || usdSelected), configRevision);
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
  // A user already on the account cycle may keep it (or return to it) even
  // while new users cannot pick it; the original grant decides, not the draft.
  const savedOnAccountReset = !!savedGrant && savedGrant.active !== false
    && savedGrant.policy.tokenPeriod === "accountReset";
  const editingValueStatus = savedGrant ? matchingValueStatus(valueStatus.items, savedGrant) : undefined;
  const initialPreviewRow = singlePreview.preview?.rows.find(row => row.email === editedEmail);
  const canInitializeValue = !disabled && !!onValueUsageInitializationsChange && !savedGrant
    && singlePreview.preview?.ready === true
    && singlePreview.preview?.checks?.some(check => check.code === "value_usage_initialization" && check.ready)
    && initialPreviewRow?.canInitializeUsage === true && !!initialPreviewRow.windowId
    && valueRowKind(initialPreviewRow) !== "scheduled" && valueRowKind(initialPreviewRow) !== "blocked";

  const openAdd = () => {
    refreshAccountReset();
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
    refreshAccountReset();
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
    refreshAccountReset();
    setBatchCycle(firstSelected.accountQuotaCycle);
    setBatchError("");
    setAmountAttempted(false);
    setBatchDraft(batchPolicyDraft(firstSelected));
  };

  const saveDraft = async () => {
    if (!draft) return;
    if (draft.quotaUnit === "usdEquivalent" && !shareSaved) {
      setDraftError(t("share.userLimit.valueQuota.saveFirst")); return;
    }
    // Editing other fields of a user already on this period needs no new
    // projection; only a new period, a new limit or a usage edit does.
    const previousGrant = editingEmail ? value[editingEmail] : undefined;
    const accountQuotaUnchanged = !!previousGrant && previousGrant.policy.tokenPeriod === "accountReset"
      && !previousGrant.policy.valueQuota && draft.usageAction === "unchanged"
      && (previousGrant.policy.tokenLimit ?? null) === (draft.tokenLimit.trim() ? Number(draft.tokenLimit) : null);
    if (draft.tokenPeriod === "accountReset" && draft.quotaUnit === "tokens" && !accountQuotaUnchanged) {
      try {
        if (!await confirmAccountPreview({ [draft.email]: { tokenPeriod: "accountReset",
          tokenLimit: draft.tokenLimit.trim() ? Number(draft.tokenLimit) : undefined } }, JSON.stringify(draft))) return;
      } catch (error) { setDraftError(describeError(error)); return; }
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
                observed: observedTokens.toLocaleString(i18n.language),
              })
            : t("share.userLimit.invalidUsage"),
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
    if (requested.valueQuota || quotaConversionNeeded(savedGrants[email], requested)) {
      const preview = await resolveValuePreview(singlePreview);
      if (!preview) return;
      if (currentDraft.current !== draft) return;
      const row = preview.rows.find(row => row.email === email);
      try {
        next.quotaUnitConversion = row && quotaConversionInput(row, {amount: draft.quotaUnit === "usdEquivalent" ? draft.consumedValue : draft.consumedTokens, reason: draft.valueReason});
        next.quotaUnitConversionPolicy = next.quotaUnitConversion ? sharePolicyKey(requested) : undefined;
      } catch { setDraftError(t("share.userLimit.valueQuota.conversionCalibrationRequired")); return; }
      const parsed = parseInitialValueUsage(preview, email, row?.calibrationRequired ? "" : draft.consumedValue, draft.valueReason, !!savedGrants[email]);
      if (!parsed.ok || (parsed.initialization && !onValueUsageInitializationsChange)) {
        setDraftError(t("share.userLimit.valueQuota.initialInvalid")); return;
      }
      initialization = parsed.initialization;
    }
    if (!next.quotaUnitConversion && draft.quotaUnit==="tokens" && draft.usageAction === "set" && consumedTokens != null) {
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
    } else if (!next.quotaUnitConversion && draft.quotaUnit==="tokens" && draft.usageAction === "clear") {
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
      if (draft.quotaUnit === "usdEquivalent" || next.quotaUnitConversion) delete nextEdits[email];
      if (!next.quotaUnitConversion && draft.quotaUnit==="tokens" && draft.usageAction === "set" && consumedTokens != null) {
        nextEdits[email] = {
          action: "set",
          targetTokens: consumedTokens,
          expectedGrantRevision: previous?.revision,
          expectedAccountCycle: draft.tokenPeriod === "accountReset" ? next.accountQuotaCycle : undefined,
          period: draft.tokenPeriod,
          anchorAtMs: tokenPeriodAnchorAtMs,
          source: usageEdits[editingEmail ?? email]?.source ?? "manual",
        };
      } else if (!next.quotaUnitConversion && draft.quotaUnit==="tokens" && draft.usageAction === "clear") {
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
    if (batchDraft.applyTokenLimit && batchDraft.quotaUnit === "usdEquivalent" && !shareSaved) {
      setBatchError(t("share.userLimit.valueQuota.saveFirst")); return;
    }
    const previewGrants = grants.filter((grant) => selectedEditableEmails.has(grant.email));
    if (batchDraft.quotaUnit==="tokens" && (batchDraft.applyTokenLimit || batchDraft.applyConsumedTokens) && previewGrants.some((grant) =>
      (batchDraft.applyTokenLimit ? batchDraft.tokenPeriod : grant.policy.tokenPeriod) === "accountReset")) {
      try {
        const policies = Object.fromEntries(previewGrants.filter((grant) =>
          (batchDraft.applyTokenLimit ? batchDraft.tokenPeriod : grant.policy.tokenPeriod) === "accountReset")
          .map((grant) => [grant.email, { ...grant.policy, tokenPeriod: "accountReset" as const,
            tokenLimit: batchDraft.applyTokenLimit ? batchDraft.tokenLimit.trim() ? Number(batchDraft.tokenLimit) : undefined : grant.policy.tokenLimit }]));
        if (!await confirmAccountPreview(policies, JSON.stringify([batchDraft, [...selectedEditableEmails].sort()]))) return;
      } catch (error) { setBatchError(describeError(error)); return; }
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
                observed: usageFloor.toLocaleString(i18n.language),
              })
            : t("share.userLimit.invalidUsage")
          : t("share.userLimit.invalidPolicy"),
      );
      return;
    }

    const nextGrants = applyShareUserPolicyBatch(value, selectedEditableEmails, batchPatchFor(batchDraft));
    if (batchDraft.applyTokenLimit && batchUsdPolicies) {
      const preview = await resolveValuePreview(batchPreview);
      if (!preview) return;
      try {
        for (const row of preview.rows) {
          const grant = nextGrants[row.email];
          grant.quotaUnitConversion = quotaConversionInput(row, batchDraft.calibrations?.[row.email]);
          grant.quotaUnitConversionPolicy = grant.quotaUnitConversion ? sharePolicyKey(grant.policy) : undefined;
        }
      } catch { setBatchError(t("share.userLimit.valueQuota.conversionCalibrationRequired")); return; }
    }
    if (batchDraft.applyConsumedTokens && !valueQuota) {
      for (const grant of selectedGrants) {
        const current = nextGrants[grant.email] ?? grant;
        if (current.quotaUnitConversion) continue;
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
        if (nextGrants[email]?.quotaUnitConversion || nextGrants[email]?.policy.valueQuota || valueQuota) { delete nextEdits[email]; continue; }
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

  const unlimited = t("share.unlimited");
  const permanent = t("share.permanent");
  const periodLabels: Record<ShareTokenPeriod, string> = {
    accountReset: t("share.userLimit.accountReset.label"),
    lifetime: t("share.userLimit.periodLifetime"),
    day: t("share.userLimit.periodDay"),
    week: t("share.userLimit.periodWeek"),
    sevenDays: t("share.userLimit.periodSevenDays"),
    calendarMonth: t("share.userLimit.periodMonth"),
    thirtyDays: t("share.userLimit.periodThirtyDays"),
  };
  /** The disabled Account reset option says why, in a few words. */
  const periodOptionLabel = (period: ShareTokenPeriod) => period === "accountReset" && !accountSelectable
    ? t("share.userLimit.accountReset.optionUnavailable", { label: periodLabels.accountReset,
      reason: shareSaved ? t("share.userLimit.accountReset.optionNotReady") : t("share.userLimit.accountReset.optionSaveFirst") })
    : periodLabels[period];
  const accountResetShown = accountResetInUse(value);
  const draftIntent = draft ? JSON.stringify(draft) : "";
  const batchIntent = batchDraft ? JSON.stringify([batchDraft, [...selectedEditableEmails].sort()]) : "";
  // The Account reset projection is on screen and the next press applies it.
  const accountConfirmPending = !!draft && draft.tokenPeriod === "accountReset" && draft.quotaUnit === "tokens"
    && accountReady && previewIntent === draftIntent;
  const batchAccountConfirmPending = !!batchDraft && accountReady && previewIntent === batchIntent;
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
    <div className="space-y-3 md:col-span-2">
      <ShareSubheading
        title={t("share.userLimit.title")}
        description={freeAccess ? t("share.userLimit.publicHint") : t("share.userLimit.privateHint")}
        action={<>
          {selecting ? (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={disabled}
              onClick={exitSelecting}
            >
              {t("common.cancel")}
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
              ? t("share.userLimit.batchEditSelected", { count: selectedEditableEmails.size })
              : t("share.userLimit.batchEdit")}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={disabled}
            onClick={openAdd}
          >
            <Plus className="mr-1.5 h-4 w-4" />
            {t("share.userLimit.add")}
          </Button>
        </>}
      />
      {accountResetShown ? <ShareAccountResetStatus preview={accountPreview} saveFirst={!shareSaved}
        cycle={Object.values(value).find((grant) => grant.accountQuotaCycle)?.accountQuotaCycle}
        loadFailed={accountLoadFailed} onRetry={refreshAccountReset} /> : null}
      {valueStatus.diagnostics ? <details className="rounded-md border p-2 text-xs text-muted-foreground"><summary>{t("share.userLimit.valueQuota.diagnostics")}</summary>
        <p className="mt-2">{t("share.userLimit.valueQuota.diagnosticsSummary",{attempts:valueStatus.diagnostics.attempts,pending:valueStatus.diagnostics.pendingAttempts,
          unavailable:valueStatus.diagnostics.unavailableAttempts,gaps:valueStatus.diagnostics.coverageGaps,checkpoints:valueStatus.diagnostics.dailyCheckpoints})}</p>
        {valueStatus.diagnostics.capacityWarning ? <p className="mt-1 text-destructive" role="status">{t("share.userLimit.valueQuota.capacityWarning")}</p>:null}
      </details>:null}
      {/* Narrow screens scroll the table sideways rather than crushing its columns. */}
      <div className="w-full overflow-hidden rounded-md border border-border-default">
        <Table className="w-full min-w-[46rem]">
          <TableHeader>
            <TableRow>
              {selecting ? (
                <TableHead className="h-9 w-10 px-3">
                  <Checkbox
                    checked={allSelected ? true : someSelected ? "indeterminate" : false}
                    disabled={disabled || selectableEmails.length === 0}
                    aria-label={t("share.userLimit.selectAll")}
                    onCheckedChange={(checked) =>
                      setSelectedEmails(new Set(checked === true ? selectableEmails : []))
                    }
                  />
                </TableHead>
              ) : null}
              <TableHead className="h-9 w-[28%] px-3">{t("share.userLimit.email")}</TableHead>
              <TableHead className="h-9 w-[12%] px-3">{t("share.parallelLimit")}</TableHead>
              <TableHead className="h-9 w-[18%] px-3">{t("share.userLimit.quota")}</TableHead>
              <TableHead className="h-9 w-[22%] px-3">{t("share.userLimit.used")}</TableHead>
              <TableHead className="h-9 w-[12%] px-3">{t("share.expiration")}</TableHead>
              <TableHead className="h-9 w-20 px-3"><span className="sr-only">{t("share.userLimit.actions")}</span></TableHead>
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
                    {grant.role === "owner" ? <Badge variant="secondary">{t("share.userLimit.role.owner")}</Badge> : null}
                    {grant.manager === "routerShareMarket" && grant.active !== false ? (
                      <Badge variant="secondary">{t("share.userLimit.role.market")}</Badge>
                    ) : null}
                  </div>
                </TableCell>
                <TableCell className="px-3 py-2">{displayLimit(grant.policy.parallelLimit, unlimited)}</TableCell>
                <TableCell className="px-3 py-2 align-top">
                  {malformed ? "—" : <>{usdLimit ? t("share.userLimit.valueQuota.equivalentAmount", { amount: valueLimitLabel(usdLimit) }) : enforcedPolicy.tokenLimit == null ? unlimited : tokens(enforcedPolicy.tokenLimit)} · {periodLabels[enforcedPolicy.tokenPeriod]}</>}
                  {scheduled ? <ShareValueScheduledLine limitMicros={scheduled.policy.valueQuota!.limitUsdMicros} effectiveFromMs={scheduled.effectiveFromMs} /> : null}
                </TableCell>
                <TableCell className="px-3 py-2 align-top">
                  {malformed ? <p className="whitespace-normal text-[11px] text-destructive" role="status">{t("share.userLimit.valueQuota.malformed")}</p> : usdLimit ? <>
                    {/* The correction toggle below is the one reconcile control. */}
                    <ShareValueUsageCell status={rowStatus} limitMicros={usdLimit} stale={valueStatus.stale} />
                    {correctable && rowStatus ? <ShareValueUsageCorrection key={grant.email} shareId={shareId!} revision={configRevision!} grant={grant} status={rowStatus}
                      onSaved={valueStatus.merge} open={!draft && correctionEmail === emailKey} onOpenChange={(open) => setCorrectionEmail(open ? emailKey : null)} /> : null}
                  </> : <div className="whitespace-normal break-all font-mono text-xs">
                    {enforcedPolicy.tokenPeriod === "accountReset" && !stored.usageQuota ? t("share.userLimit.accountReset.pending") : tokens(currentGrantTokens(stored, usageEdits))}
                  </div>}
                  {grant.usageRebase && !usdLimit && !malformed ? (
                    <div className="text-[11px] text-muted-foreground">
                      {t("share.userLimit.rebaseTarget", {
                        value: tokens(grant.usageRebase.targetTokens),
                      })}
                    </div>
                  ) : null}
                  {grant.manager === "routerShareMarket" && grant.active !== false ? (
                    <div className="text-[11px] text-muted-foreground">
                      {t("share.userLimit.readOnly")}
                    </div>
                  ) : null}
                </TableCell>
                <TableCell className="px-3 py-2">{displayExpiry(grant.policy.expiresAt, permanent, i18n.language)}</TableCell>
                <TableCell className="px-3 py-2">
                  <div className="flex justify-end gap-1">
                    {!protectedEmails?.has(grant.email) && !malformed ? (
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon"
                        disabled={disabled}
                        onClick={() => openEdit(grant)}
                        title={t("common.edit")}
                        aria-label={t("common.edit")}
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
                        title={t("common.delete")}
                        aria-label={t("common.delete")}
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
                ? t("share.userLimit.edit")
                : t("share.userLimit.add")}
            </DialogTitle>
          </DialogHeader>
          {draft ? (
            <div className="grid gap-4 overflow-y-auto px-6 py-5 sm:grid-cols-2">
              <div className="space-y-2 sm:col-span-2">
                <Label htmlFor="share-user-email">{t("share.userLimit.email")}</Label>
                <Input id="share-user-email" type="email" disabled={editingEmail != null} value={draft.email} onChange={(event) => setDraft({ ...draft, email: event.target.value })} />
              </div>
              <div className="space-y-2">
                <Label htmlFor="share-user-parallel">{t("share.parallelLimit")}</Label>
                <Input id="share-user-parallel" type="number" min={1} placeholder={unlimited} value={draft.parallelLimit} onChange={(event) => setDraft({ ...draft, parallelLimit: event.target.value })} />
              </div>
              <div className="space-y-2">
                <Label id="share-user-unit-label">{t("share.userLimit.valueQuota.unit")}</Label>
                <ShareValueUnitToggle labelledBy="share-user-unit-label" readinessId="share-user-usd-readiness" value={draft.quotaUnit} usdDisabled={readiness.unavailable}
                  usdBlockedReason={shareSaved ? undefined : t("share.userLimit.valueQuota.saveFirst")}
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
                  label={draft.quotaUnit === "usdEquivalent" ? t("share.userLimit.valueQuota.limit") : t("share.tokenLimit")}
                  error={limitError(draft.quotaUnit, draft.tokenLimit)} onChange={(tokenLimit) => setDraft({ ...draft, tokenLimit })} />
                {draft.quotaUnit === "usdEquivalent" ? <ShareValueResourceToggle value={draft.resourceMetering} onChange={resourceMetering=>setDraft({...draft,resourceMetering})} /> : null}
                {singleUsdPolicies ? <ShareValuePreviewLine state={singlePreview} calibrations={{[editedEmail]:{amount: draft.quotaUnit === "usdEquivalent" ? draft.consumedValue : draft.consumedTokens, reason: draft.valueReason}}} /> : null}
              </div>
              <div className={draft.quotaUnit==="usdEquivalent"?"hidden":"space-y-2 sm:col-span-2"}>
                <Label htmlFor="share-user-consumed-tokens">
                  {t("share.userLimit.consumedTokens")}
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
                    {t("share.userLimit.clearRebase")}
                  </Button>
                </div>
                {editingEmail && value[editingEmail] ? (
                  <>
                    <p className="text-xs text-muted-foreground">
                      {t("share.userLimit.consumedHint", {
                        effective: currentGrantTokens(value[editingEmail]).toLocaleString(i18n.language),
                        observed: observedGrantTokens(value[editingEmail]).toLocaleString(i18n.language),
                      })}
                    </p>
                    {editingQuota ? (
                      <p className="text-xs text-muted-foreground">
                        {t("share.userLimit.savedQuotaHint", {
                          start: editingQuota.windowStartsAtMs
                            ? formatUtcWindow(editingQuota.windowStartsAtMs)
                            : t("share.userLimit.periodLifetime"),
                          end: editingQuota.windowEndsAtMs
                            ? formatUtcWindow(editingQuota.windowEndsAtMs)
                            : "—",
                          offset: editingQuota.manualOffsetTokens.toLocaleString(i18n.language),
                        })}
                      </p>
                    ) : null}
                  </>
                ) : (
                  <p className="text-xs text-muted-foreground">
                    {t("share.userLimit.newConsumedHint")}
                  </p>
                )}
              </div>
              {initialPreviewRow?.calibrationRequired ? <div className="grid gap-2 sm:col-span-2">
                <Label>{t("share.userLimit.valueQuota.conversionCalibrationRequired")}</Label>
                <Input inputMode="decimal" value={draft.quotaUnit === "usdEquivalent" ? draft.consumedValue : draft.consumedTokens}
                  onChange={event => setDraft({...draft, ...(draft.quotaUnit === "usdEquivalent" ? {consumedValue: event.target.value} : {consumedTokens: event.target.value})})} />
                <Input maxLength={1000} aria-label={t("share.userLimit.valueQuota.reason")} placeholder={t("share.userLimit.valueQuota.reason")}
                  value={draft.valueReason} onChange={event => setDraft({...draft, valueReason: event.target.value})} />
              </div> : null}
              {draft.quotaUnit === "usdEquivalent" && !initialPreviewRow?.calibrationRequired ? <div className="space-y-2 sm:col-span-2">
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
                <Label id="share-user-period-label">{t("share.userLimit.period")}</Label>
                <Select value={draft.tokenPeriod} onValueChange={(tokenPeriod: ShareTokenPeriod) => setDraft({
                  ...draft,
                  tokenPeriod,
                  tokenPeriodAnchor: ANCHORED_PERIODS.has(tokenPeriod)
                    ? (draft.tokenPeriodAnchor || toUtcDateTime())
                    : "",
                })}>
                  <SelectTrigger aria-labelledby="share-user-period-label"><SelectValue>{periodLabels[draft.tokenPeriod]}</SelectValue></SelectTrigger>
                  <SelectContent className="z-[130]">
                    {(Object.keys(periodLabels) as ShareTokenPeriod[]).map((period) => <SelectItem key={period} value={period}
                      disabled={period === "accountReset" && !accountSelectable && !savedOnAccountReset}>{periodOptionLabel(period)}</SelectItem>)}
                  </SelectContent>
                </Select>
              </div>
              {draft.tokenPeriod === "accountReset" ? <ShareAccountResetStatus className="sm:col-span-2" preview={accountPreview}
                cycle={draftCycle} saveFirst={!shareSaved} loadFailed={accountLoadFailed} onRetry={refreshAccountReset} /> : null}
              {ANCHORED_PERIODS.has(draft.tokenPeriod) ? (
                <div className="space-y-2 sm:col-span-2">
                  <Label htmlFor="share-user-period-anchor">
                    {t("share.userLimit.anchor")}
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
                    {t("share.userLimit.anchorHint")}
                  </p>
                  {draftWindow ? (
                    <p className="text-xs text-muted-foreground">
                      {t("share.userLimit.currentWindow", {
                        start: formatUtcWindow(draftWindow.start),
                        end: formatUtcWindow(draftWindow.end),
                      })}
                    </p>
                  ) : null}
                </div>
              ) : null}
              <div className="space-y-2">
                <Label htmlFor="share-user-expiry">{t("share.expiration")}</Label>
                <Input id="share-user-expiry" type="datetime-local" aria-describedby="share-user-expiry-hint" value={draft.expiresAt} onChange={(event) => setDraft({ ...draft, expiresAt: event.target.value })} />
                <p id="share-user-expiry-hint" className="text-xs text-muted-foreground">{t("share.userLimit.expiryHint")}</p>
              </div>
              {draftError ? <p className="text-sm text-destructive sm:col-span-2" role="alert">{draftError}</p> : null}
            </div>
          ) : null}
          {draft?.tokenPeriod === "accountReset" && previewIntent && accountPreview ? <div className="space-y-0.5 px-6 text-xs" role="status">
            <p>{t("share.userLimit.accountReset.preview")}</p>
            {accountPreview.rows.map((row) => <p key={row.email} className="break-all">{row.email}: {row.currentTokens == null ? t("share.userLimit.accountReset.pending") : tokens(row.currentTokens)} → {row.expectedTokens == null ? t("share.userLimit.accountReset.notReady") : tokens(row.expectedTokens)}{row.wouldExceed ? <span className="text-destructive"> · {t("share.userLimit.accountReset.exceeded")}</span> : null}</p>)}
          </div> : null}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => setDraft(null)}>{t("common.cancel")}</Button>
            <Button type="button" disabled={previewBusy || applying} aria-busy={applying || undefined} onClick={() => void saveDraft()}>
              {(draft?.quotaUnit === "usdEquivalent" && needsConfirmation(singlePreview)) || accountConfirmPending ? t("share.userLimit.valueQuota.confirmApply") : t("common.apply")}
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
              {t("share.userLimit.batchTitle")}
            </DialogTitle>
          </DialogHeader>
          {batchDraft ? (
            <div className="grid gap-4 overflow-y-auto px-6 py-5 sm:grid-cols-2">
              <p className="text-sm text-muted-foreground sm:col-span-2">
                {t("share.userLimit.batchHint", {
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
                    {t("share.parallelLimit")}
                  </Label>
                </div>
                <Input
                  type="number"
                  min={1}
                  aria-label={t("share.parallelLimit")}
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
                    {batchDraft.quotaUnit === "usdEquivalent" ? t("share.userLimit.valueQuota.limit") : t("share.tokenLimit")}
                  </Label>
                </div>
                <ShareValueUnitToggle disabled={!batchDraft.applyTokenLimit} readinessId="share-batch-usd-readiness" value={batchDraft.quotaUnit} usdDisabled={readiness.unavailable}
                  usdBlockedReason={shareSaved ? undefined : t("share.userLimit.valueQuota.saveFirst")}
                  onChange={(unit) => { if (unit === batchDraft.quotaUnit) return; setAmountAttempted(false); setBatchDraft({...batchDraft, quotaUnit:unit, applyConsumedTokens:false,
                    unitDrafts:{...batchDraft.unitDrafts,[batchDraft.quotaUnit]:{tokenLimit:batchDraft.tokenLimit,consumedTokens:batchDraft.consumedTokens,usageAction:batchDraft.usageAction}},
                    ...(batchDraft.unitDrafts?.[unit] ?? {tokenLimit:"",consumedTokens:"",usageAction:"unchanged"}),
                    calibrationDrafts:{...batchDraft.calibrationDrafts,[batchDraft.quotaUnit]:batchDraft.calibrations ?? {}},calibrations:batchDraft.calibrationDrafts?.[unit] ?? {}}); }} />
                {batchDraft.applyTokenLimit ? <ShareValueReadiness state={readiness} id="share-batch-usd-readiness" /> : null}
                <ShareValueLimitField id="share-user-batch-token" labelHidden unit={batchDraft.quotaUnit} value={batchDraft.tokenLimit} disabled={!batchDraft.applyTokenLimit}
                  label={batchDraft.quotaUnit === "usdEquivalent" ? t("share.userLimit.valueQuota.limit") : t("share.tokenLimit")}
                  error={batchDraft.applyTokenLimit ? limitError(batchDraft.quotaUnit, batchDraft.tokenLimit) : undefined}
                  onChange={(tokenLimit) => setBatchDraft({ ...batchDraft, tokenLimit })} />
                {batchDraft.applyTokenLimit && batchDraft.quotaUnit === "usdEquivalent" ? <ShareValueResourceToggle value={batchDraft.resourceMetering} onChange={resourceMetering=>setBatchDraft({...batchDraft,resourceMetering})} /> : null}
                {batchUsdPolicies ? <ShareValuePreviewLine state={batchPreview} calibrations={batchDraft.calibrations} /> : null}
                {batchPreview.preview?.rows.filter(row => row.calibrationRequired).map(row => <div key={row.email} className="grid gap-2">
                  <Label>{row.email} · {t("share.userLimit.valueQuota.conversionCalibrationRequired")}</Label>
                  <Input inputMode="decimal" value={batchDraft.calibrations?.[row.email]?.amount ?? ""}
                    onChange={event => setBatchDraft({...batchDraft, calibrations: {...batchDraft.calibrations, [row.email]: {amount: event.target.value, reason: batchDraft.calibrations?.[row.email]?.reason ?? ""}}})} />
                  <Input maxLength={1000} aria-label={t("share.userLimit.valueQuota.reason")} placeholder={t("share.userLimit.valueQuota.reason")}
                    value={batchDraft.calibrations?.[row.email]?.reason ?? ""}
                    onChange={event => setBatchDraft({...batchDraft, calibrations: {...batchDraft.calibrations, [row.email]: {amount: batchDraft.calibrations?.[row.email]?.amount ?? "", reason: event.target.value}}})} />
                </div>)}
              </div>
              <div className="space-y-2">
                {/* The period belongs to the quota: it changes only with "apply quota" checked. */}
                <Label id="share-user-batch-period-label">{t("share.userLimit.period")}</Label>
                <Select
                  value={batchDraft.tokenPeriod}
                  disabled={!batchDraft.applyTokenLimit}
                  onValueChange={(tokenPeriod: ShareTokenPeriod) => setBatchDraft({
                    ...batchDraft,
                    tokenPeriod,
                    tokenPeriodAnchor: ANCHORED_PERIODS.has(tokenPeriod)
                      ? (batchDraft.tokenPeriodAnchor || toUtcDateTime())
                      : "",
                  })}
                >
                  <SelectTrigger aria-labelledby="share-user-batch-period-label"><SelectValue>{periodLabels[batchDraft.tokenPeriod]}</SelectValue></SelectTrigger>
                  <SelectContent className="z-[130]">
                    {(Object.keys(periodLabels) as ShareTokenPeriod[]).map((period) => (
                      <SelectItem key={period} value={period} disabled={period === "accountReset" && !accountSelectable}>{periodOptionLabel(period)}</SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                {!batchDraft.applyTokenLimit ? <p className="text-xs text-muted-foreground">{t("share.userLimit.batchPeriodHint", {
                  label: batchDraft.quotaUnit === "usdEquivalent" ? t("share.userLimit.valueQuota.limit") : t("share.tokenLimit") })}</p> : null}
              </div>
              {batchDraft.applyTokenLimit && batchDraft.tokenPeriod === "accountReset" ? <ShareAccountResetStatus className="sm:col-span-2"
                preview={accountPreview} cycle={batchCycle} saveFirst={!shareSaved} loadFailed={accountLoadFailed} onRetry={refreshAccountReset} /> : null}
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
                    {t("share.userLimit.consumedTokens")}
                  </Label>
                </div>
                <div className="flex items-center gap-2">
                  <Input
                    type="number"
                    min={0}
                    step={1}
                    aria-label={t("share.userLimit.consumedTokens")}
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
                    {t("share.userLimit.clearRebase")}
                  </Button>
                </div>
                <p className="text-xs text-muted-foreground">
                  {t("share.userLimit.batchConsumedHint")}
                </p>
              </div>
              {batchDraft.applyTokenLimit && ANCHORED_PERIODS.has(batchDraft.tokenPeriod) ? (
                <div className="space-y-2 sm:col-span-2">
                  <Label htmlFor="share-user-batch-period-anchor">
                    {t("share.userLimit.anchor")}
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
                    {t("share.userLimit.anchorHint")}
                  </p>
                  {batchWindow ? (
                    <p className="text-xs text-muted-foreground">
                      {t("share.userLimit.currentWindow", {
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
                    {t("share.expiration")}
                  </Label>
                </div>
                <Input
                  type="datetime-local"
                  aria-label={t("share.expiration")}
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
          {batchDraft && previewIntent && accountPreview ? <div className="space-y-0.5 px-6 text-xs" role="status">
            <p>{t("share.userLimit.accountReset.preview")}</p>
            {accountPreview.rows.map((row) => <p key={row.email} className="break-all">{row.email}: {row.currentTokens == null ? t("share.userLimit.accountReset.pending") : tokens(row.currentTokens)} → {row.expectedTokens == null ? t("share.userLimit.accountReset.notReady") : tokens(row.expectedTokens)}{row.wouldExceed ? <span className="text-destructive"> · {t("share.userLimit.accountReset.exceeded")}</span> : null}</p>)}
          </div> : null}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => {
              setBatchDraft(null);
              setBatchError("");
            }}>
              {t("common.cancel")}
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
              {(batchDraft?.applyTokenLimit && batchDraft.quotaUnit === "usdEquivalent" && needsConfirmation(batchPreview)) || batchAccountConfirmPending
                ? t("share.userLimit.valueQuota.confirmApply")
                : t("share.userLimit.batchApply", {
                  count: selectedEditableEmails.size,
                })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
