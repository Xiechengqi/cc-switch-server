import type { ShareBindings, ShareRecord, ShareSaleMarketKind } from "@/lib/api";
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

function normalizeForSale(raw: RawRecord): ShareRecord["forSale"] {
  const value = raw.forSale ?? raw.for_sale;
  if (value === "Free") return "Free";
  if (raw.freeAccess === true || raw.free_access === true) return "Free";
  if (value === "Yes" || value === true) return "Yes";
  return "No";
}

function normalizeSaleMarketKind(raw: RawRecord): ShareSaleMarketKind {
  const value = readString(raw, "saleMarketKind", "sale_market_kind");
  return value === "share" ? "share" : "token";
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

function normalizeAcl(raw: RawRecord) {
  const acl = raw.acl;
  if (!acl || typeof acl !== "object") {
    return {
      sharedWithEmails: [] as string[],
      marketAccessMode: "all" as const,
    };
  }
  const aclRecord = acl as RawRecord;
  const sharedWithEmails = Array.isArray(aclRecord.sharedWithEmails)
    ? aclRecord.sharedWithEmails
    : Array.isArray(aclRecord.shared_with_emails)
      ? aclRecord.shared_with_emails
      : [];
  const marketAccessModeRaw = readString(
    aclRecord,
    "marketAccessMode",
    "market_access_mode",
  );
  return {
    sharedWithEmails: sharedWithEmails.filter(
      (email): email is string => typeof email === "string",
    ),
    marketAccessMode:
      marketAccessModeRaw === "selected"
        ? ("selected" as const)
        : ("all" as const),
  };
}

export function normalizeShareRecord(raw: unknown): ShareRecord | null {
  if (!raw || typeof raw !== "object") return null;
  const record = raw as RawRecord;
  const id = readString(record, "id");
  if (!id) return null;

  const bindings = normalizeShareBindings(record);
  const acl = normalizeAcl(record);
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
    name: readString(record, "name", "displayName", "display_name") ?? id,
    ownerEmail: readString(record, "ownerEmail", "owner_email") ?? "",
    sharedWithEmails: acl.sharedWithEmails,
    marketAccessMode: acl.marketAccessMode,
    accessByApp:
      (record.accessByApp as ShareRecord["accessByApp"]) ??
      (record.access_by_app as ShareRecord["accessByApp"]),
    appSettings:
      (record.appSettings as ShareRecord["appSettings"]) ??
      (record.app_settings as ShareRecord["appSettings"]),
    forSaleOfficialPricePercentByApp:
      (record.forSaleOfficialPricePercentByApp as ShareRecord["forSaleOfficialPricePercentByApp"]) ??
      (record.for_sale_official_price_percent_by_app as ShareRecord["forSaleOfficialPricePercentByApp"]) ??
      {},
    description: readString(record, "description") ?? null,
    forSale: normalizeForSale(record),
    saleMarketKind: normalizeSaleMarketKind(record),
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
    routerLastSyncError:
      readString(record, "routerLastSyncError", "router_last_sync_error") ?? null,
    userGrants:
      (record.userGrants as ShareRecord["userGrants"]) ??
      (record.user_grants as ShareRecord["userGrants"]) ??
      {},
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
