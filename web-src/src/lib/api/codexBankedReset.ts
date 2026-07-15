import { invokeCommand } from "@/lib/runtime";

export interface CodexBankedResetCredit {
  id: string;
  status?: string | null;
  grantedAt?: string | number | null;
  expiresAt?: string | number | null;
  title?: string | null;
  description?: string | null;
  profileUserId?: string | null;
  profileImageUrl?: string | null;
  resetType?: string | null;
}

export interface CodexBankedResetStatus {
  enabled?: boolean;
  readOnly?: boolean;
  workspaceId?: string | null;
  availableCount?: number | null;
  credits: CodexBankedResetCredit[];
  countSource?: string | null;
  detailsSource?: string | null;
  countFetchedAt?: string | number | null;
  detailsFetchedAt?: string | number | null;
  detailsAvailable?: boolean;
  detailsStale?: boolean;
  detailsError?: string | null;
  nextExpiresAt?: string | number | null;
  /** Compatibility timestamp used by snapshots written before split freshness metadata. */
  queriedAt?: string | number | null;
  /** Compatibility source used by snapshots written before count/details were split. */
  source?: string | null;
  /** @deprecated Referral actions are not part of the server reset-credit UI. */
  referralKey?: string;
  /** @deprecated Referral actions are not part of the server reset-credit UI. */
  inviteEligibility?: unknown;
  /** @deprecated Referral actions are not part of the server reset-credit UI. */
  inviteEligibilityError?: string | null;
  /** @deprecated Referral actions are not part of the server reset-credit UI. */
  eligibilityRules?: string[];
  /** @deprecated Referral actions are not part of the server reset-credit UI. */
  requiresConsent?: boolean;
}

export interface CodexBankedResetInviteResult {
  invites: unknown[];
  failedEmails: string[];
  message?: string | null;
}

export interface CodexBankedResetConsumeResult {
  code?: string | null;
  creditId: string;
  redeemRequestId: string;
  availableCount?: number | null;
  remainingCredits: unknown[];
}

export async function getCodexBankedResetStatus(
  accountId?: string | null,
  force = false,
): Promise<CodexBankedResetStatus> {
  return invokeCommand<CodexBankedResetStatus>("codex_banked_reset_status", {
    accountId: accountId || null,
    force,
  });
}

/** @deprecated The server reset-credit surface is read-only. */
export async function sendCodexBankedResetInvite(
  accountId: string | null | undefined,
  emails: string[],
): Promise<CodexBankedResetInviteResult> {
  return invokeCommand<CodexBankedResetInviteResult>(
    "codex_banked_reset_invite",
    {
      accountId: accountId || null,
      emails,
    },
  );
}

/** @deprecated The server reset-credit surface is read-only. */
export async function consumeCodexBankedReset(
  accountId: string | null | undefined,
  creditId: string,
): Promise<CodexBankedResetConsumeResult> {
  return invokeCommand<CodexBankedResetConsumeResult>(
    "codex_banked_reset_consume",
    {
      accountId: accountId || null,
      creditId,
    },
  );
}

export const codexBankedResetApi = {
  getCodexBankedResetStatus,
  sendCodexBankedResetInvite,
  consumeCodexBankedReset,
};
