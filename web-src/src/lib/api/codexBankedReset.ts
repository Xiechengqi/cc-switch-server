import { invokeCommand } from "@/lib/runtime";

export interface CodexBankedResetCredit {
  id: string;
  idAvailable?: boolean;
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
  queriedAt?: string | number | null;
  source?: string | null;
}

export interface CodexBankedResetConsumeResult {
  code?: string | null;
  creditId: string;
  redeemRequestId: string;
  availableCount?: number | null;
  remainingCredits: unknown[];
}

export interface CodexBankedResetTarget {
  accountId?: string | null;
  providerId?: string | null;
  expectedRevision?: number | null;
}

export async function getCodexBankedResetStatus(
  target: CodexBankedResetTarget,
  force = false,
): Promise<CodexBankedResetStatus> {
  return invokeCommand<CodexBankedResetStatus>("codex_banked_reset_status", {
    accountId: target.accountId || null,
    providerId: target.providerId || null,
    expectedRevision: target.expectedRevision ?? null,
    force,
  });
}

export async function consumeCodexBankedReset(
  target: CodexBankedResetTarget,
  creditId?: string | null,
): Promise<CodexBankedResetConsumeResult> {
  return invokeCommand<CodexBankedResetConsumeResult>(
    "codex_banked_reset_consume",
    {
      accountId: target.accountId || null,
      providerId: target.providerId || null,
      expectedRevision: target.expectedRevision ?? null,
      creditId: creditId?.trim() || null,
    },
  );
}

export const codexBankedResetApi = {
  getCodexBankedResetStatus,
  consumeCodexBankedReset,
};
