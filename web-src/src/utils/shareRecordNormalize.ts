import type {
  ShareBindings,
  ShareRecord,
  ShareTokenPeriod,
  ShareUserGrant,
  ShareUserGrantMap,
  ShareUserPolicy,
  ShareUserUsage,
  ShareUserUsageBucket,
  ShareUserQuotaView,
  ShareUserUsageRebase,
} from "@/lib/api";
import { SHARE_APP_TYPES } from "@/lib/api/share";
import {
  normalizeShareLimitValue,
  UNLIMITED_TOKEN_LIMIT,
} from "@/utils/shareUtils";

type RawRecord = Record<string, unknown>;

function readString(
  raw: RawRecord,
  ...keys: string[]
): string | undefined {
  for (const key of keys) {
    const value = raw[key];
    if (typeof value === "string") {
      const trimmed = value.trim();
      if (trimmed) return trimmed;
    }
  }
  return undefined;
}

function readNumber(raw: RawRecord, ...keys: string[]): number | undefined {
  for (const key of keys) {
    const value = raw[key];
    if (typeof value === "number" && Number.isFinite(value)) {
      return value;
    }
    if (typeof value === "string" && value.trim()) {
      const parsed = Number(value.trim());
      if (Number.isFinite(parsed)) {
        return parsed;
      }
    }
  }
  return undefined;
}

function readShareLimit(raw: RawRecord, ...keys: string[]): number {
  const value = readNumber(raw, ...keys);
  if (value == null) {
    return UNLIMITED_TOKEN_LIMIT;
  }
  return normalizeShareLimitValue(value);
}

function readBool(raw: RawRecord, ...keys: string[]): boolean | undefined {
  for (const key of keys) {
    const value = raw[key];
    if (typeof value === "boolean") return value;
  }
  return undefined;
}

const SHARE_TOKEN_PERIODS: ReadonlySet<string> = new Set([
  "lifetime",
  "day",
  "week",
  "sevenDays",
  "calendarMonth",
  "thirtyDays",
]);

function normalizeTokenPeriod(value: unknown): ShareTokenPeriod {
  return typeof value === "string" && SHARE_TOKEN_PERIODS.has(value)
    ? (value as ShareTokenPeriod)
    : "lifetime";
}

function normalizeUsageBucket(value: unknown): ShareUserUsageBucket {
  const raw = value && typeof value === "object" ? (value as RawRecord) : {};
  return {
    startedAtMs: readNumber(raw, "startedAtMs", "started_at_ms") ?? 0,
    tokensUsed: readNumber(raw, "tokensUsed", "tokens_used") ?? 0,
    requestsCount: readNumber(raw, "requestsCount", "requests_count") ?? 0,
  };
}

function normalizeUserUsage(value: unknown): ShareUserUsage | undefined {
  if (!value || typeof value !== "object") return undefined;
  const raw = value as RawRecord;
  const anchoredRaw = raw.anchored;
  let anchored: ShareUserUsage["anchored"];
  if (anchoredRaw && typeof anchoredRaw === "object") {
    const item = anchoredRaw as RawRecord;
    const period = normalizeTokenPeriod(item.period);
    if (period === "sevenDays" || period === "thirtyDays") {
      anchored = {
        ...normalizeUsageBucket(item),
        period,
        anchorAtMs: readNumber(item, "anchorAtMs", "anchor_at_ms") ?? 0,
      };
    }
  }
  return {
    lifetime: normalizeUsageBucket(raw.lifetime),
    day: normalizeUsageBucket(raw.day),
    week: normalizeUsageBucket(raw.week),
    calendarMonth: normalizeUsageBucket(
      raw.calendarMonth ?? raw.calendar_month,
    ),
    ...(anchored ? { anchored } : {}),
  };
}

function normalizeUsageRebase(value: unknown): ShareUserUsageRebase | undefined {
  if (!value || typeof value !== "object") return undefined;
  const raw = value as RawRecord;
  const targetTokens = readNumber(raw, "targetTokens", "target_tokens");
  const appliedAtMs = readNumber(raw, "appliedAtMs", "applied_at_ms");
  if (targetTokens == null || appliedAtMs == null) return undefined;
  return {
    period: normalizeTokenPeriod(raw.period),
    anchorAtMs: readNumber(raw, "anchorAtMs", "anchor_at_ms"),
    windowStartsAtMs: readNumber(
      raw,
      "windowStartsAtMs",
      "window_starts_at_ms",
    ),
    windowEndsAtMs: readNumber(raw, "windowEndsAtMs", "window_ends_at_ms"),
    targetTokens,
    observedTokensAtRebase:
      readNumber(raw, "observedTokensAtRebase", "observed_tokens_at_rebase") ??
      0,
    observedRequestsAtRebase:
      readNumber(
        raw,
        "observedRequestsAtRebase",
        "observed_requests_at_rebase",
      ) ?? 0,
    usageWatermark:
      readNumber(raw, "usageWatermark", "usage_watermark") ?? 0,
    appliedAtMs,
    appliedBy: readString(raw, "appliedBy", "applied_by"),
    source: raw.source === "providerReset" ? "providerReset" : "manual",
  };
}

/**
 * The Server derives this view; the client only reads it.  Re-deriving the
 * effective/observed split in the browser would drift the moment the Server's
 * rebase arithmetic changes.
 */
function normalizeUsageQuota(value: unknown): ShareUserQuotaView | undefined {
  if (!value || typeof value !== "object") return undefined;
  const raw = value as RawRecord;
  const effectiveTokensUsed = readNumber(
    raw,
    "effectiveTokensUsed",
    "effective_tokens_used",
  );
  if (effectiveTokensUsed == null) return undefined;
  return {
    period: normalizeTokenPeriod(raw.period),
    anchorAtMs: readNumber(raw, "anchorAtMs", "anchor_at_ms"),
    windowStartsAtMs: readNumber(
      raw,
      "windowStartsAtMs",
      "window_starts_at_ms",
    ),
    windowEndsAtMs: readNumber(raw, "windowEndsAtMs", "window_ends_at_ms"),
    effectiveTokensUsed,
    observedTokensUsed:
      readNumber(raw, "observedTokensUsed", "observed_tokens_used") ?? 0,
    manualOffsetTokens:
      readNumber(raw, "manualOffsetTokens", "manual_offset_tokens") ?? 0,
    observedRequestsCount:
      readNumber(raw, "observedRequestsCount", "observed_requests_count") ?? 0,
    rebaseApplies: readBool(raw, "rebaseApplies", "rebase_applies") ?? false,
  };
}

function normalizeShareUserGrants(input: unknown): ShareUserGrantMap {
  if (!input || typeof input !== "object" || Array.isArray(input)) return {};
  const result: ShareUserGrantMap = {};
  for (const [key, entry] of Object.entries(input as RawRecord)) {
    if (!entry || typeof entry !== "object") continue;
    const raw = entry as RawRecord;
    const policyRaw =
      raw.policy && typeof raw.policy === "object"
        ? (raw.policy as RawRecord)
        : {};
    const email = readString(raw, "email") ?? key;
    const grant: ShareUserGrant = {
      email,
      role: raw.role === "owner" ? "owner" : "shareto",
      active: readBool(raw, "active") ?? true,
      policy: {
        parallelLimit: readNumber(
          policyRaw,
          "parallelLimit",
          "parallel_limit",
        ),
        tokenLimit: readNumber(policyRaw, "tokenLimit", "token_limit"),
        tokenPeriod: normalizeTokenPeriod(
          policyRaw.tokenPeriod ?? policyRaw.token_period,
        ),
        tokenPeriodAnchorAtMs: readNumber(
          policyRaw,
          "tokenPeriodAnchorAtMs",
          "token_period_anchor_at_ms",
        ),
        expiresAt: readNumber(policyRaw, "expiresAt", "expires_at"),
      } satisfies ShareUserPolicy,
      usage: normalizeUserUsage(raw.usage),
      usageRebase: normalizeUsageRebase(
        raw.usageRebase ?? raw.usage_rebase,
      ),
      usageQuota: normalizeUsageQuota(raw.usageQuota ?? raw.usage_quota),
      createdAtMs: readNumber(raw, "createdAtMs", "created_at_ms"),
      updatedAtMs: readNumber(raw, "updatedAtMs", "updated_at_ms"),
      revokedAtMs: readNumber(raw, "revokedAtMs", "revoked_at_ms"),
      revision: readNumber(raw, "revision"),
      manager:
        raw.manager === "routerShareMarket" ||
        raw.manager === "owner" ||
        raw.manager === "manual"
          ? raw.manager
          : undefined,
      entitlementId: readString(raw, "entitlementId", "entitlement_id"),
    };
    result[email.trim().toLowerCase()] = grant;
  }
  return result;
}

function normalizeAppKey(value: unknown): keyof ShareBindings | null {
  if (typeof value !== "string") return null;
  const normalized = value.trim().toLowerCase();
  return SHARE_APP_TYPES.includes(normalized as keyof ShareBindings)
    ? (normalized as keyof ShareBindings)
    : null;
}

export function normalizeShareBindings(raw: RawRecord): ShareBindings {
  const bindingsValue = raw.bindings;
  const normalized: ShareBindings = {};

  if (bindingsValue && typeof bindingsValue === "object" && !Array.isArray(bindingsValue)) {
    for (const app of SHARE_APP_TYPES) {
      const providerId = (bindingsValue as RawRecord)[app];
      if (typeof providerId === "string" && providerId.trim()) {
        normalized[app] = providerId.trim();
      }
    }
    if (Object.keys(normalized).length > 0) return normalized;
  }

  if (Array.isArray(bindingsValue)) {
    for (const item of bindingsValue) {
      if (!item || typeof item !== "object") continue;
      const binding = item as RawRecord;
      const app = normalizeAppKey(binding.app ?? binding.appType ?? binding.app_type);
      const providerId = readString(binding, "providerId", "provider_id");
      if (app && providerId) normalized[app] = providerId;
    }
    if (Object.keys(normalized).length > 0) return normalized;
  }

  const app = normalizeAppKey(raw.app ?? raw.appType ?? raw.app_type);
  const providerId = readString(raw, "providerId", "provider_id");
  if (app && providerId) {
    normalized[app] = providerId;
  }
  return normalized;
}

function normalizeFreeAccess(raw: RawRecord): boolean {
  return readBool(raw, "freeAccess", "free_access") ?? false;
}

function normalizeExpiresAt(raw: RawRecord): string {
  const iso = readString(raw, "expiresAt", "expires_at");
  if (iso) return iso;
  const millis = readNumber(raw, "expiresAt", "expires_at");
  if (typeof millis === "number" && millis > 0) {
    return new Date(millis).toISOString();
  }
  return new Date(0).toISOString();
}

export function normalizeShareRecord(raw: unknown): ShareRecord | null {
  if (!raw || typeof raw !== "object") return null;
  const record = raw as RawRecord;
  const id = readString(record, "id");
  if (!id) return null;

  const bindings = normalizeShareBindings(record);
  const shareSlug =
    readString(record, "shareSlug", "share_slug", "tunnelSubdomain", "tunnel_subdomain") ??
    null;
  const subdomain = readString(record, "subdomain") ?? shareSlug;
  const tunnelUrl =
    readString(
      record,
      "tunnelUrl",
      "tunnel_url",
      "routerUrl",
      "router_url",
      "directUrl",
      "direct_url",
    ) ?? null;
  const status = readString(record, "status") ?? "paused";
  const enabled = readBool(record, "enabled", "autoStart", "auto_start");

  return {
    id,
    capacityPoolId:
      readString(record, "capacityPoolId", "capacity_pool_id") ?? id,
    name: readString(record, "name", "displayName", "display_name") ?? id,
    ownerEmail: readString(record, "ownerEmail", "owner_email") ?? "",
    description: readString(record, "description") ?? null,
    freeAccess: normalizeFreeAccess(record),
    bindings,
    apiKey: readString(record, "apiKey", "api_key") ?? "",
    settingsConfig:
      readString(record, "settingsConfig", "settings_config") ?? null,
    tokenLimit: readShareLimit(record, "tokenLimit", "token_limit"),
    parallelLimit: readShareLimit(record, "parallelLimit", "parallel_limit"),
    tokensUsed: readNumber(record, "tokensUsed", "tokens_used") ?? 0,
    requestsCount: readNumber(record, "requestsCount", "requests_count") ?? 0,
    expiresAt: normalizeExpiresAt(record),
    shareSlug,
    subdomain,
    tunnelUrl,
    status:
      status === "active" || enabled === true
        ? "active"
        : status === "paused" || status === "stopped"
          ? "paused"
          : status,
    autoStart: readBool(record, "autoStart", "auto_start") ?? false,
    createdAt:
      readString(record, "createdAt", "created_at") ??
      new Date().toISOString(),
    lastUsedAt:
      readString(record, "lastUsedAt", "last_used_at") ?? null,
    configRevision:
      readNumber(record, "configRevision", "config_revision") ?? 0,
    routerSyncedRevision:
      readNumber(record, "routerSyncedRevision", "router_synced_revision") ?? 0,
    descriptorGeneration:
      readNumber(record, "descriptorGeneration", "descriptor_generation") ?? 0,
    descriptorFingerprint:
      readString(record, "descriptorFingerprint", "descriptor_fingerprint") ?? null,
    routerSyncedDescriptorGeneration:
      readNumber(
        record,
        "routerSyncedDescriptorGeneration",
        "router_synced_descriptor_generation",
      ) ?? 0,
    routerSyncedDescriptorFingerprint:
      readString(
        record,
        "routerSyncedDescriptorFingerprint",
        "router_synced_descriptor_fingerprint",
      ) ?? null,
    routerLastSyncError:
      readString(record, "routerLastSyncError", "router_last_sync_error") ?? null,
    allowPersonalCredits:
      readBool(record, "allowPersonalCredits", "allow_personal_credits") ?? false,
    autoConsumeBankedReset:
      readBool(
        record,
        "autoConsumeBankedReset",
        "auto_consume_banked_reset",
      ) ?? false,
    bankedResetExpiryLeadMinutes:
      readNumber(
        record,
        "bankedResetExpiryLeadMinutes",
        "banked_reset_expiry_lead_minutes",
      ) ?? 60,
    previousResponseCacheEnabled:
      readBool(
        record,
        "previousResponseCacheEnabled",
        "previous_response_cache_enabled",
      ) ?? false,
    userGrants: normalizeShareUserGrants(
      record.userGrants ?? record.user_grants,
    ),
  };
}

export function normalizeShareRecords(raw: unknown): ShareRecord[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .map((item) => normalizeShareRecord(item))
    .filter((item): item is ShareRecord => item !== null);
}

export function getShareProviderId(
  share: Pick<ShareRecord, "bindings"> & {
    app?: string;
    providerId?: string;
    provider_id?: string;
  },
  appId: keyof ShareBindings,
): string | null {
  const fromBindings = share.bindings?.[appId];
  if (typeof fromBindings === "string" && fromBindings.trim()) {
    return fromBindings.trim();
  }
  const legacyApp = normalizeAppKey(share.app);
  const legacyProviderId = readString(share as RawRecord, "providerId", "provider_id");
  if (legacyApp === appId && legacyProviderId) return legacyProviderId;
  return null;
}
