import { invokeCommand } from "@/lib/runtime";

export interface CodexBankedResetCredit {
  id: string;
  status?: string | null;
  grantedAt?: string | null;
  expiresAt?: string | null;
  title?: string | null;
  description?: string | null;
  profileUserId?: string | null;
  profileImageUrl?: string | null;
}

export interface CodexBankedResetStatus {
  referralKey: string;
  inviteEligibility?: unknown;
  inviteEligibilityError?: string | null;
  eligibilityRules: string[];
  requiresConsent: boolean;
  availableCount: number;
  credits: CodexBankedResetCredit[];
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
): Promise<CodexBankedResetStatus> {
  return invokeCommand<CodexBankedResetStatus>("codex_banked_reset_status", {
    accountId: accountId || null,
  });
}

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
