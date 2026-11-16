// Display rules for the Account reset quota period. The Server sends
// lower-case projection codes on grants and upper-case codes from preview and
// admission; both map to one set of translations here, so no raw code is shown.
import type {
  AccountQuotaCycle,
  AccountResetCapability,
  ShareUserGrantMap,
} from "@/lib/api/share";

const SYNC_STATUSES = ["ready", "initializing", "stale", "error", "needsReconcile"] as const;
export type AccountResetSyncStatus = (typeof SYNC_STATUSES)[number];

const REASONS: Record<string, string> = {
  history_incomplete: "historyIncomplete",
  cycle_expired: "cycleExpired",
  QUOTA_HISTORY_INCOMPLETE: "historyIncomplete",
  ACCOUNT_RESET_NOT_READY: "notReady",
  ACCOUNT_RESET_ROUTER_UPGRADE_REQUIRED: "routerUpgradeRequired",
  ACCOUNT_RESET_UNSUPPORTED: "unsupported",
  QUOTA_LEDGER_CAPACITY_LIMIT: "ledgerCapacityLimit",
  QUOTA_LEDGER_UNAVAILABLE: "ledgerUnavailable",
  QUOTA_CYCLE_CONFLICT: "cycleConflict",
  QUOTA_EXHAUSTED: "quotaExhausted",
  identity_unverified: "identityUnverified",
  quota_boundary_unavailable: "quotaBoundaryUnavailable",
  multiple_quota_scopes: "multipleQuotaScopes",
  non_recurring_package: "nonRecurringPackage",
  profile_unsupported: "profileUnsupported",
  quota_provider_missing: "quotaProviderMissing",
  quota_scope_unproven: "quotaScopeUnproven",
};

const BASES = ["upstreamWindow", "derivedFromWindow", "knownNaturalBoundary", "publicAnnouncement", "redeemConfirmation"] as const;
const BUCKETS = new Set(["shared_weekly", "agentic_subscription_base", "included_plan_credits", "premium_monthly"]);

const PREFIX = "share.userLimit.accountReset.";

/** Translation key for a sync status; unknown values read as "sync failed". */
export function accountResetStatusKey(status: string | null | undefined): string {
  return PREFIX + "status." + ((SYNC_STATUSES as readonly string[]).includes(status ?? "") ? status : "error");
}

/** Translation key for a reason code, or the generic one for codes this page does not know. */
export function accountResetReasonKey(code: string | null | undefined): string | undefined {
  const trimmed = code?.trim();
  if (!trimmed) return undefined;
  return PREFIX + "reason." + (REASONS[trimmed] ?? "unknown");
}

/** Whether a reason code has its own translation (otherwise keep it for diagnostics). */
export function isKnownAccountResetReason(code: string | null | undefined): boolean {
  return !!code?.trim() && code.trim() in REASONS;
}

/**
 * The first known code inside an error message. Preview and save failures
 * arrive as `Error(code)` or as a message that embeds the code.
 */
export function accountResetCodeInText(text: string): string | undefined {
  const trimmed = text.trim();
  if (trimmed in REASONS) return trimmed;
  return Object.keys(REASONS).find((code) => code === code.toUpperCase() && trimmed.includes(code));
}

export function accountResetBasisKey(basis: string | null | undefined): string {
  return PREFIX + "basis." + ((BASES as readonly string[]).includes(basis ?? "") ? basis : "unknown");
}

export function accountResetBucketKey(bucketId: string | null | undefined): string {
  return PREFIX + "bucket." + (bucketId && BUCKETS.has(bucketId) ? bucketId : "generic");
}

/** True once a known end has passed: the cycle on screen no longer applies. */
export function accountCycleEnded(cycle: AccountQuotaCycle | undefined, nowMs = Date.now()): boolean {
  return cycle?.endsAtMs != null && cycle.endsAtMs <= nowMs;
}

/**
 * What the page shows. An ended cycle is stale whatever the projection said,
 * so a frozen "initializing" never presents a past window as current. A
 * reason that names the real blocker (incomplete history, a full ledger) is
 * kept; otherwise the reason is that the cycle ended.
 */
export function effectiveAccountResetCapability(
  capability: AccountResetCapability | undefined,
  cycle: AccountQuotaCycle | undefined,
  nowMs = Date.now(),
): { syncStatus: AccountResetSyncStatus | string; reason?: string | null } | undefined {
  if (!capability) return undefined;
  if (accountCycleEnded(cycle, nowMs)) {
    const blocker = capability.reason && capability.reason !== "ACCOUNT_RESET_NOT_READY" ? capability.reason : null;
    return { syncStatus: "stale", reason: blocker ?? "cycle_expired" };
  }
  return { syncStatus: capability.syncStatus, reason: capability.reason };
}

/**
 * The period may be chosen when ready, or when only the observation is old:
 * history is complete and the cycle has not ended, so the preview that runs
 * on Apply refreshes the account and can confirm it. The preview reports that
 * case as `stale` with `ACCOUNT_RESET_NOT_READY` (the Share projection, as
 * `stale` with no reason). Incomplete history, a full ledger or an ended
 * cycle cannot be fixed by a refresh and stay unavailable.
 */
export function accountResetSelectable(state: { syncStatus: string; reason?: string | null } | undefined): boolean {
  if (state?.syncStatus === "ready") return true;
  return state?.syncStatus === "stale" && (!state.reason || state.reason === "ACCOUNT_RESET_NOT_READY");
}

/** Some active user is limited per Account reset period. */
export function accountResetInUse(grants: ShareUserGrantMap): boolean {
  return Object.values(grants).some(
    (grant) => grant.active !== false && grant.policy.tokenPeriod === "accountReset",
  );
}

/** Local date and time in the UI language. */
export function formatAccountResetTime(ms: number | null | undefined, language: string | undefined): string {
  const date = new Date(ms ?? Number.NaN);
  if (!Number.isFinite(date.getTime())) return "—";
  try {
    return date.toLocaleString(language || undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
  } catch {
    return date.toLocaleString();
  }
}
