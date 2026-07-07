import type {
  PublicMarket,
  ShareAccessByApp,
  ShareAppSettingsByApp,
  ShareBindings,
  ShareSaleMarketKind,
} from "@/lib/api";
import {
  UNLIMITED_PARALLEL_LIMIT,
  UNLIMITED_TOKEN_LIMIT,
} from "@/utils/shareUtils";

const SUBDOMAIN_PREFIX_LENGTH = 5;
const SUBDOMAIN_TIMESTAMP_LENGTH = 5;
const EMAIL_PATTERN = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

export function isValidShareEmail(value: string): boolean {
  return EMAIL_PATTERN.test(value);
}

export function uniqueSortedEmails(emails: string[]): string[] {
  return Array.from(new Set(emails)).sort();
}

export function normalizeShareEmails(emails: string[]): string[] {
  return uniqueSortedEmails(
    emails
      .map((email) => email.trim().toLowerCase())
      .filter((email) => email.length > 0 && isValidShareEmail(email)),
  );
}

export function formatMarketSelectLabel(market: PublicMarket): string {
  return market.displayName.replace(/^https?:\/\//i, "");
}

export function deriveSubdomainFromEmail(
  email: string | null | undefined,
): string {
  const local = (email ?? "").split("@")[0] ?? "";
  const filtered = local.toLowerCase().replace(/[^a-z]/g, "");
  const prefix =
    filtered.length === 0 ? "s" : filtered.slice(0, SUBDOMAIN_PREFIX_LENGTH);
  const suffix = Date.now().toString(36).slice(-SUBDOMAIN_TIMESTAMP_LENGTH);
  return `${prefix}-${suffix}`;
}

export function shareAppDisplayLabel(app: keyof ShareBindings): string {
  if (app === "claude") return "Claude";
  if (app === "codex") return "Codex";
  return "Gemini";
}

export interface BuildShareAclPayloadInput {
  app: keyof ShareBindings;
  forSale: "Yes" | "No" | "Free";
  saleMarketKind: ShareSaleMarketKind;
  marketAccessMode: "selected" | "all";
  shareToEmails: string[];
  selectedTokenMarketEmails: string[];
  selectedShareMarketEmail: string;
  tokenLimit: number;
  parallelLimit: number;
  expiresAt: string;
}

export function buildShareAclPayload({
  app,
  forSale,
  saleMarketKind,
  marketAccessMode,
  shareToEmails,
  selectedTokenMarketEmails,
  selectedShareMarketEmail,
  tokenLimit,
  parallelLimit,
  expiresAt,
}: BuildShareAclPayloadInput): {
  sharedWithEmails: string[];
  marketAccessMode: "selected" | "all";
  saleMarketKind: ShareSaleMarketKind;
  accessByApp: ShareAccessByApp;
  appSettings: ShareAppSettingsByApp;
} {
  const marketEmails =
    forSale !== "Yes"
      ? []
      : saleMarketKind === "share"
        ? selectedShareMarketEmail
          ? [selectedShareMarketEmail]
          : []
        : marketAccessMode === "all"
          ? []
          : selectedTokenMarketEmails;
  const emails = normalizeShareEmails([...shareToEmails, ...marketEmails]);
  const effectiveMarketAccessMode =
    forSale === "Yes" && saleMarketKind === "share"
      ? "selected"
      : marketAccessMode;

  const accessByApp: ShareAccessByApp = {
    [app]: {
      sharedWithEmails: emails,
      marketAccessMode: effectiveMarketAccessMode,
    },
  };
  const appSettings: ShareAppSettingsByApp = {
    [app]: {
      forSale,
      saleMarketKind,
      marketAccessMode: effectiveMarketAccessMode,
      sharedWithEmails: emails,
      tokenLimit: tokenLimit ?? UNLIMITED_TOKEN_LIMIT,
      parallelLimit: parallelLimit ?? UNLIMITED_PARALLEL_LIMIT,
      expiresAt,
    },
  };

  return {
    sharedWithEmails: emails,
    marketAccessMode: effectiveMarketAccessMode,
    saleMarketKind,
    accessByApp,
    appSettings,
  };
}

export const SHARE_EXPIRY_PRESETS = [
  { labelKey: "share.expiry.oneHour", value: 3600 },
  { labelKey: "share.expiry.sixHours", value: 6 * 3600 },
  { labelKey: "share.expiry.oneDay", value: 24 * 3600 },
  { labelKey: "share.expiry.sevenDays", value: 7 * 24 * 3600 },
  { labelKey: "share.expiry.thirtyDays", value: 30 * 24 * 3600 },
] as const;

export const SHARE_TOKEN_PRESETS = [10000, 50000, 100000, 500000] as const;
export const DEFAULT_SHARE_TOKEN_LIMIT_FALLBACK = 100000;
