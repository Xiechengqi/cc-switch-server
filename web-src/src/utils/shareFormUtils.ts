import type { ShareBindings } from "@/lib/api";
import type { ShareUserGrantMap, ShareUserPolicy } from "@/lib/api/share";

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

export function buildShareUserGrants({
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
    if (email && grant.manager === "routerShareMarket" && grant.active !== false) {
      next[email] = grant;
    }
  }
  for (const email of allowedEmails) {
    const previous = sourceByEmail.get(email);
    if (previous?.manager === "routerShareMarket" && previous.active !== false) {
      next[email] = previous;
      continue;
    }
    const reuseMarketTombstone =
      previous?.manager === "routerShareMarket" && previous.active === false;
    next[email] = {
      ...(reuseMarketTombstone ? undefined : previous),
      email,
      role: email === normalizedOwnerEmail ? "owner" : "shareto",
      active: true,
      manager: email === normalizedOwnerEmail ? "owner" : "manual",
      entitlementId: undefined,
      revokedAtMs: undefined,
      policy: previous?.policy ?? { ...defaultPolicy },
    };
  }

  return next;
}

export function shareAppDisplayLabel(app: keyof ShareBindings): string {
  if (app === "claude") return "Claude";
  if (app === "codex") return "Codex";
  return "Gemini";
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
