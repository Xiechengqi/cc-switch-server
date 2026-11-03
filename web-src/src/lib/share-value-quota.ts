// Wire amounts remain integers. Never convert quota limits through Number.
import type { ShareUserGrant, ShareUserValueUsageInitializationMap } from "./api/share";

const LIMIT_MAX_MICROS = 1000000000000000n;
export type ValueLimitError = "empty" | "format" | "too_many_decimals" | "too_small" | "too_large";
export type ValueLimitParse = { ok: true; micros: string } | { ok: false; error: ValueLimitError };

/** Strip an optional `$` / `US$` and validate `1,234.5` style grouping. */
function splitAmount(value: string, maxDecimals: number): { whole: string; fraction: string } | "empty" | "format" | "too_many_decimals" {
  let text = value.trim();
  const currency = /^(?:US\$|\$)\s*/i.exec(text);
  if (currency) text = text.slice(currency[0].length);
  if (!text) return "empty";
  const match = /^(\d{1,3}(?:,\d{3})+|\d*)(?:\.(\d*))?$/.exec(text);
  if (!match || (!match[1] && !match[2])) return "format";
  const fraction = match[2] ?? "";
  if (fraction.length > maxDecimals) return "too_many_decimals";
  return { whole: match[1].replace(/,/g, "") || "0", fraction };
}

/** Accepts `$10.00`, ` 10`, `.5`, `05`, `1,000`; rejects signs, exponents and >6 decimals. */
export function parseValueLimitInput(value: string): ValueLimitParse {
  const parts = splitAmount(value, 6);
  if (typeof parts === "string") return { ok: false, error: parts };
  const result = BigInt(parts.whole) * 1000000n + BigInt(parts.fraction.padEnd(6, "0"));
  if (result <= 0n) return { ok: false, error: "too_small" };
  if (result > LIMIT_MAX_MICROS) return { ok: false, error: "too_large" };
  return { ok: true, micros: result.toString() };
}
export function parseValueLimit(value: string): string | undefined {
  const parsed = parseValueLimitInput(value);
  return parsed.ok ? parsed.micros : undefined;
}
export function valueLimitErrorKey(error: ValueLimitError): string {
  return `share.userLimit.valueQuota.amountError.${error}`;
}
export function valueLimitInput(micros: string): string {
  if (!/^(0|[1-9]\d*)$/.test(micros)) return "";
  const value = BigInt(micros), fraction = (value % 1000000n).toString().padStart(6, "0").replace(/0+$/, "");
  return (value / 1000000n).toString() + (fraction ? "." + fraction : "");
}
const withGrouping = (value: bigint) => value.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
/** `US$10`, `US$10.50`, `US$1,000`. */
export function valueLimitLabel(micros: string | null | undefined): string {
  if (!micros || !/^(0|[1-9]\d*)$/.test(micros)) return "—";
  const value = BigInt(micros), fraction = (value % 1000000n).toString().padStart(6, "0").replace(/0+$/, "");
  return "US$" + withGrouping(value / 1000000n) + (fraction ? "." + fraction.padEnd(2, "0") : "");
}
export function valueUsageLabel(picos: string | null | undefined): string {
  if (!picos || !/^(0|[1-9]\d*)$/.test(picos)) return "—";
  const value = BigInt(picos);
  if (value > 0n && value < 1000000n) return "< US$0.000001";
  const whole = value / 1000000000000n, decimal = (value % 1000000000000n / 1000000n).toString().padStart(6, "0").replace(/0+$/, "").padEnd(2, "0");
  return "US$" + withGrouping(whole) + "." + decimal;
}

export const VALUE_REASON_CODES = ["ready", "scheduled_next_window", "metered_from_now", "unverified_surface", "pricing_unavailable",
  "storage_unavailable", "router_upgrade_required", "blocked_needs_reconcile", "invalid_policy"] as const;
export type ValueReasonCode = (typeof VALUE_REASON_CODES)[number];
const BLOCKING_CODES = new Set<string>(VALUE_REASON_CODES.slice(3));
export function valueReasonCode(code: unknown): ValueReasonCode | undefined {
  return VALUE_REASON_CODES.find((known) => known === code);
}
export type ValuePreviewRow = { email: string; effectiveFromMs: number; meteredFromMs?: number; usedPicos?: string | null;
  windowId?: string; canInitializeUsage?: boolean;
  reason?: string | null; reasonCode?: string | null; ready?: boolean | null };
/** A requested policy refused before any timing is worked out (bad amount, period or user). */
export type ValueInvalidRow = { email: string; reasonCode?: string | null; reason?: string | null };
export type ValuePreview = { ready: boolean; configRevision?: number; capabilityReady?: boolean; invalidRows?: ValueInvalidRow[];
  checks?: ValueCheck[];
  reason?: string | null; reasonCode?: string | null; activePricingRevision?: string | null; rows: ValuePreviewRow[] };
/**
 * One readiness check. `blockedBy` names another check this one waits on
 * (e.g. `acknowledged_tariff` waits on `sqlite_authority`), so it is a
 * consequence rather than a cause while that check is failing.
 */
export type ValueCheck = { code: string; ready: boolean; blockedBy?: string | null; state?: string; app?: string;
  profileId?: string | null; driverId?: string | null };
/** Fix-first order: an earlier check usually unblocks the later ones. Unknown codes go last. */
export const VALUE_CHECK_ORDER = ["router_protocol", "sqlite_authority", "metering_storage", "tariff_published",
  "acknowledged_tariff", "text_surface"] as const;
const checkRank = (code: string) => {
  const index = (VALUE_CHECK_ORDER as readonly string[]).indexOf(code);
  return index < 0 ? VALUE_CHECK_ORDER.length : index;
};
/** Every failed check, in fix-first order (stable within a code). */
export function valueFailedChecks(checks: ValueCheck[] | null | undefined): ValueCheck[] {
  return (Array.isArray(checks) ? checks : []).filter((check) => check && check.ready !== true)
    .map((check, index) => ({ check, index }))
    .sort((a, b) => checkRank(a.check.code) - checkRank(b.check.code) || a.index - b.index)
    .map(({ check }) => check);
}
/** The failing check this one waits on, if that blocker is itself failing. */
export function valueCheckBlocker(check: ValueCheck, checks: ValueCheck[] | null | undefined): string | undefined {
  const blocker = check.blockedBy?.trim();
  if (!blocker || blocker === check.code) return undefined;
  return valueFailedChecks(checks).some((other) => other.code === blocker) ? blocker : undefined;
}
/**
 * Failed checks that are causes: no `blockedBy`, or a blocker that is not
 * failing. A blocker cycle would hide everything, so it falls back to all
 * failed checks rather than to none.
 */
export function valueReadinessRoots(checks: ValueCheck[] | null | undefined): ValueCheck[] {
  const failed = valueFailedChecks(checks);
  const roots = failed.filter((check) => !valueCheckBlocker(check, failed));
  return roots.length ? roots : failed;
}
/**
 * What the one-line readiness label says: the first root, plus how many other
 * root causes (distinct codes; several unverified bindings count once) remain.
 */
export function valueReadinessSummary(checks: ValueCheck[] | null | undefined): { root: ValueCheck; more: number } | undefined {
  const roots = valueReadinessRoots(checks);
  if (!roots.length) return undefined;
  return { root: roots[0], more: new Set(roots.map((check) => check.code)).size - 1 };
}
export function isValuePreview(value: unknown, revision: number): value is ValuePreview {
  if (!value || typeof value !== "object") return false;
  const p = value as ValuePreview;
  return typeof p.ready === "boolean" && p.configRevision === revision && Array.isArray(p.rows)
    && p.rows.every((row) => row && typeof row.email === "string")
    && (p.invalidRows == null || (Array.isArray(p.invalidRows) && p.invalidRows.every((row) => row && typeof row.email === "string")))
    && (p.checks == null || (Array.isArray(p.checks) && p.checks.every((c) => c && typeof c.code === "string" && typeof c.ready === "boolean")));
}
export function valueCapabilityReady(value: ValuePreview | undefined) {
  return value?.capabilityReady === true && Array.isArray(value.checks)
    && ["router_protocol", "sqlite_authority", "metering_storage", "acknowledged_tariff", "text_surface"].every((code) => value.checks!.some((c) => c?.code === code))
    && value.checks.every((check) => check?.ready === true);
}
export type ValueRowKind = "scheduled" | "fromNow" | "now" | "blocked";
/** Old Servers send no reason code, so their start time decides. */
export function valueRowKind(row: ValuePreviewRow, now = Date.now()): ValueRowKind {
  const code = row.reasonCode ?? undefined;
  if (row.ready === false || (code && BLOCKING_CODES.has(code))) return "blocked";
  if (code === "scheduled_next_window") return "scheduled";
  if (code === "metered_from_now") return "fromNow";
  if (code === "ready") return "now";
  if (code) return "blocked";
  return row.effectiveFromMs > now + 5000 ? "scheduled" : "now";
}
/** A later start or carried-over usage must be seen before it is applied. */
export function valuePreviewNeedsConfirmation(preview: ValuePreview | undefined, now = Date.now()): boolean {
  return !!preview?.rows.some((row) => {
    const kind = valueRowKind(row, now);
    return kind === "scheduled" || (kind !== "blocked" && !!row.usedPicos && /^[1-9]\d*$/.test(row.usedPicos));
  });
}
/** A translation key (or untranslatable raw text for unknown codes). */
export type ValueMessage = { key: string; values?: Record<string, string | number> } | { raw: string };
function reasonMessage(code: string | null | undefined, reason: string | null | undefined): ValueMessage {
  const known = valueReasonCode(code);
  if (known) return { key: `share.userLimit.valueQuota.reasonCode.${known}` };
  return reason?.trim() ? { raw: reason.trim() } : { key: "share.userLimit.valueQuota.notReady" };
}
export function valueInvalidRowMessage(row: ValueInvalidRow): ValueMessage {
  return reasonMessage(row.reasonCode ?? "invalid_policy", row.reason);
}
export function valuePreviewBlocker(preview: ValuePreview, now = Date.now()): { email?: string; message: ValueMessage } | undefined {
  const invalid = preview.invalidRows?.[0];
  if (invalid) return { email: invalid.email, message: valueInvalidRowMessage(invalid) };
  const row = preview.rows.find((item) => valueRowKind(item, now) === "blocked");
  if (row) return { email: row.email, message: reasonMessage(row.reasonCode, row.reason) };
  if (!preview.ready) return { message: reasonMessage(preview.reasonCode, preview.reason) };
  return undefined;
}
export type ValueRequestError = "conflict" | "offline" | "timeout" | "upgrade" | "controlAuth" | "invalidResponse" | "failed";
/** Same distinctions the Router UI makes, from what the Server Web API exposes. */
export function valueRequestError(error: unknown): ValueRequestError {
  const candidate = error as { status?: number; code?: string; name?: string; message?: string } | undefined;
  if (candidate?.code === "quota_revision_conflict" || candidate?.status === 409) return "conflict";
  if (candidate?.code === "quota_server_invalid_response") return "invalidResponse";
  if (candidate?.status === 401 || candidate?.status === 403) return "controlAuth";
  if (candidate?.status === 404 || candidate?.status === 501) return "upgrade";
  if (candidate?.status === 408 || candidate?.status === 504 || candidate?.name === "TimeoutError") return "timeout";
  if (error instanceof TypeError || candidate?.status === 502) return "offline";
  return "failed";
}
export function valueErrorMessage(error: unknown): ValueMessage {
  const candidate = error as { code?: string; details?: { reasonCode?: string } } | undefined;
  const code = candidate?.details?.reasonCode ?? candidate?.code;
  if (code === "quota_target_invalid") return { key: "share.userLimit.valueQuota.error.targetInvalid" };
  if (code === "price_unavailable") return { key: "share.userLimit.valueQuota.reasonCode.pricing_unavailable" };
  if (code === "quota_unavailable") return { key: "share.userLimit.valueQuota.error.unavailable" };
  if (code === "value_quota_not_active") return { key: "share.userLimit.valueQuota.error.notActive" };
  const known = valueReasonCode(code);
  if (known) return { key: `share.userLimit.valueQuota.reasonCode.${known}` };
  return { key: `share.userLimit.valueQuota.readiness.${valueRequestError(error)}` };
}
export class ValueQuotaResponseError extends Error {
  readonly code = "quota_server_invalid_response";
  constructor() { super("quota_server_invalid_response"); this.name = "ValueQuotaResponseError"; }
}

export type ValueStatus = { email: string; usedPicos: string; observedPicos: string; manualOffsetPicos: string; windowId: string;
  policyEpoch: string; usageWatermark: number; meteringStatus: string; syncedAtMs: number; limitUsdMicros?: string | null;
  windowStartsAtMs?: number; resetsAtMs?: number | null; meteredFromMs?: number; pendingRiskPicos?: string;
  /** Set while a USD policy waits for its window; the row itself still describes the enforced one. */
  pendingLimitUsdMicros?: string | null; pendingEffectiveFromMs?: number | null;
  /** Why the window is not plainly observed, e.g. on `unavailable` rows. */
  reasonCode?: string | null };
/** A per-user refusal returned where a status row would be (corrections). */
export type ValueRowError = { email: string; code: string };
/**
 * Status rows and per-user refusals, kept apart: `{email, error:{code}}` must
 * never stand in for that user's last good status.
 */
export function splitValueRows(items: unknown): { rows: ValueStatus[]; errors: ValueRowError[] } {
  const rows: ValueStatus[] = [], errors: ValueRowError[] = [];
  for (const item of Array.isArray(items) ? items : []) {
    if (!item || typeof item !== "object" || typeof (item as { email?: unknown }).email !== "string") continue;
    const raw = item as ValueStatus & { error?: { code?: unknown } | null };
    const email = raw.email.trim().toLowerCase();
    if (raw.error != null) errors.push({ email, code: typeof raw.error.code === "string" ? raw.error.code : "failed" });
    else rows.push({ ...raw, email });
  }
  return { rows, errors };
}
/** Rows from one correction replace only those users; nobody else's row is lost. */
export function mergeValueRows(current: ValueStatus[], updates: ValueStatus[]): ValueStatus[] {
  const touched = new Set(updates.map((row) => row.email));
  return [...current.filter((row) => !touched.has(row.email)), ...updates];
}
/** Why a row is not plainly observed; nothing when usage is confirmed. */
export function valueStatusReason(status: ValueStatus | undefined): ValueMessage | undefined {
  const code = status?.reasonCode?.trim();
  if (!status || !code || valueStateName(status) === "observed") return undefined;
  const known = valueReasonCode(code);
  if (known) return { key: `share.userLimit.valueQuota.reasonCode.${known}` };
  if (code === "price_unavailable") return { key: "share.userLimit.valueQuota.reasonCode.pricing_unavailable" };
  return { raw: code };
}
/** Dates follow the UI language rather than the browser default. */
export function formatValueTime(ms: number | null | undefined, language: string | undefined, style: "short" | "full" = "full"): string {
  const date = new Date(ms ?? Number.NaN);
  if (!Number.isFinite(date.getTime())) return "—";
  const tag = language || undefined;
  try {
    return style === "short"
      ? date.toLocaleString(tag, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" })
      : date.toLocaleString(tag);
  } catch { return date.toLocaleString(); }
}
export function matchingValueStatus(items: ValueStatus[], grant: ShareUserGrant): ValueStatus | undefined {
  if (!grant.active || !grant.valueMetering) return undefined;
  return items.find((status) => status.email === grant.email.trim().toLowerCase() && status.policyEpoch === grant.valueMetering!.quotaPolicyEpoch
    && (status.limitUsdMicros ?? null) === (grant.policy.valueQuota?.limitUsdMicros ?? null));
}
export function valueQuotaAvailability(usedPicos?: string, limitMicros?: string) {
  if (!usedPicos || !limitMicros || !/^(0|[1-9]\d{0,38})$/.test(usedPicos) || !/^[1-9]\d{0,15}$/.test(limitMicros)) return undefined;
  const used = BigInt(usedPicos), limit = BigInt(limitMicros) * 1000000n;
  return { remainingPicos: (used >= limit ? 0n : limit - used).toString(), exhausted: used >= limit,
    nearLimit: used * 5n >= limit * 4n, percent: Number(used >= limit ? 10000n : used * 10000n / limit) / 100 };
}
export type ValueDiagnostics = { attempts: number; pendingAttempts: number; unavailableAttempts: number; coverageGaps: number; dailyCheckpoints: number; databaseBytes: number; capacityWarning: boolean };
export const VALUE_STATUS_STALE_MS = 180000;
export function valueStateName(status: ValueStatus): string {
  return ["observed", "pending", "blocked", "unavailable", "conservative", "scheduled"].includes(status.meteringStatus) ? status.meteringStatus : "unavailable";
}
export function valueStateKey(status: ValueStatus | undefined, now = Date.now()): string {
  if (!status) return "share.userLimit.valueQuota.pending";
  if (now - status.syncedAtMs > VALUE_STATUS_STALE_MS) return "share.userLimit.valueQuota.stale";
  return "share.userLimit.valueQuota.state." + valueStateName(status);
}
export type ValueUsageEdit = { action: "set" | "clear"; targetPicos?: string; expectedGrantRevision: number; windowId: string; expectedWatermark: number; operationId: string; reason: string };
export function valueUsageInput(input: string | undefined): string {
  if (!input || !/^(0|[1-9]\d{0,21})$/.test(input)) return "";
  const n = BigInt(input), scale = 1_000_000_000_000n;
  const fraction = String(n % scale).padStart(12, "0").replace(/0+$/, "");
  return `${n / scale}${fraction ? `.${fraction}` : ""}`;
}

export function parseInitialValueUsage(preview: ValuePreview, email: string, raw: string, reason: string, existing: boolean):
  {ok: true; initialization?: ShareUserValueUsageInitializationMap[string]} | {ok: false} {
  if (!raw.trim()) return {ok: true};
  const amount = parseValueUsage(raw), row = preview.rows.find(row => row.email === email);
  if (existing || amount == null || row?.canInitializeUsage !== true || typeof row.windowId !== "string" || !row.windowId || row.windowId.length > 512
    || !preview.ready || !preview.checks?.some(check => check.code === "value_usage_initialization" && check.ready)
    || ["scheduled", "blocked"].includes(valueRowKind(row))
    || (amount !== "0" && (!reason.trim() || new TextEncoder().encode(reason).length > 1000))) return {ok: false};
  return {ok: true, ...(amount !== "0" ? {initialization: {targetPicos: amount, windowId: row.windowId, reason: reason.trim()}} : {})};
}
export function parseValueUsage(value: string): string | undefined {
  const parts = splitAmount(value, 12);
  if (typeof parts === "string") return undefined;
  const result = BigInt(parts.whole) * 1000000000000n + BigInt(parts.fraction.padEnd(12, "0"));
  return result <= 1000000000000000000000n ? result.toString() : undefined;
}
