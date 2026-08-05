import {
  shareApi,
  type ShareRecord,
  type ShareUserGrantMap,
  type ShareUserPolicy,
} from "@/lib/api/share";
import {
  buildShareUserGrantsForAcl,
  isValidShareEmail,
  normalizeShareEmails,
} from "@/utils/shareFormUtils";
import {
  PERMANENT_EXPIRES_AT,
  UNLIMITED_PARALLEL_LIMIT,
  UNLIMITED_TOKEN_LIMIT,
} from "@/utils/shareUtils";

export interface ProviderBundleShareDraft {
  enabled: boolean;
  subdomain: string;
  description: string;
  forSale: "Yes" | "No" | "Free";
  marketAccessMode: "selected" | "all";
  tokenLimit: string;
  parallelLimit: string;
  expiry: ProviderBundleShareExpiry;
  sharedWithEmails: string[];
  userGrants: ShareUserGrantMap;
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
  const sharedWithEmails = normalizeShareEmails(share?.sharedWithEmails ?? []);
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
    forSale: share?.forSale ?? "Yes",
    marketAccessMode: share?.marketAccessMode ?? "all",
    tokenLimit: share && share.tokenLimit >= 0 ? String(share.tokenLimit) : "",
    parallelLimit:
      share && share.parallelLimit >= 0 ? String(share.parallelLimit) : "",
    expiry,
    sharedWithEmails,
    userGrants: share
      ? buildShareUserGrantsForAcl({
          source: share.userGrants,
          ownerEmail: share.ownerEmail,
          aclEmails: sharedWithEmails,
          defaultPolicy,
        })
      : {},
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
  if (draft.sharedWithEmails.some((email) => !isValidShareEmail(email))) {
    throw new Error("Share email is invalid");
  }
  const tokenLimit = limitValue(draft.tokenLimit, UNLIMITED_TOKEN_LIMIT);
  const parallelLimit = limitValue(
    draft.parallelLimit,
    UNLIMITED_PARALLEL_LIMIT,
  );
  const expiry = expiresAt(draft);
  const sharedWithEmails = normalizeShareEmails(draft.sharedWithEmails);
  const userGrants = Object.keys(draft.userGrants).length
    ? draft.userGrants
    : undefined;
  return shareApi.saveProviderBundleShare({
    bundleId,
    shareId: existing?.id,
    expectedConfigRevision: existing?.configRevision,
    enabled: draft.enabled,
    subdomain: subdomain || existing?.shareSlug || "",
    description: draft.description.trim() || undefined,
    forSale: draft.forSale,
    marketAccessMode: draft.marketAccessMode,
    tokenLimit,
    parallelLimit,
    expiresAt: expiry,
    sharedWithEmails,
    userGrants,
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
