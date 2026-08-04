import { shareApi, type ShareRecord } from "@/lib/api/share";
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
  sharedWithEmails: string;
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
    sharedWithEmails: share?.sharedWithEmails.join(", ") ?? "",
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

function normalizedEmails(value: string): string[] {
  const emails = value
    .split(/[\s,;]+/)
    .map((email) => email.trim().toLowerCase())
    .filter(Boolean);
  return [...new Set(emails)].sort();
}

export async function saveBundleShare(
  bundleId: string,
  draft: ProviderBundleShareDraft,
  existing?: ShareRecord,
): Promise<ShareRecord | undefined> {
  const tokenLimit = limitValue(draft.tokenLimit, UNLIMITED_TOKEN_LIMIT);
  const parallelLimit = limitValue(
    draft.parallelLimit,
    UNLIMITED_PARALLEL_LIMIT,
  );
  const expiry = expiresAt(draft);
  const sharedWithEmails = normalizedEmails(draft.sharedWithEmails);
  return shareApi.saveProviderBundleShare({
    bundleId,
    shareId: existing?.id,
    expectedConfigRevision: existing?.configRevision,
    enabled: draft.enabled,
    subdomain: draft.subdomain.trim() || existing?.shareSlug || "",
    description: draft.description.trim() || undefined,
    forSale: draft.forSale,
    marketAccessMode: draft.marketAccessMode,
    tokenLimit,
    parallelLimit,
    expiresAt: expiry,
    sharedWithEmails,
  });
}
