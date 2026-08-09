import { invokeCommand } from "@/lib/runtime";

export interface CodexReferralGrant {
  recipient?: string | null;
  grantType?: string | null;
  amount?: number | null;
  rewardId?: string | null;
}

export interface CodexReferralTimeFrameRule {
  invitesSent?: number | null;
  invitesTotal?: number | null;
  timeFrame?: string | null;
  ruleType?: string | null;
  capacityType?: string | null;
}

export interface CodexReferralEligibility {
  ok: boolean;
  statusCode: number;
  requestId?: string | null;
  shouldShow: boolean;
  ineligibleReason?: string | null;
  ineligibleReasonCode?: string | null;
  programId: string;
  entrypoint: string;
  offerId?: string | null;
  grants: CodexReferralGrant[];
  remainingSendCapacity?: number | null;
  remainingRewardCapacity?: number | null;
  title?: string | null;
  description?: string | null;
  rules: string[];
  timeFrameRules: CodexReferralTimeFrameRule[];
  requiresExplicitConfirmation: boolean;
  upstreamMessage?: string | null;
  challenged: boolean;
  diagnostic?: string | null;
}

export interface CodexReferralInviteItem {
  referralId?: string | null;
  email?: string | null;
  inviteUrl?: string | null;
}

export interface CodexReferralSendResult {
  ok: boolean;
  statusCode: number;
  requestId?: string | null;
  programId: string;
  entrypoint: string;
  emails: string[];
  invites: CodexReferralInviteItem[];
  upstreamMessage?: string | null;
  failedEmails: string[];
  challenged: boolean;
  diagnostic?: string | null;
}

export interface CodexReferralTrackingItem {
  referralId?: string | null;
  email?: string | null;
  status?: string | null;
  canResend: boolean;
  inviteUrl?: string | null;
  resendAvailableAt?: string | null;
  grants: CodexReferralGrant[];
  createdAt?: string | null;
  expiresAt?: string | null;
}

export interface CodexReferralTracking {
  ok: boolean;
  statusCode: number;
  requestId?: string | null;
  items: CodexReferralTrackingItem[];
  cursor?: string | null;
  upstreamMessage?: string | null;
  challenged: boolean;
  diagnostic?: string | null;
}

export interface CodexReferralTarget {
  providerId: string;
  expectedRevision: number;
}

export function getCodexReferralEligibility(
  target: CodexReferralTarget,
): Promise<CodexReferralEligibility> {
  return invokeCommand("codex_referral_eligibility", { ...target });
}

export function getCodexReferralTracking(
  target: CodexReferralTarget,
  limit = 100,
): Promise<CodexReferralTracking> {
  return invokeCommand("codex_referral_tracking", { ...target, limit });
}

export function sendCodexReferrals(
  target: CodexReferralTarget,
  emails: string[],
): Promise<CodexReferralSendResult> {
  return invokeCommand("codex_referral_send", { ...target, emails });
}

export const codexReferralsApi = {
  getEligibility: getCodexReferralEligibility,
  getTracking: getCodexReferralTracking,
  send: sendCodexReferrals,
};
