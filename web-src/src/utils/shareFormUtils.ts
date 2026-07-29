import type {
  PublicTokenMarket,
  ShareAccessByApp,
  ShareAppSettingsByApp,
  ShareBindings,
} from "@/lib/api";
import type { ShareUserGrantMap, ShareUserPolicy } from "@/lib/api/share";
import {
  UNLIMITED_PARALLEL_LIMIT,
  UNLIMITED_TOKEN_LIMIT,
} from "@/utils/shareUtils";

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

export function buildShareUserGrantsForAcl({
  source,
  ownerEmail,
  aclEmails,
  defaultPolicy,
}: {
  source: ShareUserGrantMap;
  ownerEmail: string;
  aclEmails: string[];
  defaultPolicy: ShareUserPolicy;
}): ShareUserGrantMap {
  const normalizedOwnerEmail = ownerEmail.trim().toLowerCase();
  const allowedEmails = new Set(
    normalizeShareEmails([normalizedOwnerEmail, ...aclEmails]),
  );
  const sourceByEmail = new Map(
    Object.values(source).map((grant) => [
      grant.email.trim().toLowerCase(),
      grant,
    ]),
  );
  const next: ShareUserGrantMap = {};

  for (const [email, grant] of sourceByEmail) {
    if (email && grant.manager === "routerShareMarket") {
      next[email] = grant;
    }
  }
  for (const email of allowedEmails) {
    const previous = sourceByEmail.get(email);
    if (previous?.manager === "routerShareMarket") {
      next[email] = previous;
      continue;
    }
    next[email] = {
      ...previous,
      email,
      role: email === normalizedOwnerEmail ? "owner" : "shareto",
      active: true,
      policy: previous?.policy ?? { ...defaultPolicy },
    };
  }

  return next;
}

export function formatMarketSelectLabel(market: PublicTokenMarket): string {
  return market.displayName.replace(/^https?:\/\//i, "");
}

export function shareAppDisplayLabel(app: keyof ShareBindings): string {
  if (app === "claude") return "Claude";
  if (app === "codex") return "Codex";
  return "Gemini";
}

export interface BuildShareAclPayloadInput {
  app: keyof ShareBindings;
  forSale: "Yes" | "No" | "Free";
  marketAccessMode: "selected" | "all";
  shareToEmails: string[];
  selectedTokenMarketEmails: string[];
  tokenLimit: number;
  parallelLimit: number;
  expiresAt: string;
}

export function buildShareAclPayload({
  app,
  forSale,
  marketAccessMode,
  shareToEmails,
  selectedTokenMarketEmails,
  tokenLimit,
  parallelLimit,
  expiresAt,
}: BuildShareAclPayloadInput): {
  sharedWithEmails: string[];
  marketAccessMode: "selected" | "all";
  accessByApp: ShareAccessByApp;
  appSettings: ShareAppSettingsByApp;
} {
  const marketEmails =
    forSale !== "Yes"
      ? []
      : marketAccessMode === "all"
        ? []
        : selectedTokenMarketEmails;
  const emails = normalizeShareEmails([...shareToEmails, ...marketEmails]);

  const accessByApp: ShareAccessByApp = {
    [app]: {
      sharedWithEmails: emails,
      marketAccessMode,
    },
  };
  const appSettings: ShareAppSettingsByApp = {
    [app]: {
      forSale,
      marketAccessMode,
      sharedWithEmails: emails,
      tokenLimit: tokenLimit ?? UNLIMITED_TOKEN_LIMIT,
      parallelLimit: parallelLimit ?? UNLIMITED_PARALLEL_LIMIT,
      expiresAt,
    },
  };

  return {
    sharedWithEmails: emails,
    marketAccessMode,
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
