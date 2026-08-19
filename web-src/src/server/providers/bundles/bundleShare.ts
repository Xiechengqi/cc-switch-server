import {
  shareApi,
  type ShareRecord,
  type ShareUserGrantMap,
  type ShareUserPolicy,
  type ShareUserUsageEditMap,
  type ShareTotalUsageEdit,
} from "@/lib/api/share";
import {
  buildShareUserGrants,
  normalizeShareEmails,
} from "@/utils/shareFormUtils";
import {
  getProviderSharePhase,
  PERMANENT_EXPIRES_AT,
  UNLIMITED_PARALLEL_LIMIT,
  UNLIMITED_TOKEN_LIMIT,
} from "@/utils/shareUtils";

export interface ProviderBundleShareDraft {
  enabled: boolean;
  subdomain: string;
  description: string;
  freeAccess: boolean;
  tokenLimit: string;
  /** Share-total consumed tokens; an operator correction, not a limit. */
  tokensUsed: string;
  parallelLimit: string;
  expiry: ProviderBundleShareExpiry;
  userGrants: ShareUserGrantMap;
  userUsageEdits: ShareUserUsageEditMap;
  allowPersonalCredits: boolean;
  autoConsumeBankedReset: boolean;
  bankedResetExpiryLeadMinutes: string;
  previousResponseCacheEnabled: boolean;
}

export const BUNDLE_SHARE_EXPIRY_PRESETS = [
  { value: "1h", seconds: 60 * 60, labelKey: "share.expiry.oneHour" },
  { value: "6h", seconds: 6 * 60 * 60, labelKey: "share.expiry.sixHours" },
  { value: "1d", seconds: 24 * 60 * 60, labelKey: "share.expiry.oneDay" },
  {
    value: "7d",
    seconds: 7 * 24 * 60 * 60,
    labelKey: "share.expiry.sevenDays",
  },
  {
    value: "30d",
    seconds: 30 * 24 * 60 * 60,
    labelKey: "share.expiry.thirtyDays",
  },
] as const;

export type ProviderBundleShareExpiry =
  "permanent" | (typeof BUNDLE_SHARE_EXPIRY_PRESETS)[number]["value"];

const RESERVED_SHARE_SLUGS = new Set([
  "admin",
  "api",
  "cdn-cgi",
  "router",
  "www",
]);

export function isValidShareSlug(value: string): boolean {
  const slug = value.trim();
  return (
    slug.length >= 6 &&
    slug.length <= 30 &&
    !slug.includes("--") &&
    !RESERVED_SHARE_SLUGS.has(slug) &&
    /^[a-z][a-z0-9-]*[a-z0-9]$/.test(slug)
  );
}

export function shareForBundle(
  shares: ShareRecord[] | undefined,
  bundleId: string,
): ShareRecord | undefined {
  return shares?.find((share) =>
    Object.values(share.bindings).some((providerId) => providerId === bundleId),
  );
}

export function createBundleShareDraft(
  share?: ShareRecord,
): ProviderBundleShareDraft {
  const expiresAt = share?.expiresAt ? Date.parse(share.expiresAt) : Number.NaN;
  const remaining = expiresAt - Date.now();
  const expiry =
    !Number.isFinite(expiresAt) || expiresAt >= Date.parse(PERMANENT_EXPIRES_AT)
      ? "permanent"
      : BUNDLE_SHARE_EXPIRY_PRESETS.reduce((closest, preset) =>
          Math.abs(preset.seconds * 1000 - remaining) <
          Math.abs(closest.seconds * 1000 - remaining)
            ? preset
            : closest,
        ).value;
  const canonicalUserGrants = share?.userGrants ?? {};
  const shareToEmails = normalizeShareEmails(
    Object.values(canonicalUserGrants)
      .filter((grant) => grant.active !== false && grant.role === "shareto")
      .map((grant) => grant.email),
  );
  const defaultPolicy: ShareUserPolicy = {
    parallelLimit:
      share && share.parallelLimit > 0 ? share.parallelLimit : undefined,
    tokenLimit: share && share.tokenLimit > 0 ? share.tokenLimit : undefined,
    tokenPeriod: "lifetime",
    expiresAt:
      Number.isFinite(expiresAt) && expiresAt < Date.parse(PERMANENT_EXPIRES_AT)
        ? expiresAt
        : undefined,
  };
  return {
    enabled: Boolean(
      share && share.status !== "paused" && share.status !== "deleted",
    ),
    subdomain: share?.shareSlug ?? "",
    description: share?.description ?? "",
    freeAccess: share?.freeAccess ?? false,
    tokenLimit: share && share.tokenLimit >= 0 ? String(share.tokenLimit) : "",
    tokensUsed: String(Math.max(0, share?.tokensUsed ?? 0)),
    parallelLimit:
      share && share.parallelLimit >= 0 ? String(share.parallelLimit) : "",
    expiry,
    userGrants: share
      ? buildShareUserGrants({
          source: canonicalUserGrants,
          ownerEmail: share.ownerEmail,
          aclEmails: shareToEmails,
          defaultPolicy,
        })
      : {},
    userUsageEdits: {},
    allowPersonalCredits: share?.allowPersonalCredits ?? false,
    autoConsumeBankedReset: share?.autoConsumeBankedReset ?? false,
    bankedResetExpiryLeadMinutes: String(
      share?.bankedResetExpiryLeadMinutes ?? 60,
    ),
    previousResponseCacheEnabled: share
      ? Boolean(share.previousResponseCacheEnabled)
      : true,
  };
}

function limitValue(value: string, fallback: number): number {
  if (!value.trim()) return fallback;
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 0) {
    throw new Error("Share limits must be non-negative integers");
  }
  return parsed;
}

function expiresAt(draft: ProviderBundleShareDraft): string {
  if (draft.expiry === "permanent") return PERMANENT_EXPIRES_AT;
  const preset = BUNDLE_SHARE_EXPIRY_PRESETS.find(
    (candidate) => candidate.value === draft.expiry,
  );
  if (!preset) throw new Error("Share expiry preset is invalid");
  return new Date(Date.now() + preset.seconds * 1000).toISOString();
}

export async function saveBundleShare(
  bundleId: string,
  draft: ProviderBundleShareDraft,
  existing?: ShareRecord,
): Promise<ShareRecord | undefined> {
  const subdomain = draft.subdomain.trim();
  if (draft.enabled && subdomain && !isValidShareSlug(subdomain)) {
    throw new Error("Share slug is invalid");
  }
  const tokenLimit = limitValue(draft.tokenLimit, UNLIMITED_TOKEN_LIMIT);
  const parallelLimit = limitValue(
    draft.parallelLimit,
    UNLIMITED_PARALLEL_LIMIT,
  );
  const expiry = expiresAt(draft);
  const userGrants = Object.keys(draft.userGrants).length
    ? draft.userGrants
    : undefined;
  const allowedUsageEmails = new Set(
    Object.values(draft.userGrants)
      .filter(
        (grant) =>
          grant.active !== false && grant.manager !== "routerShareMarket",
      )
      .map((grant) => grant.email.trim().toLowerCase()),
  );
  const userUsageEdits = Object.fromEntries(
    Object.entries(draft.userUsageEdits).filter(([email]) =>
      allowedUsageEmails.has(email.trim().toLowerCase()),
    ),
  );
  // Only an actual edit is sent.  Resending the value the editor was opened
  // with would silently erase requests that landed in the meantime.
  let shareUsageEdit: ShareTotalUsageEdit | undefined;
  const tokensUsedRaw = draft.tokensUsed.trim();
  if (existing && tokensUsedRaw) {
    const tokensUsed = Number(tokensUsedRaw);
    if (!Number.isSafeInteger(tokensUsed) || tokensUsed < 0) {
      throw new Error("Consumed tokens must be a non-negative integer");
    }
    if (tokensUsed !== existing.tokensUsed) {
      shareUsageEdit =
        tokensUsed === 0 ? { action: "clear" } : { action: "set", tokensUsed };
    }
  }
  const bankedResetExpiryLeadMinutes = limitValue(
    draft.bankedResetExpiryLeadMinutes,
    60,
  );
  if (
    bankedResetExpiryLeadMinutes < 10 ||
    bankedResetExpiryLeadMinutes > 7 * 24 * 60
  ) {
    throw new Error(
      "Banked Reset lead time must be between 10 and 10080 minutes",
    );
  }
  return shareApi.saveProviderBundleShare({
    bundleId,
    shareId: existing?.id,
    expectedConfigRevision: existing?.configRevision,
    enabled: draft.enabled,
    subdomain: subdomain || existing?.shareSlug || "",
    description: draft.description.trim() || undefined,
    freeAccess: draft.freeAccess,
    tokenLimit,
    parallelLimit,
    expiresAt: expiry,
    allowPersonalCredits: draft.allowPersonalCredits,
    autoConsumeBankedReset: draft.autoConsumeBankedReset,
    bankedResetExpiryLeadMinutes,
    previousResponseCacheEnabled: draft.previousResponseCacheEnabled,
    userGrants,
    userUsageEdits,
    shareUsageEdit,
  });
}

export async function enableBundleShare(
  bundleId: string,
  existing?: ShareRecord,
): Promise<ShareRecord | undefined> {
  const draft = createBundleShareDraft(existing);
  draft.enabled = true;
  if (!draft.subdomain.trim()) {
    const suggestion = await shareApi.suggestShareSlug();
    draft.subdomain = suggestion.subdomain;
  }
  return saveBundleShare(bundleId, draft, existing);
}

export async function toggleBundleShare(
  bundleId: string,
  existing?: ShareRecord,
): Promise<"enabled" | "disabled"> {
  if (existing && getProviderSharePhase(existing) === "sharing") {
    await shareApi.disable(existing.id);
    return "disabled";
  }

  await enableBundleShare(bundleId, existing);
  return "enabled";
}

export async function deleteBundleShare(share: ShareRecord): Promise<void> {
  await shareApi.delete(share.id);
}
