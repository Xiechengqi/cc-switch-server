import { useEffect, useMemo, useState } from "react";
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
} from "@/lib/api/share";
import { isValidShareEmail } from "@/utils/shareFormUtils";
import { applyShareUserPolicyBatch } from "./share-user-policy-batch";

type PolicyDraft = {
  email: string;
  parallelLimit: string;
  tokenLimit: string;
  tokenPeriod: ShareTokenPeriod;
  tokenPeriodAnchor: string;
  expiresAt: string;
  consumedTokens: string;
  usageAction: "unchanged" | "set" | "clear";
};

type BatchPolicyDraft = Omit<
  PolicyDraft,
  "email" | "consumedTokens" | "usageAction"
> & {
  applyParallelLimit: boolean;
  applyTokenLimit: boolean;
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
  value: ShareUserGrantMap;
  ownerEmail: string;
  defaultPolicy: ShareUserPolicy;
  protectedEmails?: ReadonlySet<string>;
  usageEdits?: ShareUserUsageEditMap;
  onUsageEditsChange?: (value: ShareUserUsageEditMap) => void;
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
    parallelLimit: policy.parallelLimit == null ? "" : String(policy.parallelLimit),
    tokenLimit: policy.tokenLimit == null ? "" : String(policy.tokenLimit),
    tokenPeriod: policy.tokenPeriod ?? "lifetime",
    tokenPeriodAnchor: toUtcDateTime(policy.tokenPeriodAnchorAtMs),
    expiresAt: toLocalDateTime(policy.expiresAt),
    consumedTokens: "",
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

function batchPolicyDraft(policy: ShareUserPolicy): BatchPolicyDraft {
  const draft = policyDraft("", policy);
  return {
    parallelLimit: draft.parallelLimit,
    tokenLimit: draft.tokenLimit,
    tokenPeriod: draft.tokenPeriod,
    tokenPeriodAnchor: draft.tokenPeriodAnchor,
    expiresAt: draft.expiresAt,
    applyParallelLimit: false,
    applyTokenLimit: false,
    applyExpiresAt: false,
  };
}

function displayLimit(value: number | undefined, unlimited: string) {
  return value == null ? unlimited : value.toLocaleString();
}

function displayExpiry(value: number | undefined, permanent: string) {
  return value == null ? permanent : new Date(value).toLocaleString();
}

export function ShareUserGrantsEditor({
  value,
  ownerEmail,
  defaultPolicy,
  protectedEmails,
  usageEdits = {},
  onUsageEditsChange,
  disabled,
  onChange,
}: ShareUserGrantsEditorProps) {
  const { t } = useTranslation();
  const normalizedOwner = ownerEmail.trim().toLowerCase();
  const [editingEmail, setEditingEmail] = useState<string | null>(null);
  const [draft, setDraft] = useState<PolicyDraft | null>(null);
  const [selecting, setSelecting] = useState(false);
  const [selectedEmails, setSelectedEmails] = useState<Set<string>>(new Set());
  const [batchDraft, setBatchDraft] = useState<BatchPolicyDraft | null>(null);
  const [batchError, setBatchError] = useState("");
  const [draftError, setDraftError] = useState("");

  const grants = useMemo(
    () =>
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
      .filter((grant) => !protectedEmails?.has(grant.email))
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

  const openAdd = () => {
    setEditingEmail(null);
    setDraftError("");
    setDraft({
      ...policyDraft("", defaultPolicy),
      consumedTokens: "",
      usageAction: "unchanged",
    });
  };

  const openEdit = (grant: ShareUserGrant) => {
    if (grant.role === "owner") return;
    setEditingEmail(grant.email);
    setDraftError("");
    setDraft({
      ...policyDraft(grant.email, grant.policy),
      ...usageEditForGrant(grant, usageEdits),
    });
  };

  const exitSelecting = () => {
    setSelecting(false);
    setSelectedEmails(new Set());
  };

  const openBatchEdit = () => {
    if (!selecting) {
      setSelecting(true);
      return;
    }
    const firstSelected = grants.find((grant) => selectedEditableEmails.has(grant.email));
    if (!firstSelected) return;
    setBatchError("");
    setBatchDraft(batchPolicyDraft(firstSelected.policy));
  };

  const saveDraft = () => {
    if (!draft) return;
    const email = draft.email.trim().toLowerCase();
    const parallelLimit = draft.parallelLimit.trim()
      ? Number(draft.parallelLimit)
      : undefined;
    const tokenLimit = draft.tokenLimit.trim()
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
    const observedTokens = previous ? observedGrantTokens(previous) : 0;
    const usageInvalid =
      draft.usageAction === "set" &&
      (consumedTokens == null ||
        !Number.isSafeInteger(consumedTokens) ||
        consumedTokens < 0 ||
        consumedTokens < observedTokens);
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
      }
      return;
    }
    setDraftError("");
    const next: ShareUserGrant = {
      ...previous,
      email,
      role: email === normalizedOwner ? "owner" : "shareto",
      active: true,
      policy: {
        parallelLimit,
        tokenLimit,
        tokenPeriod: draft.tokenPeriod,
        tokenPeriodAnchorAtMs,
        expiresAt,
      },
    };
    if (draft.usageAction === "set" && consumedTokens != null) {
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
    } else if (draft.usageAction === "clear") {
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
    if (onUsageEditsChange) {
      const nextEdits: ShareUserUsageEditMap = { ...usageEdits };
      if (editingEmail && editingEmail !== email) delete nextEdits[editingEmail];
      if (draft.usageAction === "set" && consumedTokens != null) {
        nextEdits[email] = {
          action: "set",
          targetTokens: consumedTokens,
          expectedGrantRevision: previous?.revision,
          period: draft.tokenPeriod,
          anchorAtMs: tokenPeriodAnchorAtMs,
          source: usageEdits[editingEmail ?? email]?.source ?? "manual",
        };
      } else if (draft.usageAction === "clear" && previous?.usageRebase) {
        nextEdits[email] = {
          action: "clear",
          expectedGrantRevision: previous.revision,
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

  const saveBatchDraft = () => {
    if (!batchDraft || selectedEditableEmails.size === 0) return;
    const parallelLimit = batchDraft.parallelLimit.trim()
      ? Number(batchDraft.parallelLimit)
      : undefined;
    const tokenLimit = batchDraft.tokenLimit.trim()
      ? Number(batchDraft.tokenLimit)
      : undefined;
    const expiresAt = batchDraft.expiresAt
      ? new Date(batchDraft.expiresAt).getTime()
      : undefined;
    const anchored = ANCHORED_PERIODS.has(batchDraft.tokenPeriod);
    const tokenPeriodAnchorAtMs = anchored
      ? parseUtcDateTime(batchDraft.tokenPeriodAnchor)
      : undefined;
    if (
      !batchDraft.applyParallelLimit &&
      !batchDraft.applyTokenLimit &&
      !batchDraft.applyExpiresAt
    ) {
      return;
    }
    if (
      (batchDraft.applyParallelLimit && parallelLimit != null &&
        (!Number.isInteger(parallelLimit) || parallelLimit < 1)) ||
      (batchDraft.applyTokenLimit && tokenLimit != null &&
        (!Number.isInteger(tokenLimit) || tokenLimit < 1)) ||
      (batchDraft.applyExpiresAt && expiresAt != null && !Number.isFinite(expiresAt)) ||
      (batchDraft.applyTokenLimit && anchored && (
        tokenPeriodAnchorAtMs == null ||
        !Number.isFinite(tokenPeriodAnchorAtMs) ||
        tokenPeriodAnchorAtMs > Math.floor(Date.now() / 60_000) * 60_000
      ))
    ) {
      setBatchError(t("share.userLimit.invalidPolicy", {
        defaultValue: "限制必须为正整数，且时间必须有效。",
      }));
      return;
    }

    onChange(applyShareUserPolicyBatch(value, selectedEditableEmails, {
      ...(batchDraft.applyParallelLimit
        ? { parallelLimit: { value: parallelLimit } }
        : {}),
      ...(batchDraft.applyTokenLimit
        ? {
            tokenLimit: {
              value: tokenLimit,
              period: batchDraft.tokenPeriod,
              periodAnchorAtMs: tokenPeriodAnchorAtMs,
            },
          }
        : {}),
      ...(batchDraft.applyExpiresAt
        ? { expiresAt: { value: expiresAt } }
        : {}),
    }));
    if (onUsageEditsChange && batchDraft.applyTokenLimit) {
      const nextEdits = { ...usageEdits };
      for (const email of selectedEditableEmails) delete nextEdits[email];
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

      <div className="overflow-x-auto rounded-md border border-border-default">
        <Table className={selecting ? "min-w-[900px]" : "min-w-[840px]"}>
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
              <TableHead className="h-9 px-3">Email</TableHead>
              <TableHead className="h-9 px-3">{t("share.parallelLimit", { defaultValue: "并发" })}</TableHead>
              <TableHead className="h-9 px-3">Token</TableHead>
              <TableHead className="h-9 px-3">
                {t("share.userLimit.consumedTokens", {
                  defaultValue: "已消耗 Token（当前周期）",
                })}
              </TableHead>
              <TableHead className="h-9 px-3">{t("share.expiration", { defaultValue: "到期" })}</TableHead>
              <TableHead className="h-9 w-20 px-3" />
            </TableRow>
          </TableHeader>
          <TableBody>
            {grants.map((grant) => (
              <TableRow key={grant.email}>
                {selecting ? (
                  <TableCell className="w-10 px-3 py-2">
                    <Checkbox
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
                    />
                  </TableCell>
                ) : null}
                <TableCell className="px-3 py-2">
                  <div className="flex min-w-0 items-center gap-2">
                    <span className="truncate">{grant.email}</span>
                    {grant.role === "owner" ? <Badge variant="secondary">Owner</Badge> : null}
                    {grant.manager === "routerShareMarket" ? (
                      <Badge variant="secondary">Share Market</Badge>
                    ) : null}
                  </div>
                </TableCell>
                <TableCell className="px-3 py-2">{displayLimit(grant.policy.parallelLimit, unlimited)}</TableCell>
                <TableCell className="px-3 py-2">
                  {displayLimit(grant.policy.tokenLimit, unlimited)} · {periodLabels[grant.policy.tokenPeriod]}
                </TableCell>
                <TableCell className="px-3 py-2">
                  <div className="font-mono text-xs">
                    {currentGrantTokens(grant, usageEdits).toLocaleString()}
                  </div>
                  {grant.usageRebase ? (
                    <div className="text-[11px] text-muted-foreground">
                      {t("share.userLimit.rebaseTarget", {
                        defaultValue: "基线 {{value}}",
                        value: grant.usageRebase.targetTokens.toLocaleString(),
                      })}
                    </div>
                  ) : null}
                  {grant.manager === "routerShareMarket" ? (
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
                    {grant.role !== "owner" && !protectedEmails?.has(grant.email) ? (
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
            ))}
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
                <Label htmlFor="share-user-token">{t("share.tokenLimit", { defaultValue: "Token 限额" })}</Label>
                <Input id="share-user-token" type="number" min={1} placeholder={unlimited} value={draft.tokenLimit} onChange={(event) => setDraft({ ...draft, tokenLimit: event.target.value })} />
              </div>
              <div className="space-y-2 sm:col-span-2">
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
                {draftError ? (
                  <p className="text-xs text-destructive">{draftError}</p>
                ) : null}
              </div>
              <div className="space-y-2">
                <Label>{t("share.userLimit.period", { defaultValue: "Token 周期" })}</Label>
                <Select value={draft.tokenPeriod} onValueChange={(tokenPeriod: ShareTokenPeriod) => setDraft({
                  ...draft,
                  tokenPeriod,
                  tokenPeriodAnchor: ANCHORED_PERIODS.has(tokenPeriod)
                    ? (draft.tokenPeriodAnchor || toUtcDateTime())
                    : "",
                })}>
                  <SelectTrigger><SelectValue /></SelectTrigger>
                  <SelectContent className="z-[120]">
                    {(Object.keys(periodLabels) as ShareTokenPeriod[]).map((period) => <SelectItem key={period} value={period}>{periodLabels[period]}</SelectItem>)}
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
            </div>
          ) : null}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => setDraft(null)}>{t("common.cancel", { defaultValue: "取消" })}</Button>
            <Button type="button" onClick={saveDraft}>{t("common.save", { defaultValue: "保存" })}</Button>
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
                    {t("share.tokenLimit", { defaultValue: "Token 限额" })}
                  </Label>
                </div>
                <Input
                  type="number"
                  min={1}
                  disabled={!batchDraft.applyTokenLimit}
                  placeholder={unlimited}
                  value={batchDraft.tokenLimit}
                  onChange={(event) => setBatchDraft({
                    ...batchDraft,
                    tokenLimit: event.target.value,
                  })}
                />
              </div>
              <div className="space-y-2">
                <Label>{t("share.userLimit.period", { defaultValue: "Token 周期" })}</Label>
                <Select
                  disabled={!batchDraft.applyTokenLimit}
                  value={batchDraft.tokenPeriod}
                  onValueChange={(tokenPeriod: ShareTokenPeriod) => setBatchDraft({
                    ...batchDraft,
                    tokenPeriod,
                    tokenPeriodAnchor: ANCHORED_PERIODS.has(tokenPeriod)
                      ? (batchDraft.tokenPeriodAnchor || toUtcDateTime())
                      : "",
                  })}
                >
                  <SelectTrigger><SelectValue /></SelectTrigger>
                  <SelectContent className="z-[120]">
                    {(Object.keys(periodLabels) as ShareTokenPeriod[]).map((period) => (
                      <SelectItem key={period} value={period}>{periodLabels[period]}</SelectItem>
                    ))}
                  </SelectContent>
                </Select>
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
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => {
              setBatchDraft(null);
              setBatchError("");
            }}>
              {t("common.cancel", { defaultValue: "取消" })}
            </Button>
            <Button
              type="button"
              disabled={batchDraft != null &&
                !batchDraft.applyParallelLimit &&
                !batchDraft.applyTokenLimit &&
                !batchDraft.applyExpiresAt}
              onClick={saveBatchDraft}
            >
              {t("share.userLimit.batchApply", {
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
